//! VCS source abstraction for GitHub, GitLab, and other code hosting platforms

use std::env;

use semver::Version;
use tracing::{debug, warn};

use crate::github::{fetch_latest_github_release, parse_github_url};
use crate::gitlab::{fetch_latest_gitlab_release, parse_gitlab_url};

/// Release information from a VCS source
#[derive(Debug)]
pub struct Release {
    pub tag_name: String,
    pub is_prerelease: bool,
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

    /// Get the latest release from the upstream source
    ///
    /// Fetches the latest release/tag from the VCS platform.
    /// Automatically checks for authentication tokens in environment variables:
    /// - `GITHUB_TOKEN` for GitHub sources
    /// - `GITLAB_TOKEN` for GitLab sources
    ///
    /// # Returns
    /// The latest release information
    ///
    /// # Errors
    /// Returns an error if the API request fails or no releases are found
    pub async fn get_latest_release(&self) -> anyhow::Result<Release> {
        match self {
            UpstreamSource::GitHub { owner, repo } => {
                let token = env::var("GITHUB_TOKEN").ok();

                if token.is_none() {
                    warn!(
                        "GITHUB_TOKEN not set - using unauthenticated GitHub API (60 \
                         requests/hour rate limit)"
                    );
                }

                let github_release =
                    fetch_latest_github_release(owner, repo, token.as_deref()).await?;

                Ok(Release {
                    tag_name: github_release.tag_name,
                    is_prerelease: github_release.prerelease,
                })
            },
            UpstreamSource::GitLab { owner, project } => {
                let token = env::var("GITLAB_TOKEN").ok();

                if token.is_none() {
                    warn!(
                        "GITLAB_TOKEN not set - using unauthenticated GitLab API (~300 \
                         requests/hour rate limit)"
                    );
                }

                let gitlab_release =
                    fetch_latest_gitlab_release(owner, project, token.as_deref()).await?;

                Ok(Release {
                    tag_name: gitlab_release.tag_name,
                    is_prerelease: gitlab_release.upcoming_release,
                })
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

    /// Compare two version strings
    ///
    /// # Arguments
    /// * `current` - Current version string
    /// * `new` - New version string
    ///
    /// # Returns
    /// `Ok(true)` if new version is newer, `Ok(false)` otherwise
    pub fn is_version_newer(current: &str, new: &str) -> anyhow::Result<bool> {
        is_version_newer(current, new)
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

/// Compare versions and return true if new_version is greater than current_version
///
/// # Arguments
/// * `current` - Current version string
/// * `new` - New version string
///
/// # Returns
/// `Ok(true)` if new version is newer, `Ok(false)` otherwise
///
/// # Errors
/// Returns an error if version comparison fails
pub fn is_version_newer(current: &str, new: &str) -> anyhow::Result<bool> {
    // Strip common prefixes like 'v' or 'version-'
    let clean_current = current
        .trim_start_matches('v')
        .trim_start_matches("version-");
    let clean_new = new.trim_start_matches('v').trim_start_matches("version-");

    // Try semantic versioning first
    if let (Ok(curr_ver), Ok(new_ver)) = (Version::parse(clean_current), Version::parse(clean_new))
    {
        return Ok(new_ver > curr_ver);
    }

    // Fallback to string comparison
    debug!("Could not parse versions as semver, falling back to string comparison");
    Ok(clean_new > clean_current)
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
}
