//! GitLab API integration and utilities

use std::sync::OnceLock;

use anyhow::Context;
use regex::Regex;
use serde::Deserialize;
use tracing::debug;

/// GitLab release information from the API
#[derive(Debug, Deserialize)]
pub struct GitlabRelease {
    pub tag_name: String,
    #[serde(default)]
    pub upcoming_release: bool,
}

/// Represents a GitLab project with owner/group and project name
#[derive(Debug)]
pub struct GitlabProject {
    pub owner: String,
    pub project: String,
}

/// GitLab tag information from the API
#[derive(Debug, Deserialize)]
pub struct GitlabTag {
    pub name: String,
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
    static GITLAB_URL_REGEX: OnceLock<Regex> = OnceLock::new();
    let gitlab_regex = GITLAB_URL_REGEX.get_or_init(|| {
        Regex::new(r"gitlab\.com[:/]([^/]+)/([^/]+?)(?:\.git|/-|/|$)")
            .expect("hard-coded GitLab URL regex must compile")
    });
    let caps = gitlab_regex.captures(url)?;

    Some(GitlabProject {
        owner: caps.get(1)?.as_str().to_owned(),
        project: caps.get(2)?.as_str().to_owned(),
    })
}

/// Fetch tags from GitLab API
///
/// Retrieves all tags from a repository.
/// Tags are returned in reverse chronological order (newest first).
///
/// # Arguments
/// * `owner` - Project owner/group
/// * `project` - Project name
/// * `token` - Optional GitLab personal access token for authentication
///
/// # Returns
/// A vector of tags, or an empty vector if no tags exist
pub async fn fetch_gitlab_tags(
    owner: &str,
    project: &str,
    token: Option<&str>,
) -> anyhow::Result<Vec<GitlabTag>> {
    let encoded_path = format!("{owner}%2F{project}");
    let url = format!(
        "https://gitlab.com/api/v4/projects/{encoded_path}/repository/tags?order_by=updated&sort=desc"
    );

    debug!("Fetching tags from {}", url);

    let client = reqwest::Client::new();
    let mut request = client.get(&url).header("User-Agent", "ekapkgs-update");

    // Add authorization header if token is provided
    if let Some(token_str) = token {
        request = request.header("PRIVATE-TOKEN", token_str);
    }

    let response = request.send().await.with_context(|| format!("GET {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!(
            "GitLab tags API request failed with status: {}",
            response.status()
        );
    }

    let tags: Vec<GitlabTag> = response
        .json()
        .await
        .with_context(|| format!("decode JSON from {url}"))?;
    Ok(tags)
}

/// Fetch all releases from GitLab API
///
/// Retrieves all releases from a project.
/// Releases are returned in reverse chronological order (newest first).
///
/// # Arguments
/// * `owner` - Project owner/group
/// * `project` - Project name
/// * `token` - Optional GitLab personal access token for authentication
///
/// # Returns
/// A vector of releases
pub async fn fetch_gitlab_releases(
    owner: &str,
    project: &str,
    token: Option<&str>,
) -> anyhow::Result<Vec<GitlabRelease>> {
    let encoded_path = format!("{owner}%2F{project}");
    let url = format!("https://gitlab.com/api/v4/projects/{encoded_path}/releases");

    debug!("Fetching all releases from {}", url);

    let client = reqwest::Client::new();
    let mut request = client.get(&url).header("User-Agent", "ekapkgs-update");

    // Add authorization header if token is provided
    if let Some(token_str) = token {
        request = request.header("PRIVATE-TOKEN", token_str);
    }

    let response = request.send().await.with_context(|| format!("GET {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!(
            "GitLab releases API request failed with status: {}",
            response.status()
        );
    }

    let releases: Vec<GitlabRelease> = response
        .json()
        .await
        .with_context(|| format!("decode JSON from {url}"))?;
    Ok(releases)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper for testing URL parsing with expected owner/project values.
    ///
    /// Asserts that the URL parses successfully and returns the parsed project
    /// for further assertions.
    fn assert_parses_to(url: &str, expected_owner: &str, expected_project: &str) {
        let project = parse_gitlab_url(url).expect("URL should parse successfully");
        assert_eq!(project.owner, expected_owner);
        assert_eq!(project.project, expected_project);
    }

    #[test]
    fn test_parse_gitlab_url_https() {
        assert_parses_to("https://gitlab.com/owner/project", "owner", "project");
    }

    #[test]
    fn test_parse_gitlab_url_git() {
        assert_parses_to("git@gitlab.com:owner/project.git", "owner", "project");
    }

    #[test]
    fn test_parse_gitlab_url_with_path() {
        assert_parses_to(
            "https://gitlab.com/owner/project/-/archive/v1.0.0/project-v1.0.0.tar.gz",
            "owner",
            "project",
        );
    }

    #[test]
    fn test_parse_gitlab_url_invalid() {
        let url = "https://github.com/owner/repo";
        let result = parse_gitlab_url(url);
        assert!(result.is_none());
    }
}
