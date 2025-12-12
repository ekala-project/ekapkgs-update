//! VCS source abstraction for GitHub, GitLab, and other code hosting platforms

use std::env;

use semver::Version;
use tracing::{debug, warn};

use crate::github::{fetch_github_releases, fetch_github_tags, parse_github_url};
use crate::gitlab::{fetch_gitlab_releases, fetch_gitlab_tags, parse_gitlab_url};

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

/// Upstream VCS source (GitHub, GitLab, etc.)
#[derive(Debug)]
pub enum UpstreamSource {
    GitHub { owner: String, repo: String },
    GitLab { owner: String, project: String },
}

impl UpstreamSource {
    /// Parse a URL and return the appropriate UpstreamSource
    ///
    /// Tries to parse the URL as GitHub first, then GitLab.
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
            None
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
///
/// Removes all leading non-numerical characters from tag names to extract the version.
/// This handles various tag naming conventions like "v1.0.0", "release-1.0.0", "version-2.3.4",
/// etc.
///
/// # Arguments
/// * `tag` - The tag name to extract version from
///
/// # Returns
/// The version string with leading non-numerical characters removed
///
/// # Example
/// ```
/// use ekapkgs_update::vcs_sources::extract_version_from_tag;
///
/// assert_eq!(extract_version_from_tag("v1.0.0"), "1.0.0");
/// assert_eq!(extract_version_from_tag("release-2.3.4"), "2.3.4");
/// assert_eq!(extract_version_from_tag("version-1.2.3"), "1.2.3");
/// assert_eq!(extract_version_from_tag("1.0.0"), "1.0.0");
/// ```
pub fn extract_version_from_tag(tag: &str) -> &str {
    // Find the first digit in the tag
    if let Some(pos) = tag.find(|c: char| c.is_ascii_digit()) {
        &tag[pos..]
    } else {
        // If no digit found, return the original tag
        tag
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

    // Try semantic versioning first
    if let (Ok(curr_ver), Ok(new_ver)) = (Version::parse(clean_current), Version::parse(clean_new))
    {
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
}
