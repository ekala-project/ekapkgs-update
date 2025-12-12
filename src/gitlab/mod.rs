//! GitLab API integration and utilities

use regex::Regex;
use serde::Deserialize;
use tracing::debug;

/// GitLab release information from the API
#[derive(Debug, Deserialize)]
pub struct GitlabRelease {
    pub tag_name: String,
    pub _name: Option<String>,
    #[serde(default)]
    pub upcoming_release: bool,
}

/// Represents a GitLab project with owner/group and project name
#[derive(Debug)]
pub struct GitlabProject {
    pub owner: String,
    pub project: String,
}

impl GitlabProject {
    /// Get URL-encoded project path for API calls
    pub fn encoded_path(&self) -> String {
        format!("{}%2F{}", self.owner, self.project)
    }
}

/// GitLab tag information from the API
#[derive(Debug, Deserialize)]
struct GitlabTag {
    name: String,
}

/// Parse GitLab URL to extract owner/group and project
///
/// Supports various GitLab URL formats:
/// - HTTPS: `https://gitlab.com/owner/project`
/// - SSH: `git@gitlab.com:owner/project.git`
/// - With paths: `https://gitlab.com/owner/project/-/archive/v1.0.0.tar.gz`
/// - Nested groups: `https://gitlab.com/group/subgroup/project`
///
/// # Arguments
/// * `url` - GitLab URL to parse
///
/// # Returns
/// `Some(GitlabProject)` if the URL is a valid GitLab URL, `None` otherwise
///
/// # Example
/// ```
/// use ekapkgs_update::gitlab::parse_gitlab_url;
///
/// let project = parse_gitlab_url("https://gitlab.com/owner/project").unwrap();
/// assert_eq!(project.owner, "owner");
/// assert_eq!(project.project, "project");
/// ```
pub fn parse_gitlab_url(url: &str) -> Option<GitlabProject> {
    // Match gitlab.com with support for nested groups (but we'll only take last two parts)
    let gitlab_regex = Regex::new(r"gitlab\.com[:/]([^/]+)/([^/]+?)(?:\.git|/-|/|$)").ok()?;
    let caps = gitlab_regex.captures(url)?;

    Some(GitlabProject {
        owner: caps.get(1)?.as_str().to_string(),
        project: caps.get(2)?.as_str().to_string(),
    })
}

/// Fetch tags from GitLab API
///
/// Internal helper function to retrieve all tags from a repository.
/// Tags are returned in reverse chronological order (newest first).
///
/// # Arguments
/// * `owner` - Project owner/group
/// * `project` - Project name
/// * `token` - Optional GitLab personal access token for authentication
///
/// # Returns
/// A vector of tags, or an empty vector if no tags exist
async fn fetch_gitlab_tags(
    owner: &str,
    project: &str,
    token: Option<&str>,
) -> anyhow::Result<Vec<GitlabTag>> {
    let encoded_path = format!("{}%2F{}", owner, project);
    let url = format!(
        "https://gitlab.com/api/v4/projects/{}/repository/tags?order_by=updated&sort=desc",
        encoded_path
    );

    debug!("Fetching tags from {}", url);

    let client = reqwest::Client::new();
    let mut request = client.get(&url).header("User-Agent", "ekapkgs-update");

    // Add authorization header if token is provided
    if let Some(token_str) = token {
        request = request.header("PRIVATE-TOKEN", token_str);
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        anyhow::bail!(
            "GitLab tags API request failed with status: {}",
            response.status()
        );
    }

    let tags: Vec<GitlabTag> = response.json().await?;
    Ok(tags)
}

/// Fetch latest release from GitLab API
///
/// Makes an API call to GitLab to retrieve the latest release for a given project.
/// Optionally authenticates with a GitLab token for higher rate limits.
///
/// # Arguments
/// * `owner` - Project owner/group
/// * `project` - Project name
/// * `token` - Optional GitLab personal access token for authentication
///
/// # Returns
/// The latest release information from GitLab
///
/// # Rate Limits
/// - **Without token**: ~300 requests per hour (varies by instance)
/// - **With token**: Higher limits (varies by instance settings)
///
/// # Errors
/// Returns an error if:
/// - The API request fails
/// - The response cannot be deserialized
/// - The HTTP status is not successful
/// - Rate limit is exceeded
///
/// # Example
/// ```no_run
/// use ekapkgs_update::gitlab::fetch_latest_gitlab_release;
///
/// # async fn example() -> anyhow::Result<()> {
/// // With authentication
/// let token = std::env::var("GITLAB_TOKEN").ok();
/// let release = fetch_latest_gitlab_release("owner", "project", token.as_deref()).await?;
/// println!("Latest version: {}", release.tag_name);
///
/// // Without authentication (lower rate limits)
/// let release = fetch_latest_gitlab_release("owner", "project", None).await?;
/// # Ok(())
/// # }
/// ```
pub async fn fetch_latest_gitlab_release(
    owner: &str,
    project: &str,
    token: Option<&str>,
) -> anyhow::Result<GitlabRelease> {
    let encoded_path = format!("{}%2F{}", owner, project);
    let url = format!(
        "https://gitlab.com/api/v4/projects/{}/releases",
        encoded_path
    );

    debug!("Fetching latest release from {}", url);

    let client = reqwest::Client::new();
    let mut request = client.get(&url).header("User-Agent", "ekapkgs-update");

    // Add authorization header if token is provided
    if let Some(token_str) = token {
        request = request.header("PRIVATE-TOKEN", token_str);
    }

    let response = request.send().await?;

    // If releases endpoint returns 404, fallback to tags
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        debug!(
            "No releases found for {}/{}, falling back to tags",
            owner, project
        );
        let tags = fetch_gitlab_tags(owner, project, token).await?;

        if let Some(first_tag) = tags.first() {
            let version = crate::vcs_sources::extract_version_from_tag(&first_tag.name);
            debug!(
                "Using latest tag: {} (extracted version: {})",
                first_tag.name, version
            );
            return Ok(GitlabRelease {
                tag_name: version.to_string(),
                _name: None,
                upcoming_release: false,
            });
        } else {
            anyhow::bail!("No releases or tags found for {}/{}", owner, project);
        }
    }

    if !response.status().is_success() {
        anyhow::bail!(
            "GitLab API request failed with status: {}",
            response.status()
        );
    }

    let releases: Vec<GitlabRelease> = response.json().await?;

    // Find the first non-prerelease (upcoming_release: false)
    let release = releases
        .into_iter()
        .find(|r| !r.upcoming_release)
        .ok_or_else(|| {
            anyhow::anyhow!("No non-prerelease releases found for {}/{}", owner, project)
        })?;

    Ok(release)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gitlab_url_https() {
        let url = "https://gitlab.com/owner/project";
        let result = parse_gitlab_url(url);
        assert!(result.is_some());
        let project = result.unwrap();
        assert_eq!(project.owner, "owner");
        assert_eq!(project.project, "project");
    }

    #[test]
    fn test_parse_gitlab_url_git() {
        let url = "git@gitlab.com:owner/project.git";
        let result = parse_gitlab_url(url);
        assert!(result.is_some());
        let project = result.unwrap();
        assert_eq!(project.owner, "owner");
        assert_eq!(project.project, "project");
    }

    #[test]
    fn test_parse_gitlab_url_with_path() {
        let url = "https://gitlab.com/owner/project/-/archive/v1.0.0/project-v1.0.0.tar.gz";
        let result = parse_gitlab_url(url);
        assert!(result.is_some());
        let project = result.unwrap();
        assert_eq!(project.owner, "owner");
        assert_eq!(project.project, "project");
    }

    #[test]
    fn test_parse_gitlab_url_invalid() {
        let url = "https://github.com/owner/repo";
        let result = parse_gitlab_url(url);
        assert!(result.is_none());
    }

    #[test]
    fn test_encoded_path() {
        let project = GitlabProject {
            owner: "owner".to_string(),
            project: "project".to_string(),
        };
        assert_eq!(project.encoded_path(), "owner%2Fproject");
    }

    #[test]
    fn test_encoded_path_with_special_chars() {
        let project = GitlabProject {
            owner: "my-group".to_string(),
            project: "my-project".to_string(),
        };
        assert_eq!(project.encoded_path(), "my-group%2Fmy-project");
    }
}
