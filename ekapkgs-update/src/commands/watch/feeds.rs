//! Feed polling for upstream release detection.
//!
//! Supports two strategies:
//! - **RSS/Atom feeds** (GitHub releases, PyPI) — parsed with `feed-rs`
//! - **API polling** (GitLab, SourceHut, DirectoryListing) — reuses existing
//!   platform clients via [`UpstreamSource::fetch_all_releases`]

use anyhow::Context;
use tracing::{debug, warn};

use crate::vcs_sources::{Release, UpstreamSource};

/// Poll a single upstream source for its releases.
///
/// Uses RSS/Atom feeds when available (GitHub, PyPI) and falls back to API
/// polling for platforms without feed support.
pub async fn poll_source(source: &UpstreamSource) -> anyhow::Result<Vec<Release>> {
    if let Some(feed_url) = source.feed_url() {
        match poll_feed(&feed_url).await {
            Ok(releases) if !releases.is_empty() => return Ok(releases),
            Ok(_) => {
                debug!(
                    "{}/{}: RSS feed returned no entries, falling back to API",
                    source.source_type(),
                    source.source_key()
                );
            },
            Err(e) => {
                debug!(
                    "{}/{}: RSS feed failed ({}), falling back to API",
                    source.source_type(),
                    source.source_key(),
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
            // Use the entry title as the tag name. GitHub Atom feeds use the
            // release tag as the title (e.g. "v1.2.3"). PyPI uses the version
            // string directly (e.g. "1.2.3").
            let tag_name = entry.title.map(|t| t.content).or_else(|| {
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
            // upstream_key is the base_url; we need the pname from the URL
            let pname = upstream_key
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(upstream_key);
            Some(UpstreamSource::DirectoryListing {
                base_url: upstream_key.to_owned(),
                pname: pname.to_owned(),
            })
        },
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
            reconstructed.feed_url().unwrap(),
            "https://github.com/BurntSushi/ripgrep/releases.atom"
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
        assert!(reconstructed.feed_url().is_none());
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
            reconstructed.feed_url().unwrap(),
            "https://pypi.org/rss/project/requests/releases.xml"
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
}
