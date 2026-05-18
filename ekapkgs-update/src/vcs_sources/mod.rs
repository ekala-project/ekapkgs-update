//! VCS source abstraction for GitHub, GitLab, and other code hosting platforms

use std::str::FromStr;
use std::sync::OnceLock;
use std::{env, fmt};

use anyhow::Context;
use regex::Regex;
use semver::Version;
use tracing::{debug, trace, warn};

use crate::github::{fetch_github_releases, fetch_github_tags, parse_github_url};
use crate::gitlab::{fetch_gitlab_releases, fetch_gitlab_tags, parse_gitlab_url};
use crate::pypi::fetch_pypi_releases;
use crate::sourcehut::{fetch_sourcehut_tags, parse_sourcehut_url};

/// Release information from a VCS source
#[derive(Debug)]
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

/// Upstream VCS source (GitHub, GitLab, SourceHut, PyPI, etc.)
#[derive(Debug)]
pub enum UpstreamSource {
    GitHub { owner: String, repo: String },
    GitLab { owner: String, project: String },
    SourceHut { owner: String, repo: String },
    PyPI { pname: String },
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
        if let Some(github_repo) = parse_github_url(url) {
            return Some(UpstreamSource::GitHub {
                owner: github_repo.owner,
                repo: github_repo.repo,
            });
        }
        if let Some(gitlab_project) = parse_gitlab_url(url) {
            return Some(UpstreamSource::GitLab {
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

    /// Get the best compatible release based on semver strategy
    ///
    /// Fetches all releases/tags from the VCS platform and filters them based on
    /// the semver strategy to find the best match for the current version.
    /// Automatically checks for authentication tokens in environment variables:
    /// - `GITHUB_TOKEN` for GitHub sources
    /// - `GITLAB_TOKEN` for GitLab sources
    /// - `SOURCEHUT_TOKEN` for SourceHut sources
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
        match self {
            UpstreamSource::GitHub { owner, repo } => {
                let token = env::var("GITHUB_TOKEN").ok();

                if token.is_none() {
                    warn!(
                        "GITHUB_TOKEN not set - using unauthenticated GitHub API (60 \
                         requests/hour rate limit)"
                    );
                }

                // Try to fetch all releases first; fall back to tags on failure.
                let all_releases = fetch_github_releases(owner, repo, token.as_deref()).await;
                if let Err(e) = &all_releases {
                    debug!("GitHub releases endpoint failed, falling back to tags: {e}");
                }

                let releases: Vec<Release> = match all_releases {
                    Ok(gh_releases) => gh_releases
                        .into_iter()
                        .map(|r| Release {
                            tag_name: r.tag_name,
                            is_prerelease: r.prerelease,
                        })
                        .collect(),
                    Err(_) => fetch_github_tags(owner, repo, token.as_deref())
                        .await?
                        .into_iter()
                        .map(|t| Release {
                            tag_name: t.name,
                            is_prerelease: false,
                        })
                        .collect(),
                };

                // Filter and find best match
                find_best_release(
                    releases,
                    current_version,
                    strategy,
                    version_prefix,
                    explicit_version,
                    version_regex,
                    include_prereleases,
                )
            },
            UpstreamSource::GitLab { owner, project } => {
                let token = env::var("GITLAB_TOKEN").ok();

                if token.is_none() {
                    warn!(
                        "GITLAB_TOKEN not set - using unauthenticated GitLab API (~300 \
                         requests/hour rate limit)"
                    );
                }

                // Try to fetch all releases first; fall back to tags on failure.
                let all_releases = fetch_gitlab_releases(owner, project, token.as_deref()).await;
                if let Err(e) = &all_releases {
                    debug!("GitLab releases endpoint failed, falling back to tags: {e}");
                }

                let releases: Vec<Release> = match all_releases {
                    Ok(gl_releases) => gl_releases
                        .into_iter()
                        .map(|r| Release {
                            tag_name: r.tag_name,
                            is_prerelease: r.upcoming_release,
                        })
                        .collect(),
                    Err(_) => fetch_gitlab_tags(owner, project, token.as_deref())
                        .await?
                        .into_iter()
                        .map(|t| Release {
                            tag_name: t.name,
                            is_prerelease: false,
                        })
                        .collect(),
                };

                // Filter and find best match
                find_best_release(
                    releases,
                    current_version,
                    strategy,
                    version_prefix,
                    explicit_version,
                    version_regex,
                    include_prereleases,
                )
            },
            UpstreamSource::SourceHut { owner, repo } => {
                let token = env::var("SOURCEHUT_TOKEN").ok();

                if token.is_none() {
                    warn!(
                        "SOURCEHUT_TOKEN not set - using unauthenticated SourceHut API (limited \
                         rate limit)"
                    );
                }

                // SourceHut uses tags via GraphQL API
                let tags = fetch_sourcehut_tags(owner, repo, token.as_deref()).await?;

                let releases: Vec<Release> = tags
                    .into_iter()
                    .map(|t| Release {
                        tag_name: t.name,
                        is_prerelease: false,
                    })
                    .collect();

                // Filter and find best match
                find_best_release(
                    releases,
                    current_version,
                    strategy,
                    version_prefix,
                    explicit_version,
                    version_regex,
                    include_prereleases,
                )
            },
            UpstreamSource::PyPI { pname } => {
                // PyPI doesn't require authentication tokens
                let pypi_response = fetch_pypi_releases(pname).await?;

                // Convert PyPI releases to our Release struct
                // PyPI returns a HashMap where keys are version strings
                // Treat yanked releases as prereleases (a version is yanked when any
                // of its artifacts has been yanked).
                let releases: Vec<Release> = pypi_response
                    .releases
                    .into_iter()
                    .map(|(version, artifacts)| Release {
                        tag_name: version,
                        is_prerelease: artifacts.iter().any(|a| a.yanked),
                    })
                    .collect();

                // Filter and find best match
                find_best_release(
                    releases,
                    current_version,
                    strategy,
                    version_prefix,
                    explicit_version,
                    version_regex,
                    include_prereleases,
                )
            },
        }
    }

    /// Get a human-readable description of this source
    pub fn description(&self) -> String {
        match self {
            UpstreamSource::GitHub { owner, repo } => format!("GitHub repo: {owner}/{repo}"),
            UpstreamSource::GitLab { owner, project } => {
                format!("GitLab project: {owner}/{project}")
            },
            UpstreamSource::SourceHut { owner, repo } => {
                format!("SourceHut repo: {owner}/{repo}")
            },
            UpstreamSource::PyPI { pname } => format!("PyPI package: {pname}"),
        }
    }
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
fn find_best_release(
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
        for release in releases {
            let version = release.resolved_version(version_regex)?;

            // Match either the tag directly or the extracted version
            if release.tag_name == target_version || version == target_version {
                return Ok(release);
            }
        }

        anyhow::bail!("Explicit version '{target_version}' not found in available releases");
    }

    // Filter releases by semver strategy and optionally include prereleases.
    // Owning each `Release` here means the eventually-chosen one can be returned by move.
    let mut compatible_releases: Vec<Release> = releases
        .into_iter()
        .filter(|r| {
            r.matches(
                current_version,
                strategy,
                version_prefix,
                version_regex,
                include_prereleases,
            )
        })
        .collect();

    // Sort by version (newest first)
    compatible_releases.sort_by(|a, b| {
        let version_a = extract_version_from_tag(&a.tag_name);
        let version_b = extract_version_from_tag(&b.tag_name);
        // Reverse order for newest first
        version_cmp_desc(version_a, version_b)
    });

    // Return the best (first after sorting) release by moving it out of the
    // sorted Vec; no clone required.
    compatible_releases.into_iter().next().ok_or_else(|| {
        anyhow::anyhow!(
            "No compatible releases found for version {current_version} with strategy {strategy}"
        )
    })
}

/// Compare two version strings descendingly (newest first).
///
/// Tries to parse both sides as semver (after stripping a leading `v`) and
/// returns `b.cmp(&a)` so that the larger version sorts first. Falls back to
/// lexicographic comparison if either side fails to parse, in which case the
/// fallback is also reversed (`b.cmp(a)`) to preserve newest-first ordering.
fn version_cmp_desc(a: &str, b: &str) -> std::cmp::Ordering {
    match (
        Version::parse(a.trim_start_matches('v')),
        Version::parse(b.trim_start_matches('v')),
    ) {
        (Ok(va), Ok(vb)) => vb.cmp(&va),
        _ => b.cmp(a),
    }
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
/// assert_eq!(extract_version_from_tag("1.2.3-unstable"), "1.2.3");
/// assert_eq!(
///     extract_version_from_tag("v2.0.0-unstable-2024-01-01"),
///     "2.0.0"
/// );
/// ```
pub fn extract_version_from_tag(tag: &str) -> &str {
    // Find the first digit in the tag
    let version = if let Some(pos) = tag.find(|c: char| c.is_ascii_digit()) {
        &tag[pos..]
    } else {
        // If no digit found, return the original tag
        return tag;
    };

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

/// Normalize a version string to ensure it has at least 3 components for semver parsing
///
/// Appends missing version components to ensure the version can be parsed as valid semver.
/// This prevents falling back to lexicographic string comparison for versions like "1.25".
///
/// # Arguments
/// * `version` - The version string to normalize
///
/// # Returns
/// A normalized version string with at least 3 components (MAJOR.MINOR.PATCH)
///
/// # Examples
/// ```
/// use ekapkgs_update::vcs_sources::normalize_version;
///
/// assert_eq!(normalize_version("1.25"), "1.25.0");
/// assert_eq!(normalize_version("1.9"), "1.9.0");
/// assert_eq!(normalize_version("2"), "2.0.0");
/// assert_eq!(normalize_version("1.2.3"), "1.2.3");
/// assert_eq!(normalize_version("1.0.0-beta"), "1.0.0-beta");
/// ```
pub fn normalize_version(version: &str) -> String {
    // Count the number of dots to determine component count
    // Handle pre-release versions by splitting on '-' first
    let (base_version, suffix) = if let Some(dash_pos) = version.find('-') {
        (&version[..dash_pos], Some(&version[dash_pos..]))
    } else {
        (version, None)
    };

    let dot_count = base_version.matches('.').count();

    let normalized_base = match dot_count {
        0 => format!("{base_version}.0.0"), // "1" -> "1.0.0"
        1 => format!("{base_version}.0"),   // "1.25" -> "1.25.0"
        _ => base_version.to_owned(),       // "1.2.3" or more -> unchanged
    };

    if let Some(suffix) = suffix {
        format!("{normalized_base}{suffix}")
    } else {
        normalized_base
    }
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
/// Versions that fail to parse as semver fall back to lexicographic string
/// comparison for `Latest`/`Major`, and are rejected for `Minor`/`Patch`.
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
    // Strip common prefixes like 'v' or 'version-'
    let clean_current = current
        .trim_start_matches('v')
        .trim_start_matches("version-");
    let clean_new = new.trim_start_matches('v').trim_start_matches("version-");

    // Reject versions that don't have at least major and minor components
    // Extract base version (before any '-' for pre-release)
    let base_new = clean_new.split('-').next().unwrap_or(clean_new);
    let base_current = clean_current.split('-').next().unwrap_or(clean_current);

    // Require at least one dot (indicating major.minor format)
    if !base_new.contains('.') || !base_current.contains('.') {
        debug!(
            "Rejecting version without major.minor components (current: {}, new: {})",
            clean_current, clean_new
        );
        return false;
    }

    // Normalize versions to ensure they have 3 components for semver parsing
    let normalized_current = normalize_version(clean_current);
    let normalized_new = normalize_version(clean_new);

    // Try semantic versioning first
    if let (Ok(curr_ver), Ok(new_ver)) = (
        Version::parse(&normalized_current),
        Version::parse(&normalized_new),
    ) {
        // First check if new version is actually newer
        if new_ver < curr_ver {
            warn!(
                "Rejecting downgrade: {} -> {} (current version is newer)",
                current, new
            );
            return false;
        }
        if new_ver == curr_ver {
            debug!(
                "Rejecting same version: {} -> {} (no change)",
                current, new
            );
            return false;
        }

        // Apply strategy-specific constraints
        match strategy {
            // Accept any newer version
            SemverStrategy::Latest | SemverStrategy::Major => true,
            // Only accept if major version matches
            SemverStrategy::Minor => new_ver.major == curr_ver.major,
            // Only accept if major and minor versions match
            SemverStrategy::Patch => {
                new_ver.major == curr_ver.major && new_ver.minor == curr_ver.minor
            },
        }
    } else {
        // For non-semver versions, only Latest/Major strategies work
        debug!(
            "Could not parse versions as semver (current: {}, new: {}), using string comparison \
             (strategy: {})",
            clean_current, clean_new, strategy
        );

        match strategy {
            SemverStrategy::Latest | SemverStrategy::Major => {
                if clean_new <= clean_current {
                    if clean_new < clean_current {
                        warn!(
                            "Rejecting downgrade (string comparison): {} -> {}",
                            current, new
                        );
                    } else {
                        debug!(
                            "Rejecting same version (string comparison): {} -> {}",
                            current, new
                        );
                    }
                    false
                } else {
                    true
                }
            },
            SemverStrategy::Minor | SemverStrategy::Patch => {
                warn!(
                    "Version '{}' is not valid semver, cannot apply {} strategy. Skipping update.",
                    clean_current, strategy
                );
                false
            },
        }
    }
}

#[cfg(test)]
mod tests;
