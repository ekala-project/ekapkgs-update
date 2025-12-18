//! GitHub API integration and utilities

use regex::Regex;
use serde::Deserialize;
use tracing::debug;

/// GitHub release information from the API
#[derive(Debug, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    pub _name: Option<String>,
    pub prerelease: bool,
}

/// Represents a GitHub repository with owner and name
#[derive(Debug)]
pub struct GithubRepo {
    pub owner: String,
    pub repo: String,
}

/// GitHub tag information from the API
#[derive(Debug, Deserialize)]
pub struct GithubTag {
    pub name: String,
}

/// GitHub PR creation response from the API
#[derive(Debug, Deserialize)]
pub struct GithubPullRequest {
    pub html_url: String,
    pub number: i64,
}

/// Parse GitHub URL to extract owner and repo
///
/// Supports various GitHub URL formats:
/// - HTTPS: `https://github.com/owner/repo`
/// - SSH: `git@github.com:owner/repo.git`
/// - With paths: `https://github.com/owner/repo/archive/v1.0.0.tar.gz`
///
/// # Arguments
/// * `url` - GitHub URL to parse
///
/// # Returns
/// `Some(GithubRepo)` if the URL is a valid GitHub URL, `None` otherwise
///
/// # Example
/// ```
/// use ekapkgs_update::github::parse_github_url;
///
/// let repo = parse_github_url("https://github.com/owner/repo").unwrap();
/// assert_eq!(repo.owner, "owner");
/// assert_eq!(repo.repo, "repo");
/// ```
pub fn parse_github_url(url: &str) -> Option<GithubRepo> {
    let github_regex = Regex::new(r"github\.com[:/]([^/]+)/([^/]+?)(?:\.git|/|$)").ok()?;
    let caps = github_regex.captures(url)?;

    Some(GithubRepo {
        owner: caps.get(1)?.as_str().to_string(),
        repo: caps.get(2)?.as_str().to_string(),
    })
}

/// Fetch tags from GitHub API
///
/// Retrieves all tags from a repository.
/// Tags are returned in reverse chronological order (newest first).
///
/// # Arguments
/// * `owner` - Repository owner/organization
/// * `repo` - Repository name
/// * `token` - Optional GitHub personal access token for authentication
///
/// # Returns
/// A vector of tags, or an empty vector if no tags exist
pub async fn fetch_github_tags(
    owner: &str,
    repo: &str,
    token: Option<&str>,
) -> anyhow::Result<Vec<GithubTag>> {
    let url = format!("https://api.github.com/repos/{}/{}/tags", owner, repo);

    debug!("Fetching tags from {}", url);

    let client = reqwest::Client::new();
    let mut request = client
        .get(&url)
        .header("User-Agent", "ekapkgs-update")
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");

    // Add authorization header if token is provided
    if let Some(token_str) = token {
        request = request.header("Authorization", format!("Bearer {}", token_str));
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        anyhow::bail!(
            "GitHub tags API request failed with status: {}",
            response.status()
        );
    }

    let tags: Vec<GithubTag> = response.json().await?;
    Ok(tags)
}

/// Fetch all releases from GitHub API
///
/// Retrieves all releases from a repository.
/// Releases are returned in reverse chronological order (newest first).
///
/// # Arguments
/// * `owner` - Repository owner/organization
/// * `repo` - Repository name
/// * `token` - Optional GitHub personal access token for authentication
///
/// # Returns
/// A vector of releases
pub async fn fetch_github_releases(
    owner: &str,
    repo: &str,
    token: Option<&str>,
) -> anyhow::Result<Vec<GithubRelease>> {
    let url = format!("https://api.github.com/repos/{}/{}/releases", owner, repo);

    debug!("Fetching all releases from {}", url);

    let client = reqwest::Client::new();
    let mut request = client
        .get(&url)
        .header("User-Agent", "ekapkgs-update")
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");

    // Add authorization header if token is provided
    if let Some(token_str) = token {
        request = request.header("Authorization", format!("Bearer {}", token_str));
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        anyhow::bail!(
            "GitHub releases API request failed with status: {}",
            response.status()
        );
    }

    let releases: Vec<GithubRelease> = response.json().await?;
    Ok(releases)
}

/// Create a pull request on GitHub
///
/// # Arguments
/// * `owner` - Repository owner/organization
/// * `repo` - Repository name
/// * `title` - PR title
/// * `body` - PR description/body
/// * `head` - Branch name containing the changes (e.g., "update/foo-1.2.3")
/// * `base` - Target branch to merge into (e.g., "main" or "master")
/// * `token` - GitHub personal access token for authentication
///
/// # Returns
/// The created pull request information (URL and number)
pub async fn create_pull_request(
    owner: &str,
    repo: &str,
    title: &str,
    body: &str,
    head: &str,
    base: &str,
    token: &str,
) -> anyhow::Result<GithubPullRequest> {
    let url = format!("https://api.github.com/repos/{}/{}/pulls", owner, repo);

    debug!("Creating PR at {}", url);

    let client = reqwest::Client::new();
    let request_body = serde_json::json!({
        "title": title,
        "body": body,
        "head": head,
        "base": base,
    });

    let response = client
        .post(&url)
        .header("User-Agent", "ekapkgs-update")
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("Authorization", format!("Bearer {}", token))
        .json(&request_body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await?;
        anyhow::bail!(
            "GitHub PR creation failed with status {}: {}",
            status,
            error_text
        );
    }

    let pr: GithubPullRequest = response.json().await?;
    debug!("Created PR #{}: {}", pr.number, pr.html_url);

    Ok(pr)
}

#[cfg(test)]
mod tests {
    use super::*;
    pub use crate::vcs_sources::extract_version_from_tag;

    #[test]
    fn test_parse_github_url_https() {
        let url = "https://github.com/owner/repo";
        let result = parse_github_url(url);
        assert!(result.is_some());
        let repo = result.unwrap();
        assert_eq!(repo.owner, "owner");
        assert_eq!(repo.repo, "repo");
    }

    #[test]
    fn test_parse_github_url_git() {
        let url = "git@github.com:owner/repo.git";
        let result = parse_github_url(url);
        assert!(result.is_some());
        let repo = result.unwrap();
        assert_eq!(repo.owner, "owner");
        assert_eq!(repo.repo, "repo");
    }

    #[test]
    fn test_parse_github_url_with_path() {
        let url = "https://github.com/owner/repo/archive/v1.0.0.tar.gz";
        let result = parse_github_url(url);
        assert!(result.is_some());
        let repo = result.unwrap();
        assert_eq!(repo.owner, "owner");
        assert_eq!(repo.repo, "repo");
    }

    #[test]
    fn test_parse_github_url_invalid() {
        let url = "https://gitlab.com/owner/repo";
        let result = parse_github_url(url);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_version_from_tag_v_prefix() {
        assert_eq!(extract_version_from_tag("v1.0.0"), "1.0.0");
        assert_eq!(extract_version_from_tag("v2.3.4"), "2.3.4");
    }

    #[test]
    fn test_extract_version_from_tag_release_prefix() {
        assert_eq!(extract_version_from_tag("release-1.0.0"), "1.0.0");
        assert_eq!(extract_version_from_tag("release-2.3.4"), "2.3.4");
    }

    #[test]
    fn test_extract_version_from_tag_version_prefix() {
        assert_eq!(extract_version_from_tag("version-1.2.3"), "1.2.3");
        assert_eq!(extract_version_from_tag("version-4.5.6"), "4.5.6");
    }

    #[test]
    fn test_extract_version_from_tag_no_prefix() {
        assert_eq!(extract_version_from_tag("1.0.0"), "1.0.0");
        assert_eq!(extract_version_from_tag("2.3.4"), "2.3.4");
    }

    #[test]
    fn test_extract_version_from_tag_complex_prefix() {
        assert_eq!(extract_version_from_tag("foo-bar-1.0.0"), "1.0.0");
        assert_eq!(extract_version_from_tag("myapp-v2.3.4"), "2.3.4");
    }

    #[test]
    fn test_extract_version_from_tag_no_digit() {
        assert_eq!(extract_version_from_tag("latest"), "latest");
        assert_eq!(extract_version_from_tag("main"), "main");
    }
}
