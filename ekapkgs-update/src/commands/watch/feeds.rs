//! Feed polling for upstream release detection.
//!
//! Supports two strategies:
//! - **RSS/Atom feeds** (GitHub releases then tags, PyPI) — parsed with `feed-rs`
//! - **API polling** (GitLab, SourceHut, DirectoryListing) — reuses existing
//!   platform clients via [`UpstreamSource::fetch_all_releases`]

use std::cmp::Ordering;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use anyhow::Context;
use tracing::{debug, warn};

use crate::database::{SourceHealth, UpstreamIndexEntry};
use crate::github::parse_github_url;
use crate::vcs_sources::{
    HttpStatusError, Release, UpstreamSource, compare_version_components, is_version_acceptable,
    version_looks_prerelease,
};

/// Decide whether an indexed package should be updated, given the releases
/// polled for its upstream source.
///
/// Applies the entry's recorded version policy (semver strategy, prerelease
/// handling, version regex) and returns the target version only when it is
/// strictly newer than the indexed version.
pub fn update_version_for_entry(
    entry: &UpstreamIndexEntry,
    releases: &[Release],
) -> Option<String> {
    let current = entry.current_version.as_deref().filter(|v| !v.is_empty())?;
    let (_, version) = entry
        .version_policy()
        .best_release(releases.to_vec(), current)?;
    (compare_version_components(&version, current) == Ordering::Greater).then_some(version)
}

/// Whether any package tracking a source can use `releases`: for at least one
/// entry, a release passes its version policy (prerelease and regex filters)
/// and is either its current version (the feed shows it is up to date) or a
/// newer one its strategy accepts. A feed whose entries are all filtered out
/// (a monorepo's feed full of other components' tags, only a new major under
/// a `minor` strategy, or only releases older than the package) is not an
/// answer: the next feed or the API may list more.
pub fn releases_usable(entries: &[UpstreamIndexEntry], releases: &[Release]) -> bool {
    entries.iter().any(|entry| {
        let Some(current) = entry.current_version.as_deref().filter(|v| !v.is_empty()) else {
            return false;
        };
        let policy = entry.version_policy();
        releases.iter().any(|release| {
            if release.is_prerelease && !policy.include_prereleases {
                return false;
            }
            let Ok(version) = release.resolved_version(policy.version_regex.as_deref()) else {
                return false;
            };
            if !policy.include_prereleases && version_looks_prerelease(&version) {
                return false;
            }
            compare_version_components(&version, current) == Ordering::Equal
                || is_version_acceptable(current, &version, policy.strategy)
        })
    })
}

/// Releases polled from an upstream source.
#[derive(Debug, Default)]
pub struct PolledSource {
    pub releases: Vec<Release>,
    /// Where the source answered from when it redirected to another
    /// repository (a GitHub rename or transfer), as `owner/repo`.
    pub moved_to: Option<String>,
    /// The releases came from an RSS/Atom feed, which carries no prerelease
    /// (GitHub) or yanked (PyPI) flags: see [`release_flagged_prerelease`].
    pub from_feed: bool,
}

/// Shared by a poll cycle's sources: once a platform API answered 429 (or
/// GitHub's rate-limit 403), the rest of the cycle makes no more API
/// fallback requests. Feeds (not rate limited the same way) are still read.
#[derive(Debug, Default)]
pub struct ApiGate {
    rate_limited: AtomicBool,
}

impl ApiGate {
    pub fn is_rate_limited(&self) -> bool {
        self.rate_limited.load(AtomicOrdering::Relaxed)
    }

    /// Note an API failure; returns whether it was a rate limit.
    pub fn note_error(&self, err: &anyhow::Error) -> bool {
        let limited = HttpStatusError::find(err).is_some_and(HttpStatusError::is_rate_limited);
        if limited && !self.rate_limited.swap(true, AtomicOrdering::Relaxed) {
            warn!("Upstream API rate limited ({err:#}); no more API polls this cycle");
        }
        limited
    }
}

/// A source was not polled: it needs the API, which is rate limited for the
/// rest of the cycle. Nothing is recorded for it, so it stays due.
#[derive(Debug)]
pub struct ApiSkipped;

impl std::fmt::Display for ApiSkipped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("API rate limited, poll skipped")
    }
}

impl std::error::Error for ApiSkipped {}

/// Poll a single upstream source for its releases.
///
/// Uses RSS/Atom feeds when available (GitHub releases then tags, PyPI); the
/// first feed with entries `usable` accepts wins, so the common case stays
/// one request. Otherwise the platform API is asked
/// ([`UpstreamSource::fetch_all_releases`]); when it fails or lists nothing,
/// the first non-empty feed is used anyway. A feed that redirects to another
/// repository marks the source as moved (the GitHub API redirects to a
/// numeric repository id, so only the feed shows the new name).
pub async fn poll_source(
    source: &UpstreamSource,
    usable: impl Fn(&[Release]) -> bool,
    gate: &ApiGate,
) -> anyhow::Result<PolledSource> {
    let label = format!("{}/{}", source.source_type(), source.source_key());
    poll_with(
        &label,
        source.feed_urls(),
        |url| async move {
            let (releases, final_url) = poll_feed(&url).await?;
            Ok((releases, relocation(source, &final_url)))
        },
        || async {
            source
                .fetch_all_releases()
                .await
                .with_context(|| format!("API poll failed for {label}"))
        },
        usable,
        gate,
    )
    .await
}

/// [`poll_source`] with the requests passed in (tested without network).
/// `feed` returns a feed's releases and the relocation it showed.
pub async fn poll_with<F, FFut, A, AFut>(
    label: &str,
    feed_urls: Vec<String>,
    feed: F,
    api: A,
    usable: impl Fn(&[Release]) -> bool,
    gate: &ApiGate,
) -> anyhow::Result<PolledSource>
where
    F: Fn(String) -> FFut,
    FFut: Future<Output = anyhow::Result<(Vec<Release>, Option<String>)>>,
    A: FnOnce() -> AFut,
    AFut: Future<Output = anyhow::Result<Vec<Release>>>,
{
    let mut moved_to = None;
    // First feed that had entries, none of them usable.
    let mut unusable: Option<Vec<Release>> = None;
    for feed_url in feed_urls {
        match feed(feed_url.clone()).await {
            Ok((releases, relocated)) => {
                if moved_to.is_none() {
                    moved_to = relocated;
                }
                if releases.is_empty() {
                    debug!("{label}: feed {feed_url} returned no entries");
                } else if usable(&releases) {
                    return Ok(PolledSource {
                        releases,
                        moved_to,
                        from_feed: true,
                    });
                } else {
                    debug!("{label}: no entry of feed {feed_url} is usable");
                    unusable.get_or_insert(releases);
                }
            },
            Err(e) => debug!("{label}: feed {feed_url} failed: {e:#}"),
        }
    }

    let from_feed = |releases| PolledSource {
        releases,
        moved_to: moved_to.clone(),
        from_feed: true,
    };
    if gate.is_rate_limited() {
        return match unusable {
            Some(releases) => Ok(from_feed(releases)),
            None => Err(ApiSkipped.into()),
        };
    }
    match api().await {
        Ok(releases) if releases.is_empty() && unusable.is_some() => {
            Ok(from_feed(unusable.unwrap_or_default()))
        },
        Ok(releases) => Ok(PolledSource {
            releases,
            moved_to,
            from_feed: false,
        }),
        Err(e) => {
            gate.note_error(&e);
            match unusable {
                Some(releases) => {
                    debug!("{label}: API fallback failed, using the feed: {e:#}");
                    Ok(from_feed(releases))
                },
                None => Err(e),
            }
        },
    }
}

/// Whether the platform flags a release the feed listed as a prerelease
/// (GitHub's release `prerelease`) or withdrawn (a yanked PyPI version).
/// Feeds carry neither flag. One API request; only asked for a release about
/// to be dispatched. Other platforms' releases come from their APIs already.
pub async fn release_flagged_prerelease(
    source: &UpstreamSource,
    release: &Release,
) -> anyhow::Result<bool> {
    match source {
        UpstreamSource::GitHub { owner, repo } => {
            let token = crate::github::auth::read_token().await;
            Ok(crate::github::fetch_github_release_by_tag(
                owner,
                repo,
                &release.tag_name,
                token.as_deref(),
            )
            .await?
            .is_some_and(|r| r.prerelease))
        },
        UpstreamSource::PyPI { pname } => {
            crate::pypi::release_is_yanked(pname, &release.tag_name).await
        },
        UpstreamSource::GitLab { .. }
        | UpstreamSource::SourceHut { .. }
        | UpstreamSource::DirectoryListing { .. }
        | UpstreamSource::Script { .. } => Ok(false),
    }
}

/// The repository a source answered from, when a request for it was
/// redirected elsewhere (`owner/repo`). Case-only differences are not moves.
pub fn relocation(source: &UpstreamSource, final_url: &str) -> Option<String> {
    let UpstreamSource::GitHub { owner, repo } = source else {
        return None;
    };
    let actual = parse_github_url(final_url)?;
    (!actual.owner.eq_ignore_ascii_case(owner) || !actual.repo.eq_ignore_ascii_case(repo))
        .then(|| format!("{}/{}", actual.owner, actual.repo))
}

/// Health of a source whose poll failed with `err`.
pub fn failure_health(err: &anyhow::Error) -> SourceHealth {
    if HttpStatusError::find(err).is_some_and(HttpStatusError::is_not_found) {
        SourceHealth::NotFound
    } else {
        SourceHealth::Error
    }
}

/// Fetch and parse an RSS or Atom feed, returning releases and the URL
/// that answered (after redirects).
async fn poll_feed(url: &str) -> anyhow::Result<(Vec<Release>, String)> {
    let client = reqwest::Client::builder()
        .user_agent("ekapkgs-update/0.1")
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetch feed {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!("feed {} returned HTTP {}", url, response.status());
    }
    let final_url = response.url().to_string();

    let body = response
        .bytes()
        .await
        .with_context(|| format!("read feed body from {url}"))?;

    Ok((parse_feed_bytes(&body)?, final_url))
}

/// Parse raw RSS/Atom feed bytes into releases.
///
/// Separated from [`poll_feed`] so feed parsing can be tested with fixture data.
pub fn parse_feed_bytes(data: &[u8]) -> anyhow::Result<Vec<Release>> {
    let feed = feed_rs::parser::parse(data).context("parse feed")?;

    let releases: Vec<Release> = feed
        .entries
        .into_iter()
        .filter_map(|entry| {
            // GitHub release feeds link to `/releases/tag/<tag>`; the entry
            // title is the release *name*, which is often not the tag
            // (e.g. "Release 1.2.3"). PyPI uses the version as the title.
            let tag_from_link = entry
                .links
                .iter()
                .find_map(|link| tag_from_release_link(&link.href));
            let tag_name = tag_from_link
                .or_else(|| entry.title.map(|t| t.content.trim().to_owned()))
                .or_else(|| {
                    // Fall back to the last path segment of the entry link
                    entry.links.first().and_then(|link| {
                        link.href
                            .trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .map(String::from)
                    })
                })?;

            if tag_name.is_empty() {
                return None;
            }

            Some(Release {
                tag_name,
                is_prerelease: false, // RSS feeds don't carry prerelease info
            })
        })
        .collect();

    Ok(releases)
}

/// Extract the tag from a GitHub release link
/// (`https://github.com/o/r/releases/tag/v1.2.3`), percent-decoded.
fn tag_from_release_link(href: &str) -> Option<String> {
    let (_, tag) = href.split_once("/releases/tag/")?;
    let tag = tag.split(['?', '#']).next()?.trim_end_matches('/');
    if tag.is_empty() {
        return None;
    }
    Some(percent_decode(tag))
}

/// Minimal `%XX` decoder (invalid escapes are kept verbatim).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(byte) = s
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// Reconstruct an [`UpstreamSource`] from database index fields.
///
/// This is the inverse of [`UpstreamSource::source_type`] +
/// [`UpstreamSource::source_key`] + [`UpstreamSource::instance`].
pub fn upstream_source_from_index(
    upstream_type: &str,
    upstream_key: &str,
    instance: Option<&str>,
) -> Option<UpstreamSource> {
    match upstream_type {
        "github" => {
            let (owner, repo) = upstream_key.split_once('/')?;
            Some(UpstreamSource::GitHub {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
            })
        },
        "gitlab" => {
            let (owner, project) = upstream_key.split_once('/')?;
            Some(UpstreamSource::GitLab {
                instance: instance?.to_owned(),
                owner: owner.to_owned(),
                project: project.to_owned(),
            })
        },
        "sourcehut" => {
            // Older indexes stored `~~owner/repo`.
            let key = upstream_key.trim_start_matches('~');
            let (owner, repo) = key.split_once('/')?;
            Some(UpstreamSource::SourceHut {
                owner: format!("~{owner}"),
                repo: repo.to_owned(),
            })
        },
        "pypi" => Some(UpstreamSource::PyPI {
            pname: upstream_key.to_owned(),
        }),
        "directory" => {
            // upstream_key is the base_url; recover the pname from it. GNU
            // listings end in `/<pname>/`, SourceForge ones in
            // `/projects/<pname>/files/` (see `UpstreamSource::from_url`).
            let trimmed = upstream_key.trim_end_matches('/');
            let pname = trimmed
                .strip_suffix("/files")
                .and_then(|rest| rest.rsplit_once("/projects/"))
                .map(|(_, pname)| pname)
                .or_else(|| trimmed.rsplit('/').next())
                .unwrap_or(upstream_key);
            Some(UpstreamSource::DirectoryListing {
                base_url: upstream_key.to_owned(),
                pname: pname.to_owned(),
            })
        },
        "script" => Some(UpstreamSource::Script {
            script_path: upstream_key.to_owned(),
        }),
        other => {
            warn!("Unknown upstream type in index: {}", other);
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn releases(tags: &[&str]) -> Vec<Release> {
        tags.iter()
            .map(|t| Release {
                tag_name: (*t).to_owned(),
                is_prerelease: false,
            })
            .collect()
    }

    fn entry(current: &str, regex: Option<&str>, strategy: Option<&str>) -> UpstreamIndexEntry {
        UpstreamIndexEntry {
            repo_name: String::new(),
            attr_path: "foo".into(),
            upstream_type: "github".into(),
            upstream_key: "o/r".into(),
            instance: None,
            current_version: Some(current.into()),
            src_url: None,
            drv_path: None,
            pname: None,
            semver_strategy: strategy.map(str::to_owned),
            version_regex: regex.map(str::to_owned),
            include_prereleases: false,
        }
    }

    #[test]
    fn releases_usable_cases() {
        let monorepo = releases(&["vscode-v0.3.0", "vscode-v0.2.0"]);
        let regex = Some(r"^cli-v(.+)$");
        assert!(!releases_usable(&[entry("1.0.0", regex, None)], &monorepo));
        assert!(releases_usable(
            &[entry("1.0.0", regex, None)],
            &releases(&["cli-v1.0.0"])
        ));
        // Without a regex, other components' tags don't resolve to the
        // package's versions either.
        assert!(!releases_usable(&[entry("1.0.0", None, None)], &monorepo));
        // Only a new major under a minor strategy: not usable.
        assert!(!releases_usable(
            &[entry("1.4.0", None, Some("minor"))],
            &releases(&["v2.0.0"])
        ));
        // Any package of the source suffices.
        assert!(releases_usable(
            &[
                entry("1.0.0", regex, None),
                entry("0.2.0", Some(r"^vscode-v(.+)$"), None)
            ],
            &monorepo
        ));
        // The current version: up to date, usable. Only older ones: not.
        assert!(releases_usable(
            &[entry("1.4.0", None, Some("minor"))],
            &releases(&["v2.0.0", "v1.4.0"])
        ));
        assert!(!releases_usable(
            &[entry("1.4.0", None, None)],
            &releases(&["v1.3.0", "v1.2.0"])
        ));
        assert!(!releases_usable(&[entry("1.0.0", None, None)], &[]));
        assert!(!releases_usable(&[], &monorepo));
    }

    /// Feed outcome: `None` = request failed.
    type Canned = Vec<(&'static str, Option<Vec<Release>>)>;

    async fn run_poll(
        feeds: Canned,
        api: Option<Vec<Release>>,
        api_status: Option<reqwest::StatusCode>,
        usable: impl Fn(&[Release]) -> bool,
        gate: &ApiGate,
    ) -> (anyhow::Result<PolledSource>, Vec<String>) {
        let calls = std::sync::Mutex::new(Vec::new());
        let urls = feeds.iter().map(|(u, _)| (*u).to_owned()).collect();
        let result = poll_with(
            "test",
            urls,
            |url: String| {
                calls.lock().unwrap().push(url.clone());
                let canned = feeds
                    .iter()
                    .find(|(u, _)| *u == url)
                    .and_then(|(_, r)| r.clone());
                async move {
                    canned
                        .map(|r| (r, None))
                        .ok_or_else(|| anyhow::anyhow!("feed failed"))
                }
            },
            || {
                calls.lock().unwrap().push("api".to_owned());
                async move {
                    match (api, api_status) {
                        (Some(r), _) => Ok(r),
                        (None, Some(status)) => {
                            Err(HttpStatusError::new("GitHub tags API request", status).into())
                        },
                        (None, None) => Err(anyhow::anyhow!("api failed")),
                    }
                }
            },
            usable,
            gate,
        )
        .await;
        (result, calls.into_inner().unwrap())
    }

    #[tokio::test]
    async fn poll_falls_back_until_usable() {
        let wanted = |r: &[Release]| r.iter().any(|r| r.tag_name.starts_with("cli-"));
        let tags = |r: &anyhow::Result<PolledSource>| -> Vec<String> {
            r.as_ref()
                .unwrap()
                .releases
                .iter()
                .map(|r| r.tag_name.clone())
                .collect()
        };
        let gate = ApiGate::default();

        // Usable releases feed: one request.
        let (r, calls) = run_poll(
            vec![
                ("releases", Some(releases(&["cli-v1.0"]))),
                ("tags", Some(releases(&["cli-v0.9"]))),
            ],
            None,
            None,
            wanted,
            &gate,
        )
        .await;
        assert_eq!(
            (tags(&r), calls),
            (vec!["cli-v1.0".into()], vec!["releases".into()])
        );
        assert!(r.unwrap().from_feed);

        // Releases feed without usable entries: tags feed.
        let (r, calls) = run_poll(
            vec![
                ("releases", Some(releases(&["vscode-v1.0"]))),
                ("tags", Some(releases(&["cli-v1.0"]))),
            ],
            None,
            None,
            wanted,
            &gate,
        )
        .await;
        assert_eq!(tags(&r), vec!["cli-v1.0"]);
        assert_eq!(calls, vec!["releases", "tags"]);

        // No feed usable: the API.
        let (r, calls) = run_poll(
            vec![
                ("releases", Some(releases(&["vscode-v1.0"]))),
                ("tags", Some(Vec::new())),
            ],
            Some(releases(&["cli-v1.0", "vscode-v1.0"])),
            None,
            wanted,
            &gate,
        )
        .await;
        assert_eq!(tags(&r), vec!["cli-v1.0", "vscode-v1.0"]);
        assert_eq!(calls, vec!["releases", "tags", "api"]);
        assert!(!r.unwrap().from_feed);

        // API failed or empty: the unusable feed is still an answer.
        for api in [None, Some(Vec::new())] {
            let (r, _) = run_poll(
                vec![("releases", Some(releases(&["vscode-v1.0"])))],
                api,
                None,
                wanted,
                &gate,
            )
            .await;
            assert_eq!(tags(&r), vec!["vscode-v1.0"]);
        }

        // Every feed failed, API failed: the API's error.
        let (r, _) = run_poll(vec![("releases", None)], None, None, wanted, &gate).await;
        assert_eq!(r.unwrap_err().to_string(), "api failed");
        assert!(!gate.is_rate_limited());
    }

    #[tokio::test]
    async fn rate_limit_stops_api_fallbacks() {
        let gate = ApiGate::default();
        let any = |_: &[Release]| true;
        let (r, _) = run_poll(
            Vec::new(),
            None,
            Some(reqwest::StatusCode::FORBIDDEN),
            any,
            &gate,
        )
        .await;
        assert!(HttpStatusError::find(&r.unwrap_err()).is_some());
        assert!(gate.is_rate_limited());

        // Later sources needing the API are skipped without a request.
        let (r, calls) = run_poll(
            vec![("releases", Some(Vec::new()))],
            Some(releases(&["v1"])),
            None,
            any,
            &gate,
        )
        .await;
        assert!(r.unwrap_err().downcast_ref::<ApiSkipped>().is_some());
        assert_eq!(calls, vec!["releases"]);
        // ... unless a feed listed something.
        let (r, calls) = run_poll(
            vec![("releases", Some(releases(&["v0"])))],
            Some(releases(&["v1"])),
            None,
            |_: &[Release]| false,
            &gate,
        )
        .await;
        assert_eq!(r.unwrap().releases[0].tag_name, "v0");
        assert_eq!(calls, vec!["releases"]);
    }

    #[test]
    fn github_feed_uses_tag_from_link_not_release_name() {
        let atom = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Release notes from foo</title>
  <entry>
    <id>tag:github.com,2008:Repository/1/v2.0.0</id>
    <title>Foo 2.0: the big one</title>
    <link rel="alternate" type="text/html" href="https://github.com/o/foo/releases/tag/v2.0.0"/>
    <updated>2026-01-01T00:00:00Z</updated>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/1/release%2B1.9</id>
    <title>1.9</title>
    <link rel="alternate" type="text/html" href="https://github.com/o/foo/releases/tag/release%2B1.9"/>
    <updated>2025-01-01T00:00:00Z</updated>
  </entry>
</feed>"#;
        let tags: Vec<String> = parse_feed_bytes(atom)
            .unwrap()
            .into_iter()
            .map(|r| r.tag_name)
            .collect();
        assert_eq!(tags, vec!["v2.0.0", "release+1.9"]);
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(
            tag_from_release_link("https://pypi.org/project/x/1.0/"),
            None
        );
    }

    #[test]
    fn roundtrip_github() {
        let source = UpstreamSource::GitHub {
            owner: "BurntSushi".to_owned(),
            repo: "ripgrep".to_owned(),
        };
        let reconstructed = upstream_source_from_index(
            source.source_type(),
            &source.source_key(),
            source.instance(),
        )
        .unwrap();
        assert_eq!(reconstructed.source_type(), "github");
        assert_eq!(reconstructed.source_key(), "BurntSushi/ripgrep");
        assert_eq!(
            reconstructed.feed_urls(),
            vec![
                "https://github.com/BurntSushi/ripgrep/releases.atom".to_owned(),
                "https://github.com/BurntSushi/ripgrep/tags.atom".to_owned(),
            ]
        );
    }

    #[test]
    fn roundtrip_gitlab() {
        let source = UpstreamSource::GitLab {
            instance: "gitlab.freedesktop.org".to_owned(),
            owner: "mesa".to_owned(),
            project: "mesa".to_owned(),
        };
        let reconstructed = upstream_source_from_index(
            source.source_type(),
            &source.source_key(),
            source.instance(),
        )
        .unwrap();
        assert_eq!(reconstructed.source_type(), "gitlab");
        assert_eq!(reconstructed.source_key(), "mesa/mesa");
        assert_eq!(reconstructed.instance(), Some("gitlab.freedesktop.org"));
        assert!(reconstructed.feed_urls().is_empty());
    }

    #[test]
    fn roundtrip_pypi() {
        let source = UpstreamSource::PyPI {
            pname: "requests".to_owned(),
        };
        let reconstructed = upstream_source_from_index(
            source.source_type(),
            &source.source_key(),
            source.instance(),
        )
        .unwrap();
        assert_eq!(reconstructed.source_type(), "pypi");
        assert_eq!(reconstructed.source_key(), "requests");
        assert_eq!(
            reconstructed.feed_urls(),
            vec!["https://pypi.org/rss/project/requests/releases.xml".to_owned()]
        );
    }

    #[test]
    fn roundtrip_sourcehut() {
        let source = UpstreamSource::SourceHut {
            owner: "~sircmpwn".to_owned(),
            repo: "scdoc".to_owned(),
        };
        assert_eq!(source.source_key(), "~sircmpwn/scdoc");
        // Keys written before the doubled `~` was fixed still resolve.
        for key in ["~sircmpwn/scdoc", "~~sircmpwn/scdoc"] {
            let reconstructed =
                upstream_source_from_index(source.source_type(), key, source.instance()).unwrap();
            assert_eq!(reconstructed.source_type(), "sourcehut");
            assert_eq!(reconstructed.source_key(), "~sircmpwn/scdoc");
            assert!(matches!(
                reconstructed,
                UpstreamSource::SourceHut { ref owner, .. } if owner == "~sircmpwn"
            ));
        }
    }

    #[test]
    fn roundtrip_directory() {
        let source = UpstreamSource::DirectoryListing {
            base_url: "https://ftp.gnu.org/gnu/autoconf/".to_owned(),
            pname: "autoconf".to_owned(),
        };
        let reconstructed = upstream_source_from_index(
            source.source_type(),
            &source.source_key(),
            source.instance(),
        )
        .unwrap();
        assert_eq!(reconstructed.source_type(), "directory");
        assert_eq!(
            reconstructed.source_key(),
            "https://ftp.gnu.org/gnu/autoconf/"
        );
    }

    #[test]
    fn unknown_type_returns_none() {
        assert!(upstream_source_from_index("bitbucket", "foo/bar", None).is_none());
    }

    #[test]
    fn roundtrip_sourceforge_directory_keeps_pname() {
        let source = UpstreamSource::DirectoryListing {
            base_url: "https://sourceforge.net/projects/libpng/files/".to_owned(),
            pname: "libpng".to_owned(),
        };
        let reconstructed = upstream_source_from_index(
            source.source_type(),
            &source.source_key(),
            source.instance(),
        )
        .unwrap();
        assert_eq!(
            reconstructed.description(),
            source.description(),
            "pname must survive the index round-trip"
        );

        let explicit = upstream_source_from_index(
            "directory",
            "https://sourceforge.net/projects/libpng/files/",
            None,
        )
        .unwrap();
        assert_eq!(
            explicit.description(),
            "Directory listing: libpng (https://sourceforge.net/projects/libpng/files/)"
        );
    }

    #[test]
    fn relocation_cases() {
        let gh = UpstreamSource::GitHub {
            owner: "rust-lang-nursery".to_owned(),
            repo: "rustfmt".to_owned(),
        };
        for (final_url, expected) in [
            (
                "https://github.com/rust-lang-nursery/rustfmt/releases.atom",
                None,
            ),
            (
                "https://github.com/Rust-Lang-Nursery/RustFmt/releases.atom",
                None,
            ),
            (
                "https://github.com/rust-lang/rustfmt/releases.atom",
                Some("rust-lang/rustfmt"),
            ),
            (
                "https://github.com/rust-lang-nursery/rustfmt-old/tags.atom",
                Some("rust-lang-nursery/rustfmt-old"),
            ),
            ("https://github.com/login?return_to=x", None),
        ] {
            assert_eq!(
                relocation(&gh, final_url).as_deref(),
                expected,
                "{final_url}"
            );
        }
        let pypi = UpstreamSource::PyPI {
            pname: "requests".to_owned(),
        };
        assert_eq!(
            relocation(&pypi, "https://pypi.org/rss/project/other/releases.xml"),
            None
        );
    }

    #[test]
    fn failure_health_cases() {
        use reqwest::StatusCode;
        let status = |code| -> anyhow::Error {
            anyhow::Error::new(HttpStatusError::new("GitHub tags API request", code))
                .context("API poll failed for github/o/r")
        };
        assert_eq!(
            failure_health(&status(StatusCode::NOT_FOUND)),
            SourceHealth::NotFound
        );
        assert_eq!(
            failure_health(&status(StatusCode::GONE)),
            SourceHealth::NotFound
        );
        assert_eq!(
            failure_health(&status(StatusCode::FORBIDDEN)),
            SourceHealth::Error
        );
        assert_eq!(
            failure_health(&anyhow::anyhow!("error sending request")),
            SourceHealth::Error
        );
    }

    #[test]
    fn tag_from_release_link_cases() {
        let cases: &[(&str, Option<&str>)] = &[
            (
                "https://github.com/madler/zlib/releases/tag/v1.3.2",
                Some("v1.3.2"),
            ),
            (
                "https://github.com/o/r/releases/tag/v1.0%2Bbuild.1",
                Some("v1.0+build.1"),
            ),
            ("https://github.com/o/r/releases/tag/", None),
            ("https://pypi.org/project/requests/2.32.3/", None),
            // Invalid escapes are kept verbatim.
            ("https://github.com/o/r/releases/tag/100%zz", Some("100%zz")),
        ];
        for (href, expected) in cases {
            assert_eq!(tag_from_release_link(href).as_deref(), *expected, "{href}");
        }
    }

    #[test]
    fn github_releases_feed_uses_tag_not_release_name() -> anyhow::Result<()> {
        // Trimmed from https://github.com/madler/zlib/releases.atom
        let feed = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xml:lang="en-US">
  <id>tag:github.com,2008:https://github.com/madler/zlib/releases</id>
  <title>Release notes from zlib</title>
  <updated>2026-02-17T00:00:00Z</updated>
  <entry>
    <id>tag:github.com,2008:Repository/1/v1.3.2</id>
    <updated>2026-02-17T00:00:00Z</updated>
    <link rel="alternate" type="text/html" href="https://github.com/madler/zlib/releases/tag/v1.3.2"/>
    <title>zlib 1.3.2</title>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/1/v1.3.1</id>
    <updated>2024-01-22T00:00:00Z</updated>
    <link rel="alternate" type="text/html" href="https://github.com/madler/zlib/releases/tag/v1.3.1"/>
    <title>zlib 1.3.1</title>
  </entry>
</feed>"#;
        let releases = parse_feed_bytes(feed)?;
        let tags: Vec<&str> = releases.iter().map(|r| r.tag_name.as_str()).collect();
        assert_eq!(tags, vec!["v1.3.2", "v1.3.1"]);
        Ok(())
    }

    #[test]
    fn pypi_feed_uses_title() -> anyhow::Result<()> {
        let feed = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>PyPI recent updates for requests</title>
    <link>https://pypi.org/project/requests/</link>
    <description>Recent updates to the Python Package Index for requests</description>
    <item>
      <title>2.32.3</title>
      <link>https://pypi.org/project/requests/2.32.3/</link>
    </item>
  </channel>
</rss>"#;
        let releases = parse_feed_bytes(feed)?;
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag_name, "2.32.3");
        Ok(())
    }
}
