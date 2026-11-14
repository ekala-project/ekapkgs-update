//! Feed polling for upstream release detection.
//!
//! Supports two strategies:
//! - **RSS/Atom feeds** (GitHub releases then tags, PyPI) — parsed with `feed-rs`
//! - **API polling** (GitLab, SourceHut, DirectoryListing) — reuses existing
//!   platform clients via [`UpstreamSource::fetch_all_releases`]

use std::cmp::Ordering;

use anyhow::Context;
use tracing::{debug, warn};

use crate::database::UpstreamIndexEntry;
use crate::vcs_sources::{Release, UpstreamSource, compare_version_components};

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

/// Poll a single upstream source for its releases.
///
/// Uses RSS/Atom feeds when available (GitHub, PyPI) and falls back to API
/// polling for platforms without feed support.
pub async fn poll_source(source: &UpstreamSource) -> anyhow::Result<Vec<Release>> {
    for feed_url in source.feed_urls() {
        match poll_feed(&feed_url).await {
            Ok(releases) if !releases.is_empty() => return Ok(releases),
            Ok(_) => {
                debug!(
                    "{}/{}: feed {} returned no entries",
                    source.source_type(),
                    source.source_key(),
                    feed_url
                );
            },
            Err(e) => {
                debug!(
                    "{}/{}: feed {} failed: {}",
                    source.source_type(),
                    source.source_key(),
                    feed_url,
                    e
                );
            },
        }
    }

    // Fall back to existing API-based fetching
    source.fetch_all_releases().await.with_context(|| {
        format!(
            "API poll failed for {}/{}",
            source.source_type(),
            source.source_key()
        )
    })
}

/// Fetch and parse an RSS or Atom feed, returning releases.
async fn poll_feed(url: &str) -> anyhow::Result<Vec<Release>> {
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

    let body = response
        .bytes()
        .await
        .with_context(|| format!("read feed body from {url}"))?;

    parse_feed_bytes(&body)
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
            let key = upstream_key.strip_prefix('~').unwrap_or(upstream_key);
            let (owner, repo) = key.split_once('/')?;
            Some(UpstreamSource::SourceHut {
                owner: owner.to_owned(),
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
            owner: "sircmpwn".to_owned(),
            repo: "scdoc".to_owned(),
        };
        let reconstructed = upstream_source_from_index(
            source.source_type(),
            &source.source_key(),
            source.instance(),
        )
        .unwrap();
        assert_eq!(reconstructed.source_type(), "sourcehut");
        assert_eq!(reconstructed.source_key(), "~sircmpwn/scdoc");
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
