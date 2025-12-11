//! GitHub API integration and utilities

use regex::Regex;
use semver::Version;
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
struct GithubTag {
    name: String,
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
/// Internal helper function to retrieve all tags from a repository.
/// Tags are returned in reverse chronological order (newest first).
///
/// # Arguments
/// * `owner` - Repository owner/organization
/// * `repo` - Repository name
/// * `token` - Optional GitHub personal access token for authentication
///
/// # Returns
/// A vector of tags, or an empty vector if no tags exist
async fn fetch_github_tags(
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

/// Fetch latest release from GitHub API
///
/// Makes an API call to GitHub to retrieve the latest release for a given repository.
/// Optionally authenticates with a GitHub token for higher rate limits.
///
/// # Arguments
/// * `owner` - Repository owner/organization
/// * `repo` - Repository name
/// * `token` - Optional GitHub personal access token for authentication
///
/// # Returns
/// The latest release information from GitHub
///
/// # Rate Limits
/// - **Without token**: 60 requests per hour
/// - **With token**: 5,000 requests per hour
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
/// use ekapkgs_update::github::fetch_latest_github_release;
///
/// # async fn example() -> anyhow::Result<()> {
/// // With authentication
/// let token = std::env::var("GITHUB_TOKEN").ok();
/// let release = fetch_latest_github_release("owner", "repo", token.as_deref()).await?;
/// println!("Latest version: {}", release.tag_name);
///
/// // Without authentication (lower rate limits)
/// let release = fetch_latest_github_release("owner", "repo", None).await?;
/// # Ok(())
/// # }
/// ```
pub async fn fetch_latest_github_release(
    owner: &str,
    repo: &str,
    token: Option<&str>,
) -> anyhow::Result<GithubRelease> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        owner, repo
    );

    debug!("Fetching latest release from {}", url);

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

    // If releases endpoint returns 404, fallback to tags
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        debug!(
            "No releases found for {}/{}, falling back to tags",
            owner, repo
        );
        let tags = fetch_github_tags(owner, repo, token).await?;

        if let Some(first_tag) = tags.first() {
            let version = extract_version_from_tag(&first_tag.name);
            debug!(
                "Using latest tag: {} (extracted version: {})",
                first_tag.name, version
            );
            return Ok(GithubRelease {
                tag_name: version.to_string(),
                _name: None,
                prerelease: false,
            });
        } else {
            anyhow::bail!("No releases or tags found for {}/{}", owner, repo);
        }
    }

    if !response.status().is_success() {
        anyhow::bail!(
            "GitHub API request failed with status: {}",
            response.status()
        );
    }

    let release: GithubRelease = response.json().await?;
    Ok(release)
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
/// use ekapkgs_update::github::extract_version_from_tag;
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
    fn test_is_version_newer_semver() {
        assert!(is_version_newer("1.0.0", "1.0.1").unwrap());
        assert!(is_version_newer("1.0.0", "2.0.0").unwrap());
        assert!(!is_version_newer("2.0.0", "1.0.0").unwrap());
        assert!(!is_version_newer("1.0.0", "1.0.0").unwrap());
    }

    #[test]
    fn test_is_version_newer_with_v_prefix() {
        assert!(is_version_newer("v1.0.0", "v1.0.1").unwrap());
        assert!(is_version_newer("1.0.0", "v1.0.1").unwrap());
        assert!(is_version_newer("v1.0.0", "1.0.1").unwrap());
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
