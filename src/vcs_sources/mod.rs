//! VCS source abstraction for GitHub, GitLab, and other code hosting platforms

use std::env;

use regex::Regex;
use semver::Version;
use tracing::{debug, warn};

use crate::github::{fetch_github_releases, fetch_github_tags, parse_github_url};
use crate::gitlab::{fetch_gitlab_releases, fetch_gitlab_tags, parse_gitlab_url};
use crate::pypi::fetch_pypi_releases;

/// Release information from a VCS source
#[derive(Debug)]
pub struct Release {
    pub tag_name: String,
    pub is_prerelease: bool,
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

impl SemverStrategy {
    /// Parse strategy from string
    pub fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.to_lowercase().as_str() {
            "latest" => Ok(SemverStrategy::Latest),
            "major" => Ok(SemverStrategy::Major),
            "minor" => Ok(SemverStrategy::Minor),
            "patch" => Ok(SemverStrategy::Patch),
            _ => anyhow::bail!(
                "Invalid semver strategy: '{}'. Valid options: latest, major, minor, patch",
                s
            ),
        }
    }
}

/// Upstream VCS source (GitHub, GitLab, PyPI, etc.)
#[derive(Debug)]
pub enum UpstreamSource {
    GitHub { owner: String, repo: String },
    GitLab { owner: String, project: String },
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
/// Returns the package name if found
fn parse_pypi_url(url: &str) -> Option<String> {
    // Match mirror://pypi/{first-letter}/{package-name}/{filename}
    // Format: mirror://pypi/a/azure-mgmt-advisor/azure-mgmt-advisor-9.0.0.zip
    if url.starts_with("mirror://pypi/") {
        let parts: Vec<&str> = url.split('/').collect();
        // Expected format: ["mirror:", "", "pypi", "{letter}", "{package-name}", "{filename}"]
        if parts.len() >= 5 {
            // Extract package name from the path (4th element, 0-indexed)
            return Some(parts[4].to_string());
        }
    }

    // Match pypi.org/project/{pname}
    if let Ok(pypi_project_regex) = Regex::new(r"pypi\.(?:python\.)?org/project/([^/]+)") {
        if let Some(caps) = pypi_project_regex.captures(url) {
            return caps.get(1).map(|m| m.as_str().to_string());
        }
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
                        return Some(potential_name.to_string());
                    }
                }
            }
        }
    }

    None
}

impl UpstreamSource {
    /// Parse a URL and return the appropriate UpstreamSource
    ///
    /// Tries to parse the URL as GitHub first, then GitLab, then PyPI.
    ///
    /// # Arguments
    /// * `url` - Source URL to parse
    ///
    /// # Returns
    /// `Some(UpstreamSource)` if the URL matches a known VCS platform, `None` otherwise
    pub fn from_url(url: &str) -> Option<Self> {
        if let Some(github_repo) = parse_github_url(url) {
            Some(UpstreamSource::GitHub {
                owner: github_repo.owner,
                repo: github_repo.repo,
            })
        } else if let Some(gitlab_project) = parse_gitlab_url(url) {
            Some(UpstreamSource::GitLab {
                owner: gitlab_project.owner,
                project: gitlab_project.project,
            })
        } else {
            parse_pypi_url(url).map(|pypi_pname| UpstreamSource::PyPI { pname: pypi_pname })
        }
    }

    /// Get the best compatible release based on semver strategy
    ///
    /// Fetches all releases/tags from the VCS platform and filters them based on
    /// the semver strategy to find the best match for the current version.
    /// Automatically checks for authentication tokens in environment variables:
    /// - `GITHUB_TOKEN` for GitHub sources
    /// - `GITLAB_TOKEN` for GitLab sources
    ///
    /// # Arguments
    /// * `current_version` - The current version to compare against
    /// * `strategy` - The semver update strategy to apply
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

                // Try to fetch all releases first
                let all_releases = fetch_github_releases(owner, repo, token.as_deref()).await;

                let releases: Vec<Release> = match all_releases {
                    Ok(gh_releases) => {
                        // Convert GitHub releases to our Release struct
                        gh_releases
                            .into_iter()
                            .map(|r| Release {
                                tag_name: r.tag_name,
                                is_prerelease: r.prerelease,
                            })
                            .collect()
                    },
                    Err(_) => {
                        // Fallback to tags if releases endpoint fails
                        debug!("No releases found, falling back to tags");
                        let tags = fetch_github_tags(owner, repo, token.as_deref()).await?;
                        tags.into_iter()
                            .map(|t| Release {
                                tag_name: t.name,
                                is_prerelease: false,
                            })
                            .collect()
                    },
                };

                // Filter and find best match
                find_best_release(&releases, current_version, strategy)
            },
            UpstreamSource::GitLab { owner, project } => {
                let token = env::var("GITLAB_TOKEN").ok();

                if token.is_none() {
                    warn!(
                        "GITLAB_TOKEN not set - using unauthenticated GitLab API (~300 \
                         requests/hour rate limit)"
                    );
                }

                // Try to fetch all releases first
                let all_releases = fetch_gitlab_releases(owner, project, token.as_deref()).await;

                let releases: Vec<Release> = match all_releases {
                    Ok(gl_releases) => {
                        // Convert GitLab releases to our Release struct
                        gl_releases
                            .into_iter()
                            .map(|r| Release {
                                tag_name: r.tag_name,
                                is_prerelease: r.upcoming_release,
                            })
                            .collect()
                    },
                    Err(_) => {
                        // Fallback to tags if releases endpoint fails
                        debug!("No releases found, falling back to tags");
                        let tags = fetch_gitlab_tags(owner, project, token.as_deref()).await?;
                        tags.into_iter()
                            .map(|t| Release {
                                tag_name: t.name,
                                is_prerelease: false,
                            })
                            .collect()
                    },
                };

                // Filter and find best match
                find_best_release(&releases, current_version, strategy)
            },
            UpstreamSource::PyPI { pname } => {
                // PyPI doesn't require authentication tokens
                let pypi_response = fetch_pypi_releases(pname).await?;

                // Convert PyPI releases to our Release struct
                // PyPI returns a HashMap where keys are version strings
                let mut releases: Vec<Release> = Vec::new();

                for (version, artifacts) in pypi_response.releases {
                    // Check if this version has been yanked (any artifact yanked means version is
                    // yanked)
                    let is_yanked = artifacts.iter().any(|a| a.yanked);

                    releases.push(Release {
                        tag_name: version,
                        is_prerelease: is_yanked, // Treat yanked releases as prereleases
                    });
                }

                // Filter and find best match
                find_best_release(&releases, current_version, strategy)
            },
        }
    }

    /// Extract clean version string from a release
    ///
    /// Removes common prefixes like 'v', 'release-', etc. from tag names.
    ///
    /// # Arguments
    /// * `release` - The release to extract version from
    ///
    /// # Returns
    /// Clean version string
    pub fn get_version(release: &Release) -> String {
        extract_version_from_tag(&release.tag_name).to_string()
    }

    /// Get a human-readable description of this source
    pub fn description(&self) -> String {
        match self {
            UpstreamSource::GitHub { owner, repo } => format!("GitHub repo: {}/{}", owner, repo),
            UpstreamSource::GitLab { owner, project } => {
                format!("GitLab project: {}/{}", owner, project)
            },
            UpstreamSource::PyPI { pname } => format!("PyPI package: {}", pname),
        }
    }
}

/// Find the best compatible release from a list based on semver strategy
///
/// Filters releases by:
/// 1. Excluding prereleases
/// 2. Checking version compatibility with strategy
/// 3. Returns the newest compatible version
///
/// # Arguments
/// * `releases` - List of releases to filter
/// * `current_version` - Current version to compare against
/// * `strategy` - Semver strategy to apply
///
/// # Returns
/// The best matching release
///
/// # Errors
/// Returns an error if no compatible releases are found
fn find_best_release(
    releases: &[Release],
    current_version: &str,
    strategy: SemverStrategy,
) -> anyhow::Result<Release> {
    // Filter out prereleases and find compatible versions
    let mut compatible_releases: Vec<&Release> = releases
        .iter()
        .filter(|r| !r.is_prerelease)
        .filter(|r| {
            let version = extract_version_from_tag(&r.tag_name);
            is_version_acceptable(current_version, version, strategy).unwrap_or(false)
        })
        .collect();

    if compatible_releases.is_empty() {
        anyhow::bail!(
            "No compatible releases found for version {} with strategy {:?}",
            current_version,
            strategy
        );
    }

    // Sort by version (newest first)
    compatible_releases.sort_by(|a, b| {
        let version_a = extract_version_from_tag(&a.tag_name);
        let version_b = extract_version_from_tag(&b.tag_name);

        // Try to parse as semver for proper sorting
        match (
            Version::parse(version_a.trim_start_matches('v')),
            Version::parse(version_b.trim_start_matches('v')),
        ) {
            (Ok(va), Ok(vb)) => vb.cmp(&va), // Reverse order for newest first
            _ => version_b.cmp(version_a),   // Fallback to string comparison
        }
    });

    // Return the best (first after sorting) release
    Ok(Release {
        tag_name: compatible_releases[0].tag_name.clone(),
        is_prerelease: compatible_releases[0].is_prerelease,
    })
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
        0 => format!("{}.0.0", base_version), // "1" -> "1.0.0"
        1 => format!("{}.0", base_version),   // "1.25" -> "1.25.0"
        _ => base_version.to_string(),        // "1.2.3" or more -> unchanged
    };

    if let Some(suffix) = suffix {
        format!("{}{}", normalized_base, suffix)
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
/// `Ok(true)` if the new version satisfies the strategy constraints, `Ok(false)` otherwise
///
/// # Errors
/// Returns an error if version comparison fails
///
/// # Examples
/// ```
/// use ekapkgs_update::vcs_sources::{SemverStrategy, is_version_acceptable};
///
/// // Latest strategy accepts any newer version
/// assert!(is_version_acceptable("1.0.0", "2.0.0", SemverStrategy::Latest).unwrap());
/// assert!(is_version_acceptable("1.0.0", "1.1.0", SemverStrategy::Latest).unwrap());
///
/// // Minor strategy only accepts same major version
/// assert!(is_version_acceptable("1.0.0", "1.1.0", SemverStrategy::Minor).unwrap());
/// assert!(!is_version_acceptable("1.0.0", "2.0.0", SemverStrategy::Minor).unwrap());
///
/// // Patch strategy only accepts same major.minor version
/// assert!(is_version_acceptable("1.0.0", "1.0.1", SemverStrategy::Patch).unwrap());
/// assert!(!is_version_acceptable("1.0.0", "1.1.0", SemverStrategy::Patch).unwrap());
/// ```
pub fn is_version_acceptable(
    current: &str,
    new: &str,
    strategy: SemverStrategy,
) -> anyhow::Result<bool> {
    // Strip common prefixes like 'v' or 'version-'
    let clean_current = current
        .trim_start_matches('v')
        .trim_start_matches("version-");
    let clean_new = new.trim_start_matches('v').trim_start_matches("version-");

    // Normalize versions to ensure they have 3 components for semver parsing
    let normalized_current = normalize_version(clean_current);
    let normalized_new = normalize_version(clean_new);

    // Try semantic versioning first
    if let (Ok(curr_ver), Ok(new_ver)) = (
        Version::parse(&normalized_current),
        Version::parse(&normalized_new),
    ) {
        // First check if new version is actually newer
        if new_ver <= curr_ver {
            return Ok(false);
        }

        // Apply strategy-specific constraints
        match strategy {
            SemverStrategy::Latest | SemverStrategy::Major => {
                // Accept any newer version
                Ok(true)
            },
            SemverStrategy::Minor => {
                // Only accept if major version matches
                Ok(new_ver.major == curr_ver.major)
            },
            SemverStrategy::Patch => {
                // Only accept if major and minor versions match
                Ok(new_ver.major == curr_ver.major && new_ver.minor == curr_ver.minor)
            },
        }
    } else {
        // For non-semver versions, only Latest/Major strategies work
        debug!(
            "Could not parse versions as semver (current: {}, new: {}), using string comparison \
             (strategy: {:?})",
            clean_current, clean_new, strategy
        );

        match strategy {
            SemverStrategy::Latest | SemverStrategy::Major => Ok(clean_new > clean_current),
            SemverStrategy::Minor | SemverStrategy::Patch => {
                warn!(
                    "Version '{}' is not valid semver, cannot apply {:?} strategy. Skipping \
                     update.",
                    clean_current, strategy
                );
                Ok(false)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_url_github() {
        let url = "https://github.com/owner/repo";
        let source = UpstreamSource::from_url(url);
        assert!(source.is_some());
        match source.unwrap() {
            UpstreamSource::GitHub { owner, repo } => {
                assert_eq!(owner, "owner");
                assert_eq!(repo, "repo");
            },
            _ => panic!("Expected GitHub source"),
        }
    }

    #[test]
    fn test_from_url_gitlab() {
        let url = "https://gitlab.com/owner/project";
        let source = UpstreamSource::from_url(url);
        assert!(source.is_some());
        match source.unwrap() {
            UpstreamSource::GitLab { owner, project } => {
                assert_eq!(owner, "owner");
                assert_eq!(project, "project");
            },
            _ => panic!("Expected GitLab source"),
        }
    }

    #[test]
    fn test_from_url_invalid() {
        let url = "https://example.com/some/path";
        let source = UpstreamSource::from_url(url);
        assert!(source.is_none());
    }

    #[test]
    fn test_from_url_pypi_project() {
        let url = "https://pypi.org/project/requests/";
        let source = UpstreamSource::from_url(url);
        assert!(source.is_some());
        match source.unwrap() {
            UpstreamSource::PyPI { pname } => {
                assert_eq!(pname, "requests");
            },
            _ => panic!("Expected PyPI source"),
        }
    }

    #[test]
    fn test_from_url_pypi_python_org() {
        let url = "https://pypi.python.org/project/django/";
        let source = UpstreamSource::from_url(url);
        assert!(source.is_some());
        match source.unwrap() {
            UpstreamSource::PyPI { pname } => {
                assert_eq!(pname, "django");
            },
            _ => panic!("Expected PyPI source"),
        }
    }

    #[test]
    fn test_from_url_pypi_files() {
        let url = "https://files.pythonhosted.org/packages/abc/def/requests-2.28.1.tar.gz";
        let source = UpstreamSource::from_url(url);
        assert!(source.is_some());
        match source.unwrap() {
            UpstreamSource::PyPI { pname } => {
                assert_eq!(pname, "requests");
            },
            _ => panic!("Expected PyPI source"),
        }
    }

    #[test]
    fn test_from_url_pypi_mirror() {
        let url = "mirror://pypi/a/azure-mgmt-advisor/azure-mgmt-advisor-9.0.0.zip";
        let source = UpstreamSource::from_url(url);
        assert!(source.is_some());
        match source.unwrap() {
            UpstreamSource::PyPI { pname } => {
                assert_eq!(pname, "azure-mgmt-advisor");
            },
            _ => panic!("Expected PyPI source"),
        }
    }

    #[test]
    fn test_from_url_pypi_mirror_single_letter() {
        let url = "mirror://pypi/r/requests/requests-2.28.1.tar.gz";
        let source = UpstreamSource::from_url(url);
        assert!(source.is_some());
        match source.unwrap() {
            UpstreamSource::PyPI { pname } => {
                assert_eq!(pname, "requests");
            },
            _ => panic!("Expected PyPI source"),
        }
    }

    #[test]
    fn test_description_github() {
        let source = UpstreamSource::GitHub {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
        };
        assert_eq!(source.description(), "GitHub repo: owner/repo");
    }

    #[test]
    fn test_description_gitlab() {
        let source = UpstreamSource::GitLab {
            owner: "owner".to_string(),
            project: "project".to_string(),
        };
        assert_eq!(source.description(), "GitLab project: owner/project");
    }

    #[test]
    fn test_get_version() {
        let release = Release {
            tag_name: "v1.2.3".to_string(),
            is_prerelease: false,
        };
        assert_eq!(UpstreamSource::get_version(&release), "1.2.3");
    }

    // SemverStrategy tests
    #[test]
    fn test_semver_strategy_from_str() {
        assert_eq!(
            SemverStrategy::from_str("latest").unwrap(),
            SemverStrategy::Latest
        );
        assert_eq!(
            SemverStrategy::from_str("major").unwrap(),
            SemverStrategy::Major
        );
        assert_eq!(
            SemverStrategy::from_str("minor").unwrap(),
            SemverStrategy::Minor
        );
        assert_eq!(
            SemverStrategy::from_str("patch").unwrap(),
            SemverStrategy::Patch
        );

        // Test case insensitivity
        assert_eq!(
            SemverStrategy::from_str("LATEST").unwrap(),
            SemverStrategy::Latest
        );
        assert_eq!(
            SemverStrategy::from_str("MaJoR").unwrap(),
            SemverStrategy::Major
        );

        // Test invalid
        assert!(SemverStrategy::from_str("invalid").is_err());
    }

    // is_version_acceptable tests - Latest strategy
    #[test]
    fn test_version_acceptable_latest() {
        // Latest accepts any newer version
        assert!(is_version_acceptable("1.0.0", "2.0.0", SemverStrategy::Latest).unwrap());
        assert!(is_version_acceptable("1.0.0", "1.1.0", SemverStrategy::Latest).unwrap());
        assert!(is_version_acceptable("1.0.0", "1.0.1", SemverStrategy::Latest).unwrap());

        // Doesn't accept same or older
        assert!(!is_version_acceptable("1.0.0", "1.0.0", SemverStrategy::Latest).unwrap());
        assert!(!is_version_acceptable("2.0.0", "1.0.0", SemverStrategy::Latest).unwrap());
    }

    // is_version_acceptable tests - Major strategy
    #[test]
    fn test_version_acceptable_major() {
        // Major accepts any newer version (same as latest)
        assert!(is_version_acceptable("1.0.0", "2.0.0", SemverStrategy::Major).unwrap());
        assert!(is_version_acceptable("1.0.0", "1.1.0", SemverStrategy::Major).unwrap());
        assert!(is_version_acceptable("1.0.0", "1.0.1", SemverStrategy::Major).unwrap());

        // Doesn't accept same or older
        assert!(!is_version_acceptable("1.0.0", "1.0.0", SemverStrategy::Major).unwrap());
        assert!(!is_version_acceptable("2.0.0", "1.0.0", SemverStrategy::Major).unwrap());
    }

    // is_version_acceptable tests - Minor strategy
    #[test]
    fn test_version_acceptable_minor() {
        // Minor accepts only same major version
        assert!(is_version_acceptable("1.0.0", "1.1.0", SemverStrategy::Minor).unwrap());
        assert!(is_version_acceptable("1.0.0", "1.0.1", SemverStrategy::Minor).unwrap());
        assert!(is_version_acceptable("1.5.2", "1.9.0", SemverStrategy::Minor).unwrap());

        // Doesn't accept different major version
        assert!(!is_version_acceptable("1.0.0", "2.0.0", SemverStrategy::Minor).unwrap());
        assert!(!is_version_acceptable("1.0.0", "2.1.0", SemverStrategy::Minor).unwrap());

        // Doesn't accept same or older
        assert!(!is_version_acceptable("1.5.0", "1.5.0", SemverStrategy::Minor).unwrap());
        assert!(!is_version_acceptable("1.5.0", "1.4.0", SemverStrategy::Minor).unwrap());
    }

    // is_version_acceptable tests - Patch strategy
    #[test]
    fn test_version_acceptable_patch() {
        // Patch accepts only same major.minor version
        assert!(is_version_acceptable("1.0.0", "1.0.1", SemverStrategy::Patch).unwrap());
        assert!(is_version_acceptable("1.0.0", "1.0.9", SemverStrategy::Patch).unwrap());
        assert!(is_version_acceptable("2.3.4", "2.3.5", SemverStrategy::Patch).unwrap());

        // Doesn't accept different minor version
        assert!(!is_version_acceptable("1.0.0", "1.1.0", SemverStrategy::Patch).unwrap());
        assert!(!is_version_acceptable("1.0.0", "1.1.1", SemverStrategy::Patch).unwrap());

        // Doesn't accept different major version
        assert!(!is_version_acceptable("1.0.0", "2.0.0", SemverStrategy::Patch).unwrap());
        assert!(!is_version_acceptable("1.0.0", "2.0.1", SemverStrategy::Patch).unwrap());

        // Doesn't accept same or older
        assert!(!is_version_acceptable("1.0.5", "1.0.5", SemverStrategy::Patch).unwrap());
        assert!(!is_version_acceptable("1.0.5", "1.0.4", SemverStrategy::Patch).unwrap());
    }

    // Test with version prefixes
    #[test]
    fn test_version_acceptable_with_prefixes() {
        // v prefix
        assert!(is_version_acceptable("v1.0.0", "v2.0.0", SemverStrategy::Latest).unwrap());
        assert!(is_version_acceptable("v1.0.0", "2.0.0", SemverStrategy::Latest).unwrap());
        assert!(is_version_acceptable("1.0.0", "v2.0.0", SemverStrategy::Latest).unwrap());

        // Minor with v prefix
        assert!(is_version_acceptable("v1.0.0", "v1.1.0", SemverStrategy::Minor).unwrap());
        assert!(!is_version_acceptable("v1.0.0", "v2.0.0", SemverStrategy::Minor).unwrap());
    }

    // Test non-semver versions
    #[test]
    fn test_version_acceptable_non_semver() {
        // Latest/Major strategies should fall back to string comparison
        assert!(is_version_acceptable("2024.01.01", "2024.12.01", SemverStrategy::Latest).unwrap());
        assert!(is_version_acceptable("2024.01.01", "2024.12.01", SemverStrategy::Major).unwrap());

        // Minor/Patch strategies should reject non-semver
        assert!(!is_version_acceptable("2024.01.01", "2024.12.01", SemverStrategy::Minor).unwrap());
        assert!(!is_version_acceptable("2024.01.01", "2024.12.01", SemverStrategy::Patch).unwrap());
    }

    // Test edge case: version 0.x.y
    #[test]
    fn test_version_acceptable_zero_versions() {
        // 0.x versions
        assert!(is_version_acceptable("0.1.0", "0.2.0", SemverStrategy::Latest).unwrap());
        assert!(is_version_acceptable("0.1.0", "0.2.0", SemverStrategy::Major).unwrap());
        assert!(is_version_acceptable("0.1.0", "0.2.0", SemverStrategy::Minor).unwrap());
        assert!(!is_version_acceptable("0.1.0", "0.2.0", SemverStrategy::Patch).unwrap());

        assert!(is_version_acceptable("0.1.0", "0.1.1", SemverStrategy::Patch).unwrap());
    }

    // Test normalize_version function
    #[test]
    fn test_normalize_version() {
        // Two-component versions
        assert_eq!(normalize_version("1.25"), "1.25.0");
        assert_eq!(normalize_version("1.9"), "1.9.0");
        assert_eq!(normalize_version("0.5"), "0.5.0");

        // Single-component versions
        assert_eq!(normalize_version("2"), "2.0.0");
        assert_eq!(normalize_version("10"), "10.0.0");

        // Already normalized (three components)
        assert_eq!(normalize_version("1.2.3"), "1.2.3");
        assert_eq!(normalize_version("0.0.1"), "0.0.1");

        // With pre-release suffixes
        assert_eq!(normalize_version("1.0-beta"), "1.0.0-beta");
        assert_eq!(normalize_version("2.5-rc1"), "2.5.0-rc1");
        assert_eq!(normalize_version("1.2.3-alpha"), "1.2.3-alpha");

        // With unstable suffix (gets normalized before truncation)
        assert_eq!(normalize_version("1.25-unstable"), "1.25.0-unstable");
    }

    // Test two-component version comparison (the libdeflate 1.25 vs 1.9 issue)
    #[test]
    fn test_version_acceptable_two_components() {
        // This is the key test case: 1.25 should be considered newer than 1.9
        assert!(!is_version_acceptable("1.25", "1.9", SemverStrategy::Latest).unwrap());
        assert!(is_version_acceptable("1.9", "1.25", SemverStrategy::Latest).unwrap());

        // More two-component version tests
        assert!(is_version_acceptable("1.0", "1.1", SemverStrategy::Latest).unwrap());
        assert!(is_version_acceptable("1.9", "1.10", SemverStrategy::Latest).unwrap());
        assert!(is_version_acceptable("2.0", "2.1", SemverStrategy::Latest).unwrap());

        // Two-component with Minor strategy
        assert!(is_version_acceptable("1.9", "1.25", SemverStrategy::Minor).unwrap());
        assert!(!is_version_acceptable("1.9", "2.0", SemverStrategy::Minor).unwrap());

        // Two-component with Patch strategy (should upgrade minor version)
        assert!(!is_version_acceptable("1.9", "1.25", SemverStrategy::Patch).unwrap());
        assert!(is_version_acceptable("1.9", "1.9.1", SemverStrategy::Patch).unwrap());
    }

    // Test mixed component version comparison
    #[test]
    fn test_version_acceptable_mixed_components() {
        // Two-component current, three-component new
        assert!(is_version_acceptable("1.9", "1.25.0", SemverStrategy::Latest).unwrap());
        assert!(is_version_acceptable("1.9", "1.9.1", SemverStrategy::Latest).unwrap());

        // Three-component current, two-component new
        assert!(is_version_acceptable("1.9.0", "1.25", SemverStrategy::Latest).unwrap());
        assert!(!is_version_acceptable("1.25.0", "1.9", SemverStrategy::Latest).unwrap());
    }
}
