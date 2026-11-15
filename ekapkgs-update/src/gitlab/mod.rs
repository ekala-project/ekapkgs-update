//! GitLab API integration and utilities

use std::sync::OnceLock;

use anyhow::Context;
use regex::Regex;
use serde::Deserialize;

use crate::vcs_sources::HttpStatusError;
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
    /// GitLab instance domain (e.g., "gitlab.com", "gitlab.freedesktop.org")
    pub instance: String,
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
/// Known GitLab instances beyond gitlab.com
const KNOWN_GITLAB_INSTANCES: &[&str] = &[
    "gitlab.com",
    "gitlab.freedesktop.org",
    "gitlab.gnome.org",
    "gitlab.alpinelinux.org",
    "gitlab.archlinux.org",
    "invent.kde.org",
    "salsa.debian.org",
];

pub fn parse_gitlab_url(url: &str) -> Option<GitlabProject> {
    // Match any known GitLab instance
    static GITLAB_URL_REGEX: OnceLock<Regex> = OnceLock::new();
    let gitlab_regex = GITLAB_URL_REGEX.get_or_init(|| {
        let instances = KNOWN_GITLAB_INSTANCES
            .iter()
            .map(|i| regex::escape(i))
            .collect::<Vec<_>>()
            .join("|");
        Regex::new(&format!(r"(?:^|[/@])({instances})[:/]([^?#]+)"))
            .expect("GitLab URL regex must compile")
    });
    let caps = gitlab_regex.captures(url)?;
    let instance = caps.get(1)?.as_str();
    let path = project_path(caps.get(2)?.as_str())?;
    let (owner, project) = path.split_once('/')?;
    if owner.is_empty() || project.is_empty() {
        return None;
    }

    Some(GitlabProject {
        instance: instance.to_owned(),
        owner: owner.to_owned(),
        project: project.to_owned(),
    })
}

/// The full project path (`group/subgroup/project`) from the part of a
/// GitLab URL after the instance.
///
/// - API URLs (`fetchFromGitLab`): `api/v4/projects/<url-encoded path>/...`
/// - Web URLs: everything before `/-/`, or up to a `.git` segment
/// - Otherwise the first two segments: without a `/-/` or `.git` marker a
///   subgroup cannot be told from a path inside the project
///   (`owner/project/repository/archive.tar.gz`).
fn project_path(rest: &str) -> Option<String> {
    if let Some(api) = rest.strip_prefix("api/v4/projects/") {
        let encoded = api.split('/').next()?;
        return Some(percent_decode(encoded)).filter(|p| p.contains('/'));
    }
    if let Some((path, _)) = rest.split_once("/-/") {
        return Some(path.trim_end_matches(".git").to_owned());
    }
    let segments: Vec<&str> = rest.split('/').collect();
    if let Some(i) = segments.iter().position(|s| s.ends_with(".git")) {
        let mut path = segments[..=i].join("/");
        path.truncate(path.len() - ".git".len());
        return Some(path);
    }
    match segments.as_slice() {
        [owner, project, ..] => Some(format!("{owner}/{project}")),
        _ => None,
    }
}

/// Decode `%XX` escapes (GitLab API project paths encode `/` as `%2F`).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            },
            (b, _) => {
                out.push(b);
                i += 1;
            },
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The URL-encoded project path for API requests (`group%2Fsub%2Fproject`).
fn encoded_project_path(owner: &str, project: &str) -> String {
    format!("{owner}/{project}").replace('/', "%2F")
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
    instance: &str,
    owner: &str,
    project: &str,
    token: Option<&str>,
) -> anyhow::Result<Vec<GitlabTag>> {
    let encoded_path = encoded_project_path(owner, project);
    let url = format!(
        "https://{instance}/api/v4/projects/{encoded_path}/repository/tags?order_by=updated&sort=desc"
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
        return Err(HttpStatusError::new("GitLab tags API request", response.status()).into());
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
    instance: &str,
    owner: &str,
    project: &str,
    token: Option<&str>,
) -> anyhow::Result<Vec<GitlabRelease>> {
    let encoded_path = encoded_project_path(owner, project);
    let url = format!("https://{instance}/api/v4/projects/{encoded_path}/releases");

    debug!("Fetching all releases from {}", url);

    let client = reqwest::Client::new();
    let mut request = client.get(&url).header("User-Agent", "ekapkgs-update");

    // Add authorization header if token is provided
    if let Some(token_str) = token {
        request = request.header("PRIVATE-TOKEN", token_str);
    }

    let response = request.send().await.with_context(|| format!("GET {url}"))?;

    if !response.status().is_success() {
        return Err(HttpStatusError::new("GitLab releases API request", response.status()).into());
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
    fn test_parse_gitlab_url_cases() {
        let cases = [
            // fetchFromGitLab URLs go through the API
            (
                "https://gitlab.freedesktop.org/api/v4/projects/xorg%2Flib%2Flibx11/repository/archive.tar.gz?sha=abc",
                Some(("gitlab.freedesktop.org", "xorg", "lib/libx11")),
            ),
            (
                "https://gitlab.gnome.org/api/v4/projects/World%2Fgedit%2Fgedit/repository/archive.tar.gz?sha=46.2",
                Some(("gitlab.gnome.org", "World", "gedit/gedit")),
            ),
            (
                "https://salsa.debian.org/api/v4/projects/debian%2Fascii/repository/archive.tar.gz?sha=v3.32",
                Some(("salsa.debian.org", "debian", "ascii")),
            ),
            // a numeric project id carries no path
            (
                "https://gitlab.com/api/v4/projects/12345/repository/archive.tar.gz",
                None,
            ),
            (
                "https://gitlab.freedesktop.org/xorg/lib/libx11/-/archive/v1.8/libx11-v1.8.tar.gz",
                Some(("gitlab.freedesktop.org", "xorg", "lib/libx11")),
            ),
            (
                "https://invent.kde.org/plasma/kwin.git",
                Some(("invent.kde.org", "plasma", "kwin")),
            ),
            (
                "https://gitlab.gnome.org/GNOME/gtk/repository/archive.tar.gz",
                Some(("gitlab.gnome.org", "GNOME", "gtk")),
            ),
            ("https://notgitlab.com/owner/project", None),
            ("https://gitlab.com/owner", None),
        ];
        for (url, expected) in cases {
            let got = parse_gitlab_url(url);
            let got = got
                .as_ref()
                .map(|p| (p.instance.as_str(), p.owner.as_str(), p.project.as_str()));
            assert_eq!(got, expected, "{url}");
        }
    }

    #[test]
    fn encoded_project_path_cases() {
        assert_eq!(encoded_project_path("mesa", "mesa"), "mesa%2Fmesa");
        assert_eq!(
            encoded_project_path("xorg", "lib/libx11"),
            "xorg%2Flib%2Flibx11"
        );
    }

    #[test]
    fn test_parse_gitlab_url_invalid() {
        let url = "https://github.com/owner/repo";
        let result = parse_gitlab_url(url);
        assert!(result.is_none());
    }
}
