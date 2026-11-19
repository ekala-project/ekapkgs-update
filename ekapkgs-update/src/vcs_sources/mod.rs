//! VCS source abstraction for GitHub, GitLab, and other code hosting platforms

use std::str::FromStr;
use std::sync::OnceLock;
use std::{env, fmt};

use anyhow::Context;
use regex::Regex;
use tokio::process::Command;
use tracing::{debug, info, trace, warn};

mod http_status;
mod version_order;

pub use http_status::HttpStatusError;
pub use version_order::compare_version_components;

use crate::github::{fetch_github_releases, fetch_github_tags, parse_github_url};
use crate::gitlab::{fetch_gitlab_releases, fetch_gitlab_tags, parse_gitlab_url};
use crate::pypi::fetch_pypi_releases;
use crate::sourcehut::{fetch_sourcehut_tags, parse_sourcehut_url};

/// Release information from a VCS source
#[derive(Debug, Clone)]
pub struct Release {
    pub tag_name: String,
    pub is_prerelease: bool,
}

impl Release {
    /// Extract the clean version string from this release.
    ///
    /// Removes common tag prefixes like `v`, `release-`, etc. by delegating to
    /// [`extract_version_from_tag`]. The returned slice is borrowed from
    /// [`Release::tag_name`], so no allocation is performed.
    ///
    /// # Returns
    /// The version substring of [`Release::tag_name`].
    pub fn version(&self) -> &str {
        extract_version_from_tag(&self.tag_name)
    }

    /// Resolve this release's version string, optionally via a regex extractor.
    ///
    /// When `version_regex` is `None`, falls back to [`extract_version_from_tag`]
    /// which strips common tag prefixes.
    ///
    /// # Errors
    /// Returns an error if `version_regex` is `Some` and the regex either fails
    /// to compile or fails to match the tag.
    pub fn resolved_version(&self, version_regex: Option<&str>) -> anyhow::Result<String> {
        match version_regex {
            Some(regex) => extract_version_with_regex(&self.tag_name, regex),
            None => Ok(extract_version_from_tag(&self.tag_name).to_owned()),
        }
    }

    /// Check whether this release passes the version-prefix and semver filters.
    ///
    /// A release matches when all of the following hold:
    /// 1. it is not a prerelease (unless `include_prereleases` is true),
    /// 2. its resolved version starts with `version_prefix` (when supplied),
    /// 3. its resolved version is acceptable per `strategy` relative to `current_version`.
    ///
    /// A regex-extraction failure is treated as "does not match" (logged at
    /// debug, not propagated): the release is simply skipped, mirroring the
    /// previous closure-based behavior.
    pub fn matches(
        &self,
        current_version: &str,
        strategy: SemverStrategy,
        version_prefix: Option<&str>,
        version_regex: Option<&str>,
        include_prereleases: bool,
    ) -> bool {
        if self.is_prerelease && !include_prereleases {
            return false;
        }

        let version = match self.resolved_version(version_regex) {
            Ok(v) => v,
            Err(e) => {
                debug!(
                    "Skipping release {} - version extraction failed: {}",
                    self.tag_name, e
                );
                return false;
            },
        };

        // Detect prereleases from version string even if the API didn't flag them
        if !include_prereleases && version_looks_prerelease(&version) {
            debug!(
                "Skipping release {} - version string contains prerelease indicator",
                version
            );
            return false;
        }

        if let Some(prefix) = version_prefix {
            if !version.starts_with(prefix) {
                debug!(
                    "Skipping release {} - doesn't match version prefix {}",
                    version, prefix
                );
                return false;
            }
        }

        is_version_acceptable(current_version, &version, strategy)
    }
}

/// Semver update strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemverStrategy {
    /// Accept any newer non-prerelease version (current behavior)
    Latest,
    /// Allow major version updates (same as Latest)
    Major,
    /// Only update to latest minor version within the same major version
    Minor,
    /// Only update to latest patch version within the same major.minor version
    Patch,
}

impl FromStr for SemverStrategy {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.to_lowercase().as_str() {
            "latest" => Ok(SemverStrategy::Latest),
            "major" => Ok(SemverStrategy::Major),
            "minor" => Ok(SemverStrategy::Minor),
            "patch" => Ok(SemverStrategy::Patch),
            _ => anyhow::bail!(
                "Invalid semver strategy: '{s}'. Valid options: latest, major, minor, patch"
            ),
        }
    }
}

impl fmt::Display for SemverStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SemverStrategy::Latest => "latest",
            SemverStrategy::Major => "major",
            SemverStrategy::Minor => "minor",
            SemverStrategy::Patch => "patch",
        };
        f.write_str(s)
    }
}

/// Per-package constraints on which upstream release an automated update may
/// target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionPolicy {
    pub strategy: SemverStrategy,
    pub include_prereleases: bool,
    pub version_regex: Option<String>,
}

impl VersionPolicy {
    /// Pick the newest release this policy allows relative to `current_version`.
    ///
    /// Returns the release together with its resolved version string, or
    /// `None` when no release qualifies.
    pub fn best_release(
        &self,
        releases: Vec<Release>,
        current_version: &str,
    ) -> Option<(Release, String)> {
        let regex = self.version_regex.as_deref();
        let release = find_best_release(
            releases,
            current_version,
            self.strategy,
            None,
            None,
            regex,
            self.include_prereleases,
        )
        .ok()?;
        let version = release.resolved_version(regex).ok()?;
        Some((release, version))
    }
}

/// Upstream VCS source (GitHub, GitLab, SourceHut, PyPI, etc.)
#[derive(Debug)]
pub enum UpstreamSource {
    GitHub {
        owner: String,
        repo: String,
    },
    GitLab {
        instance: String,
        owner: String,
        project: String,
    },
    SourceHut {
        owner: String,
        repo: String,
    },
    PyPI {
        pname: String,
    },
    /// Directory listing-based source (GNU FTP, SourceForge, etc.)
    /// `base_url` is the directory listing URL, `pname` is the project name
    /// used to extract versions from filenames like `<pname>-<version>.tar.*`
    DirectoryListing {
        base_url: String,
        pname: String,
    },
    /// Custom script that outputs available versions, one per line to stdout.
    /// The `script_path` is a Nix store path obtained from
    /// `passthru.ekapkgs-update.update-script`.
    Script {
        script_path: String,
    },
}

/// Parse PyPI URL to extract package name
///
/// Matches URLs like:
/// - `https://files.pythonhosted.org/packages/.../package-1.0.0.tar.gz`
/// - `https://pypi.org/project/package/`
/// - `https://pypi.python.org/packages/.../package-1.0.0.tar.gz`
/// - `mirror://pypi/a/azure-mgmt-advisor/azure-mgmt-advisor-9.0.0.zip`
///
/// Returns the package name if found.
///
/// # Failure modes
///
/// All shapes that fail to parse collapse to `None`. The distinct branches
/// (malformed `mirror://pypi/` path, no regex match, heuristic miss on
/// `pythonhosted.org`, completely unrelated URL) are emitted at
/// `tracing::trace` level so the silent failure can be diagnosed without
/// changing the return type.
///
/// The hard-coded PyPI regex is compiled at most once via [`OnceLock`]; a
/// regex-compile bug surfaces as a panic at first call (via `expect`) rather
/// than as a silent `None`.
fn parse_pypi_url(url: &str) -> Option<String> {
    // Match mirror://pypi/{first-letter}/{package-name}/{filename}
    // Format: mirror://pypi/a/azure-mgmt-advisor/azure-mgmt-advisor-9.0.0.zip
    if url.starts_with("mirror://pypi/") {
        let parts: Vec<&str> = url.split('/').collect();
        // Expected format: ["mirror:", "", "pypi", "{letter}", "{package-name}", "{filename}"]
        // Extract package name from the path (4th element, 0-indexed)
        if let Some(package) = parts.get(4) {
            return Some((*package).to_owned());
        }
        trace!(
            "parse_pypi_url: mirror://pypi/ URL too short (got {} segments): {url}",
            parts.len()
        );
    }

    // Match pypi.org/project/{pname}
    static PYPI_PROJECT_REGEX: OnceLock<Regex> = OnceLock::new();
    let pypi_project_regex = PYPI_PROJECT_REGEX.get_or_init(|| {
        Regex::new(r"pypi\.(?:python\.)?org/project/([^/]+)")
            .expect("hard-coded PyPI project regex must compile")
    });
    if let Some(caps) = pypi_project_regex.captures(url) {
        return caps.get(1).map(|m| m.as_str().to_owned());
    }

    // Match files.pythonhosted.org or pypi.python.org packages
    // URL format: https://files.pythonhosted.org/packages/hash/hash/package-version.tar.gz
    if url.contains("pythonhosted.org") || url.contains("pypi.python.org") {
        // Extract filename from URL
        if let Some(filename) = url.split('/').next_back() {
            // Remove file extension and version suffix to get package name
            // This is a heuristic and may not work for all cases
            if let Some(name_with_version) = filename.split('.').next() {
                // Try to extract package name by removing version suffix
                // Common pattern: package-name-1.0.0
                if let Some(idx) = name_with_version.rfind('-') {
                    let potential_name = &name_with_version[..idx];
                    // Check if what follows looks like a version (starts with digit)
                    if name_with_version[idx + 1..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_digit())
                    {
                        return Some(potential_name.to_owned());
                    }
                    trace!(
                        "parse_pypi_url: pythonhosted heuristic: text after '-' is not a \
                         version-like token in {filename}"
                    );
                } else {
                    trace!(
                        "parse_pypi_url: pythonhosted heuristic: no '-' separator in {filename}"
                    );
                }
            } else {
                trace!("parse_pypi_url: pythonhosted heuristic: empty filename component in {url}");
            }
        } else {
            trace!("parse_pypi_url: pythonhosted heuristic: URL has no path segments: {url}");
        }
    } else {
        trace!("parse_pypi_url: URL does not match any PyPI shape: {url}");
    }

    None
}

impl UpstreamSource {
    /// The repository name a fetcher for this upstream binds as `repo`, for
    /// upstreams whose releases are git tags (GitHub, GitLab, SourceHut).
    ///
    /// Other upstreams' `Release::tag_name` is a bare version, not a tag.
    pub fn git_repo_name(&self) -> Option<&str> {
        match self {
            UpstreamSource::GitHub { repo, .. } | UpstreamSource::SourceHut { repo, .. } => {
                Some(repo)
            },
            UpstreamSource::GitLab { project, .. } => project.rsplit('/').next(),
            _ => None,
        }
    }

    /// Parse a URL and return the appropriate UpstreamSource
    ///
    /// Tries to parse the URL as GitHub first, then GitLab, then SourceHut, then PyPI.
    ///
    /// # Arguments
    /// * `url` - Source URL to parse
    ///
    /// # Returns
    /// `Some(UpstreamSource)` if the URL matches a known VCS platform, `None` otherwise
    pub fn from_url(url: &str) -> Option<Self> {
        // Normalize mirror:// URLs to their real hosts before parsing.
        // mirror://github/owner/repo/... → https://github.com/owner/repo/...
        let normalized = if let Some(rest) = url.strip_prefix("mirror://github/") {
            format!("https://github.com/{rest}")
        } else {
            url.to_owned()
        };
        let url = &normalized;

        // Handle mirror://gnu/<pkg>/... and mirror://sourceforge/<pkg>/...
        // These map to directory listings where we scrape tarball filenames.
        if let Some(rest) = url.strip_prefix("mirror://gnu/") {
            // rest = "autoconf/autoconf-2.73.tar.xz" → pname = "autoconf"
            let pname = rest.split('/').next()?;
            return Some(UpstreamSource::DirectoryListing {
                base_url: format!("https://ftp.gnu.org/gnu/{pname}/"),
                pname: pname.to_owned(),
            });
        }
        if let Some(rest) = url.strip_prefix("mirror://sourceforge/") {
            // Also seen: mirror://sourceforge/project/<pkg>/... and
            // mirror://sourceforge//<pkg>/...
            let mut segments = rest.split('/').filter(|s| !s.is_empty()).peekable();
            if segments
                .peek()
                .is_some_and(|s| *s == "project" || *s == "projects")
            {
                segments.next();
            }
            let pname = segments.next()?;
            return Some(UpstreamSource::DirectoryListing {
                base_url: format!("https://sourceforge.net/projects/{pname}/files/"),
                pname: pname.to_owned(),
            });
        }

        if let Some(github_repo) = parse_github_url(url) {
            return Some(UpstreamSource::GitHub {
                owner: github_repo.owner,
                repo: github_repo.repo,
            });
        }
        if let Some(gitlab_project) = parse_gitlab_url(url) {
            return Some(UpstreamSource::GitLab {
                instance: gitlab_project.instance,
                owner: gitlab_project.owner,
                project: gitlab_project.project,
            });
        }
        if let Some(sourcehut_repo) = parse_sourcehut_url(url) {
            return Some(UpstreamSource::SourceHut {
                owner: sourcehut_repo.owner,
                repo: sourcehut_repo.repo,
            });
        }
        parse_pypi_url(url).map(|pypi_pname| UpstreamSource::PyPI { pname: pypi_pname })
    }

    /// Fetch all releases/tags from the upstream VCS platform without filtering
    ///
    /// Handles authentication tokens and release-to-tags fallback for each
    /// platform. Returns the full unfiltered list of releases.
    ///
    /// # Errors
    /// Returns an error if the API request fails.
    pub async fn fetch_all_releases(&self) -> anyhow::Result<Vec<Release>> {
        match self {
            UpstreamSource::GitHub { owner, repo } => {
                let token = crate::github::auth::read_token().await;

                if token.is_none() {
                    warn!(
                        "No GITHUB_TOKEN or GitHub App - using unauthenticated GitHub API (60 \
                         requests/hour rate limit)"
                    );
                }

                let all_releases = fetch_github_releases(owner, repo, token.as_deref()).await;
                if let Err(e) = &all_releases {
                    debug!("GitHub releases endpoint failed, falling back to tags: {e}");
                }

                match all_releases {
                    Ok(gh_releases) if !gh_releases.is_empty() => Ok(gh_releases
                        .into_iter()
                        .map(|r| Release {
                            tag_name: r.tag_name,
                            is_prerelease: r.prerelease,
                        })
                        .collect()),
                    Ok(_) => {
                        debug!("GitHub releases endpoint returned empty, falling back to tags");
                        Ok(fetch_github_tags(owner, repo, token.as_deref())
                            .await?
                            .into_iter()
                            .map(|t| Release {
                                tag_name: t.name,
                                is_prerelease: false,
                            })
                            .collect())
                    },
                    Err(_) => Ok(fetch_github_tags(owner, repo, token.as_deref())
                        .await?
                        .into_iter()
                        .map(|t| Release {
                            tag_name: t.name,
                            is_prerelease: false,
                        })
                        .collect()),
                }
            },
            UpstreamSource::GitLab {
                instance,
                owner,
                project,
            } => {
                let token = env::var("GITLAB_TOKEN").ok();

                if token.is_none() {
                    warn!(
                        "GITLAB_TOKEN not set - using unauthenticated GitLab API (~300 \
                         requests/hour rate limit)"
                    );
                }

                let all_releases =
                    fetch_gitlab_releases(instance, owner, project, token.as_deref()).await;
                if let Err(e) = &all_releases {
                    debug!("GitLab releases endpoint failed, falling back to tags: {e}");
                }

                match all_releases {
                    Ok(gl_releases) => Ok(gl_releases
                        .into_iter()
                        .map(|r| Release {
                            tag_name: r.tag_name,
                            is_prerelease: r.upcoming_release,
                        })
                        .collect()),
                    Err(_) => Ok(
                        fetch_gitlab_tags(instance, owner, project, token.as_deref())
                            .await?
                            .into_iter()
                            .map(|t| Release {
                                tag_name: t.name,
                                is_prerelease: false,
                            })
                            .collect(),
                    ),
                }
            },
            UpstreamSource::SourceHut { owner, repo } => {
                let token = env::var("SOURCEHUT_TOKEN").ok();

                if token.is_none() {
                    warn!(
                        "SOURCEHUT_TOKEN not set - using unauthenticated SourceHut API (limited \
                         rate limit)"
                    );
                }

                let tags = fetch_sourcehut_tags(owner, repo, token.as_deref()).await?;
                Ok(tags
                    .into_iter()
                    .map(|t| Release {
                        tag_name: t.name,
                        is_prerelease: false,
                    })
                    .collect())
            },
            UpstreamSource::PyPI { pname } => {
                let pypi_response = fetch_pypi_releases(pname).await?;
                Ok(pypi_response
                    .releases
                    .into_iter()
                    .map(|(version, artifacts)| Release {
                        tag_name: version,
                        is_prerelease: artifacts.iter().any(|a| a.yanked),
                    })
                    .collect())
            },
            UpstreamSource::DirectoryListing { base_url, pname } => {
                fetch_directory_listing_releases(base_url, pname).await
            },
            UpstreamSource::Script { script_path } => fetch_script_releases(script_path).await,
        }
    }

    /// Get the best compatible release based on semver strategy
    ///
    /// Fetches all releases/tags from the VCS platform and filters them based on
    /// the semver strategy to find the best match for the current version.
    ///
    /// # Arguments
    /// * `current_version` - The current version to compare against
    /// * `strategy` - The semver update strategy to apply
    /// * `version_prefix` - Optional version prefix to filter releases (e.g., "1.2" for v1_2
    ///   variants)
    ///
    /// # Returns
    /// The best compatible release information
    ///
    /// # Errors
    /// Returns an error if the API request fails or no compatible releases are found
    pub async fn get_compatible_release(
        &self,
        current_version: &str,
        strategy: SemverStrategy,
        version_prefix: Option<&str>,
        explicit_version: Option<&str>,
        version_regex: Option<&str>,
        include_prereleases: bool,
    ) -> anyhow::Result<Release> {
        let releases = self.fetch_all_releases().await?;
        let result = find_best_release(
            releases,
            current_version,
            strategy,
            version_prefix,
            explicit_version,
            version_regex,
            include_prereleases,
        );
        let Some(target) = explicit_version else {
            return result;
        };
        let Err(err) = result else {
            return result;
        };

        // A tag is often pushed some time before its release is published.
        // Watch sees it earlier (GitHub's `releases.atom` lists bare tags), so
        // the explicit target can be a tag the releases endpoint doesn't know
        // yet. Look it up among the tags before giving up.
        let tags = match self.fetch_tag_releases().await {
            Ok(Some(tags)) => tags,
            Ok(None) => return Err(err),
            Err(tag_err) => {
                debug!("Tag lookup for explicit version {target} failed: {tag_err:#}");
                return Err(err);
            },
        };
        match find_explicit_release(tags, target, version_regex) {
            Some(release) => {
                info!(
                    "Version {target} has no release yet; using tag {}",
                    release.tag_name
                );
                Ok(release)
            },
            None => Err(err),
        }
    }

    /// Fetch the repository's tags as releases, for platforms whose releases
    /// are a subset of their tags (GitHub, GitLab).
    ///
    /// Returns `None` for upstreams without a separate tags endpoint (their
    /// [`fetch_all_releases`](Self::fetch_all_releases) already lists tags,
    /// or they have no tags at all).
    async fn fetch_tag_releases(&self) -> anyhow::Result<Option<Vec<Release>>> {
        let tags: Vec<String> = match self {
            UpstreamSource::GitHub { owner, repo } => {
                let token = crate::github::auth::read_token().await;
                fetch_github_tags(owner, repo, token.as_deref())
                    .await?
                    .into_iter()
                    .map(|t| t.name)
                    .collect()
            },
            UpstreamSource::GitLab {
                instance,
                owner,
                project,
            } => {
                let token = env::var("GITLAB_TOKEN").ok();
                fetch_gitlab_tags(instance, owner, project, token.as_deref())
                    .await?
                    .into_iter()
                    .map(|t| t.name)
                    .collect()
            },
            UpstreamSource::SourceHut { .. }
            | UpstreamSource::PyPI { .. }
            | UpstreamSource::DirectoryListing { .. }
            | UpstreamSource::Script { .. } => return Ok(None),
        };
        Ok(Some(
            tags.into_iter()
                .map(|tag_name| Release {
                    tag_name,
                    is_prerelease: false,
                })
                .collect(),
        ))
    }

    /// Get a human-readable description of this source
    pub fn description(&self) -> String {
        match self {
            UpstreamSource::GitHub { owner, repo } => format!("GitHub repo: {owner}/{repo}"),
            UpstreamSource::GitLab {
                instance,
                owner,
                project,
            } => {
                format!("GitLab project: {instance}/{owner}/{project}")
            },
            UpstreamSource::SourceHut { owner, repo } => {
                format!("SourceHut repo: {owner}/{repo}")
            },
            UpstreamSource::PyPI { pname } => format!("PyPI package: {pname}"),
            UpstreamSource::DirectoryListing { base_url, pname } => {
                format!("Directory listing: {pname} ({base_url})")
            },
            UpstreamSource::Script { script_path } => {
                format!("Custom script: {script_path}")
            },
        }
    }

    /// Returns a canonical type string for database storage and feed routing.
    pub fn source_type(&self) -> &'static str {
        match self {
            Self::GitHub { .. } => "github",
            Self::GitLab { .. } => "gitlab",
            Self::SourceHut { .. } => "sourcehut",
            Self::PyPI { .. } => "pypi",
            Self::DirectoryListing { .. } => "directory",
            Self::Script { .. } => "script",
        }
    }

    /// Returns a canonical key string for database lookups.
    ///
    /// - GitHub: `"owner/repo"`
    /// - GitLab: `"owner/project"`
    /// - SourceHut: `"~owner/repo"`
    /// - PyPI: `"pname"`
    /// - DirectoryListing: the `base_url`
    pub fn source_key(&self) -> String {
        match self {
            Self::GitHub { owner, repo } => format!("{owner}/{repo}"),
            Self::GitLab { owner, project, .. } => format!("{owner}/{project}"),
            // `owner` keeps the `~` of the URL (`parse_sourcehut_url`).
            Self::SourceHut { owner, repo } => {
                format!("~{}/{repo}", owner.trim_start_matches('~'))
            },
            Self::PyPI { pname } => pname.clone(),
            Self::DirectoryListing { base_url, .. } => base_url.clone(),
            Self::Script { script_path } => script_path.clone(),
        }
    }

    /// Returns the hosting instance, if applicable.
    ///
    /// Only meaningful for GitLab (e.g. `"gitlab.freedesktop.org"`).
    /// Returns `None` for all other source types.
    pub fn instance(&self) -> Option<&str> {
        match self {
            Self::GitLab { instance, .. } => Some(instance),
            Self::GitHub { .. }
            | Self::SourceHut { .. }
            | Self::PyPI { .. }
            | Self::DirectoryListing { .. }
            | Self::Script { .. } => None,
        }
    }

    /// RSS/Atom feed URLs the platform provides natively, in the order they
    /// should be tried (the first one with entries wins).
    ///
    /// - GitHub: `releases.atom`, then `tags.atom` — mirroring the API path,
    ///   which falls back to tags for projects that don't publish releases.
    ///   Without the tags feed, every tags-only project would hit the REST API
    ///   on every poll and exhaust the rate limit.
    /// - PyPI: `https://pypi.org/rss/project/{pname}/releases.xml`
    /// - Others: empty (use API polling instead)
    pub fn feed_urls(&self) -> Vec<String> {
        match self {
            Self::GitHub { owner, repo } => vec![
                format!("https://github.com/{owner}/{repo}/releases.atom"),
                format!("https://github.com/{owner}/{repo}/tags.atom"),
            ],
            Self::PyPI { pname } => {
                vec![format!("https://pypi.org/rss/project/{pname}/releases.xml")]
            },
            Self::GitLab { .. }
            | Self::SourceHut { .. }
            | Self::DirectoryListing { .. }
            | Self::Script { .. } => Vec::new(),
        }
    }
}

/// Find the release whose tag name or resolved version is `target_version`.
///
/// A tag `version_regex` doesn't match (e.g. an unrelated `vscode-v0.2.0` tag
/// next to the project's own) is not a match.
pub fn find_explicit_release(
    releases: Vec<Release>,
    target_version: &str,
    version_regex: Option<&str>,
) -> Option<Release> {
    releases.into_iter().find(|release| {
        release.tag_name == target_version
            || release
                .resolved_version(version_regex)
                .is_ok_and(|version| version == target_version)
    })
}

/// Find the best compatible release from a list based on semver strategy
///
/// Filters releases by:
/// 1. Excluding prereleases
/// 2. Filtering by version prefix if provided (e.g., only "1.2.*" releases)
/// 3. Checking version compatibility with strategy
/// 4. Returns the newest compatible version
///
/// # Arguments
/// * `releases` - List of releases to filter
/// * `current_version` - Current version to compare against
/// * `strategy` - Semver strategy to apply
/// * `version_prefix` - Optional version prefix to filter releases (e.g., "1.2" for v1_2 variants)
/// * `include_prereleases` - Whether to include prerelease versions
///
/// # Returns
/// The best matching release
///
/// # Errors
/// Returns an error if no compatible releases are found
pub fn find_best_release(
    releases: Vec<Release>,
    current_version: &str,
    strategy: SemverStrategy,
    version_prefix: Option<&str>,
    explicit_version: Option<&str>,
    version_regex: Option<&str>,
    include_prereleases: bool,
) -> anyhow::Result<Release> {
    // If explicit version is specified, find and return that exact version.
    // We consume the vector so the matched release is moved out instead of cloned.
    if let Some(target_version) = explicit_version {
        return find_explicit_release(releases, target_version, version_regex).ok_or_else(|| {
            anyhow::anyhow!("Explicit version '{target_version}' not found in available releases")
        });
    }

    // Sort all releases by version descending (newest first) so we can
    // short-circuit as soon as we find the first compatible release.
    // The sort key is the version the filters see: with a `version_regex`,
    // the captured version (tags it doesn't match sort by their prefix-stripped
    // version; they are rejected by the filter anyway).
    let sort_key = |r: &Release| {
        r.resolved_version(version_regex)
            .unwrap_or_else(|_| r.version().to_owned())
    };
    let mut releases: Vec<(String, Release)> =
        releases.into_iter().map(|r| (sort_key(&r), r)).collect();
    releases.sort_by(|(a, _), (b, _)| version_cmp_desc(a, b));

    // Return the first (newest) release that passes all filters.
    releases
        .into_iter()
        .map(|(_, r)| r)
        .find(|r| {
            r.matches(
                current_version,
                strategy,
                version_prefix,
                version_regex,
                include_prereleases,
            )
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No compatible releases found for version {current_version} with strategy \
                 {strategy}"
            )
        })
}

/// Compare two version strings descendingly (newest first), using
/// [`compare_version_components`] (a total order, as `sort_by` requires).
fn version_cmp_desc(a: &str, b: &str) -> std::cmp::Ordering {
    compare_version_components(b, a)
}

/// Extract version from tag name by pruning leading non-numerical characters
/// and truncating '-unstable' suffixes
///
/// Removes all leading non-numerical characters from tag names to extract the version,
/// and truncates everything from '-unstable' onwards if present.
/// This handles various tag naming conventions like "v1.0.0", "release-1.0.0", "version-2.3.4",
/// etc., as well as unstable versions like "1.2.3-unstable-2024-01-01".
///
/// # Arguments
/// * `tag` - The tag name to extract version from
///
/// # Returns
/// The version string with leading non-numerical characters removed and '-unstable' suffix
/// truncated
///
/// # Example
/// ```
/// use ekapkgs_update::vcs_sources::extract_version_from_tag;
///
/// assert_eq!(extract_version_from_tag("v1.0.0"), "1.0.0");
/// assert_eq!(extract_version_from_tag("release-2.3.4"), "2.3.4");
/// assert_eq!(extract_version_from_tag("version-1.2.3"), "1.2.3");
/// assert_eq!(extract_version_from_tag("1.0.0"), "1.0.0");
/// assert_eq!(extract_version_from_tag("lz4-1.9.4"), "1.9.4");
/// assert_eq!(extract_version_from_tag("1.2.3-unstable"), "1.2.3");
/// assert_eq!(
///     extract_version_from_tag("v2.0.0-unstable-2024-01-01"),
///     "2.0.0"
/// );
/// ```
pub fn extract_version_from_tag(tag: &str) -> &str {
    // Find the first digit in the tag
    let Some(pos) = tag.find(|c: char| c.is_ascii_digit()) else {
        // If no digit found, return the original tag
        return tag;
    };
    let mut version = &tag[pos..];

    // A name ending in digits (`lz4-1.9.4`, `gtk4-4.12.0`, `py3_2.0`): the
    // first digit belongs to the name when it follows at least two letters
    // and its number is followed by `-`/`_` and another number.
    let letters_before = tag[..pos]
        .bytes()
        .rev()
        .take_while(u8::is_ascii_alphabetic)
        .count();
    if letters_before >= 2 {
        let digits = version.bytes().take_while(u8::is_ascii_digit).count();
        let rest = &version.as_bytes()[digits..];
        if rest.len() > 1 && matches!(rest[0], b'-' | b'_') && rest[1].is_ascii_digit() {
            version = &version[digits + 1..];
        }
    }

    // Truncate '-unstable' suffix if present
    if let Some(unstable_pos) = version.find("-unstable") {
        &version[..unstable_pos]
    } else {
        version
    }
}

/// Extract version from tag name using a custom regex pattern
///
/// Uses a provided regular expression to extract the version from a tag name.
/// The regex should contain exactly one capture group that captures the version.
///
/// # Arguments
/// * `tag` - The tag name to extract version from
/// * `regex_pattern` - The regex pattern with one capture group for the version
///
/// # Returns
/// The extracted version string from the first capture group
///
/// # Errors
/// Returns an error if the regex pattern is invalid or doesn't match the tag
///
/// # Example
/// ```
/// # use ekapkgs_update::vcs_sources::extract_version_with_regex;
/// # fn main() -> anyhow::Result<()> {
/// assert_eq!(extract_version_with_regex("jq-1.6", r"jq-(.*)")?, "1.6");
/// assert_eq!(
///     extract_version_with_regex("release/v2.3.4", r"release/v(.*)")?,
///     "2.3.4"
/// );
/// # Ok(())
/// # }
/// ```
pub fn extract_version_with_regex(tag: &str, regex_pattern: &str) -> anyhow::Result<String> {
    use regex::Regex;

    let re = Regex::new(regex_pattern)
        .with_context(|| format!("Invalid version regex pattern: {regex_pattern}"))?;

    let captures = re
        .captures(tag)
        .with_context(|| format!("Version regex '{regex_pattern}' did not match tag '{tag}'"))?;

    // Get the first capture group (index 1, since 0 is the whole match)
    let version = captures
        .get(1)
        .with_context(|| {
            format!("Version regex '{regex_pattern}' must have exactly one capture group")
        })?
        .as_str()
        .to_owned();

    Ok(version)
}

/// Check whether a version string contains common prerelease indicators
/// (alpha, beta, rc, dev, pre) that the upstream API may not have flagged.
pub fn version_looks_prerelease(version: &str) -> bool {
    let lower = version.to_lowercase();
    // Check for common prerelease keywords
    if lower.contains("alpha")
        || lower.contains("beta")
        || lower.contains("dev")
        || lower.contains("preview")
    {
        return true;
    }
    // Check for "rc" or "pre" as a distinct segment: either after a dash/dot,
    // or directly after digits (e.g., "4.7.0rc1", "1.0.0-rc.2")
    if lower.contains("-rc")
        || lower.contains(".rc")
        || lower.contains("-pre")
        || lower.contains(".pre")
    {
        return true;
    }
    // Detect embedded rc/pre without separator: "1.2.3rc1", "1.2.3pre4"
    // Match digit followed by "rc" or "pre" followed by digit
    for pattern in &["rc", "pre"] {
        if let Some(pos) = lower.find(pattern) {
            if pos > 0 && lower.as_bytes()[pos - 1].is_ascii_digit() {
                return true;
            }
        }
    }
    false
}

/// Check if a new version is acceptable based on the semver strategy
///
/// # Arguments
/// * `current` - Current version string
/// * `new` - New version string
/// * `strategy` - The semver update strategy to apply
///
/// # Returns
/// `true` if the new version satisfies the strategy constraints, `false` otherwise.
///
/// Versions are compared with [`compare_version_components`], so prereleases
/// (`1.0.0rc1`) rank below their release and 4-component or date versions
/// compare numerically. `Minor`/`Patch` require the first one/two
/// dot-separated numbers to match (`2024.01.15 -> 2024.02.01` is a minor
/// update). Dot-less versions (`137`, `20240115`, `2024-01-15`) are accepted
/// under `Latest`/`Major` only, and never across schemes: a dotted version
/// against a dot-less one, or a dot-less date against a dot-less counter.
///
/// # Examples
/// ```
/// use ekapkgs_update::vcs_sources::{SemverStrategy, is_version_acceptable};
///
/// // Latest strategy accepts any newer version
/// assert!(is_version_acceptable(
///     "1.0.0",
///     "2.0.0",
///     SemverStrategy::Latest
/// ));
/// assert!(is_version_acceptable(
///     "1.0.0",
///     "1.1.0",
///     SemverStrategy::Latest
/// ));
///
/// // Minor strategy only accepts same major version
/// assert!(is_version_acceptable(
///     "1.0.0",
///     "1.1.0",
///     SemverStrategy::Minor
/// ));
/// assert!(!is_version_acceptable(
///     "1.0.0",
///     "2.0.0",
///     SemverStrategy::Minor
/// ));
///
/// // Patch strategy only accepts same major.minor version
/// assert!(is_version_acceptable(
///     "1.0.0",
///     "1.0.1",
///     SemverStrategy::Patch
/// ));
/// assert!(!is_version_acceptable(
///     "1.0.0",
///     "1.1.0",
///     SemverStrategy::Patch
/// ));
/// ```
pub fn is_version_acceptable(current: &str, new: &str, strategy: SemverStrategy) -> bool {
    use std::cmp::Ordering;

    // Strip common prefixes like 'v' or 'version-'
    let clean_current = current
        .trim_start_matches('v')
        .trim_start_matches("version-");
    let clean_new = new.trim_start_matches('v').trim_start_matches("version-");

    let starts_with_digit = |v: &str| v.starts_with(|c: char| c.is_ascii_digit());
    if !starts_with_digit(clean_current) || !starts_with_digit(clean_new) {
        debug!(
            "Rejecting non-numeric version (current: {}, new: {})",
            clean_current, clean_new
        );
        return false;
    }

    // A dotted version (`1.2`) and a dot-less one (`137`, `20240115`,
    // `2024-01-15`) are different schemes: `1.13.1 -> 120715` is a stray tag,
    // not an update. Likewise a dot-less date and a dot-less counter.
    let dotted = |v: &str| v.split('-').next().unwrap_or(v).contains('.');
    let single = !dotted(clean_current);
    if single != !dotted(clean_new)
        || (single && is_date_like(clean_current) != is_date_like(clean_new))
    {
        debug!(
            "Rejecting version scheme change (current: {}, new: {})",
            clean_current, clean_new
        );
        return false;
    }

    match compare_version_components(clean_new, clean_current) {
        Ordering::Greater => {},
        Ordering::Equal => {
            debug!("Rejecting same version: {} -> {} (no change)", current, new);
            return false;
        },
        Ordering::Less => {
            debug!(
                "Rejecting downgrade: {} -> {} (current version is newer)",
                current, new
            );
            return false;
        },
    }

    // How many leading components must match.
    let fixed = match strategy {
        SemverStrategy::Latest | SemverStrategy::Major => return true,
        SemverStrategy::Minor => 1,
        SemverStrategy::Patch => 2,
    };
    if single {
        debug!(
            "Version '{}' has no minor component, cannot apply {} strategy. Skipping update.",
            clean_current, strategy
        );
        return false;
    }
    match (
        leading_components(clean_current, fixed),
        leading_components(clean_new, fixed),
    ) {
        (Some(c), Some(n)) => c == n,
        _ => {
            debug!(
                "Version '{}' or '{}' has non-numeric leading components, cannot apply {} \
                 strategy",
                clean_current, clean_new, strategy
            );
            false
        },
    }
}

/// The first `count` dot-separated components of `version` (before any
/// `-` suffix) as numbers, missing ones as `0` (`1.9` → `[1, 9]`,
/// `2024.01.15` → `[2024, 1]`, `4.7.0rc1` → `[4, 7]`). `None` when one of
/// them doesn't start with a number.
fn leading_components(version: &str, count: usize) -> Option<Vec<u64>> {
    let base = version.split('-').next().unwrap_or(version);
    let mut parts = base.split('.');
    (0..count)
        .map(|_| match parts.next() {
            None => Some(0),
            Some(part) => {
                let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().ok()
            },
        })
        .collect()
}

/// Whether a version starts with a date: a year (`2024`, `2024-01-15`,
/// `2024.01`) or `YYYYMMDD` (`20240115`).
fn is_date_like(version: &str) -> bool {
    let digits: String = version.chars().take_while(char::is_ascii_digit).collect();
    let year_like = |y: &str| y.parse::<u32>().is_ok_and(|y| (1970..2100).contains(&y));
    match digits.len() {
        4 => year_like(&digits),
        8 => year_like(&digits[..4]),
        _ => false,
    }
}

/// Fetch release versions by running a custom script.
///
/// Executes the script at `script_path` and parses stdout as one version per
/// line. Empty lines are skipped. The script is expected to exit 0 on success.
///
/// # Errors
/// Returns an error if the script fails to execute or exits with a non-zero
/// status code.
async fn fetch_script_releases(script_path: &str) -> anyhow::Result<Vec<Release>> {
    info!("Running update-script: {}", script_path);

    let output = Command::new(script_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .with_context(|| format!("Failed to execute update-script: {script_path}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "update-script exited with status {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let releases: Vec<Release> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|version| Release {
            tag_name: version.to_owned(),
            is_prerelease: version_looks_prerelease(version),
        })
        .collect();

    debug!(
        "update-script produced {} releases from {}",
        releases.len(),
        script_path
    );

    Ok(releases)
}

/// Fetch release versions from an HTTP directory listing.
///
/// Scrapes an HTML directory listing page for tarball filenames matching
/// `<pname>-<version>.tar.*` and returns them as `Release` objects.
async fn fetch_directory_listing_releases(
    base_url: &str,
    pname: &str,
) -> anyhow::Result<Vec<Release>> {
    use regex::Regex;

    let client = reqwest::Client::new();
    let response = client
        .get(base_url)
        .header("User-Agent", "ekapkgs-update")
        .send()
        .await
        .with_context(|| format!("GET {base_url}"))?;

    if !response.status().is_success() {
        return Err(HttpStatusError::new("Directory listing request", response.status()).into());
    }

    let body = response.text().await?;

    // Match tarball filenames: <pname>-<version>.tar.{gz,xz,bz2,lz,zst}
    let pattern = format!(
        r"(?i){}-([0-9][0-9a-zA-Z._-]*)\.tar\.(?:gz|xz|bz2|lz|zst)",
        regex::escape(pname)
    );
    let re = Regex::new(&pattern)?;

    let mut seen = std::collections::HashSet::new();
    let mut releases = Vec::new();

    for caps in re.captures_iter(&body) {
        let version = caps.get(1).unwrap().as_str().to_owned();
        if seen.insert(version.clone()) {
            releases.push(Release {
                tag_name: version,
                is_prerelease: false,
            });
        }
    }

    debug!(
        "Found {} releases for {} from {}",
        releases.len(),
        pname,
        base_url
    );

    Ok(releases)
}

#[cfg(test)]
mod tests;
