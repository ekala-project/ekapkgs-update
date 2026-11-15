//! SourceHut API integration and utilities

use std::sync::OnceLock;

use anyhow::Context;
use regex::Regex;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::vcs_sources::HttpStatusError;

/// SourceHut GraphQL tag/reference information
#[derive(Debug, Deserialize)]
pub struct SourcehutReference {
    pub name: String,
}

/// One page of a repository's references.
#[derive(Debug, Deserialize)]
struct ReferenceCursor {
    results: Vec<SourcehutReference>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RepositoryData {
    references: ReferenceCursor,
}

#[derive(Debug, Deserialize)]
struct UserData {
    repository: Option<RepositoryData>,
}

#[derive(Debug, Deserialize)]
struct ResponseData {
    user: Option<UserData>,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    message: String,
}

/// Top-level GraphQL response
#[derive(Debug, Deserialize)]
struct GraphqlResponse {
    data: Option<ResponseData>,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

/// GraphQL query variables
#[derive(Debug, Serialize)]
struct QueryVariables<'a> {
    username: &'a str,
    name: &'a str,
    cursor: Option<&'a str>,
}

/// GraphQL query payload
#[derive(Debug, Serialize)]
struct GraphqlQuery<'a> {
    query: &'static str,
    variables: QueryVariables<'a>,
}

/// git.sr.ht looks repositories up by owner and name through `user`; the
/// top-level `repository` field only takes a resource ID.
const REFERENCES_QUERY: &str = r"
    query($username: String!, $name: String!, $cursor: Cursor) {
        user(username: $username) {
            repository(name: $name) {
                references(cursor: $cursor) {
                    results { name }
                    cursor
                }
            }
        }
    }
";

/// Upper bound on reference pages fetched per repository.
const MAX_REFERENCE_PAGES: usize = 20;

/// Represents a SourceHut repository with owner and name
#[derive(Debug)]
pub struct SourcehutRepo {
    pub owner: String,
    pub repo: String,
}

/// Parse SourceHut URL to extract owner and repo
///
/// Supports various SourceHut URL formats:
/// - HTTPS: `https://git.sr.ht/~owner/repo`
/// - SSH: `git@git.sr.ht:~owner/repo`
/// - With paths: `https://git.sr.ht/~owner/repo/archive/v1.0.0.tar.gz`
///
/// # Arguments
/// * `url` - SourceHut URL to parse
///
/// # Returns
/// `Some(SourcehutRepo)` if the URL is a valid SourceHut URL, `None` otherwise
///
/// # Example
/// ```
/// use ekapkgs_update::sourcehut::parse_sourcehut_url;
///
/// let repo = parse_sourcehut_url("https://git.sr.ht/~owner/repo").unwrap();
/// assert_eq!(repo.owner, "~owner");
/// assert_eq!(repo.repo, "repo");
/// ```
pub fn parse_sourcehut_url(url: &str) -> Option<SourcehutRepo> {
    static SOURCEHUT_URL_REGEX: OnceLock<Regex> = OnceLock::new();
    let sourcehut_regex = SOURCEHUT_URL_REGEX.get_or_init(|| {
        Regex::new(r"git\.sr\.ht[:/](~[^/]+)/([^/]+?)(?:\.git|/|$)")
            .expect("hard-coded SourceHut URL regex must compile")
    });
    let caps = sourcehut_regex.captures(url)?;

    Some(SourcehutRepo {
        owner: caps.get(1)?.as_str().to_owned(),
        repo: caps.get(2)?.as_str().to_owned(),
    })
}

/// Fetch references (tags) from SourceHut GraphQL API
///
/// Retrieves all references from a repository and filters for tags.
/// Tags are references that start with "refs/tags/".
///
/// # Arguments
/// * `owner` - Repository owner (with ~ prefix, e.g., "~username")
/// * `repo` - Repository name
/// * `token` - Optional SourceHut personal access token for authentication
///
/// # Returns
/// A vector of tag names (without the "refs/tags/" prefix)
pub async fn fetch_sourcehut_tags(
    owner: &str,
    repo: &str,
    token: Option<&str>,
) -> anyhow::Result<Vec<SourcehutReference>> {
    let url = "https://git.sr.ht/query";
    let username = owner.trim_start_matches('~');

    debug!("Fetching tags from SourceHut for ~{}/{}", username, repo);

    let client = reqwest::Client::new();
    let mut tags = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_REFERENCE_PAGES {
        let mut request = client.post(url).json(&GraphqlQuery {
            query: REFERENCES_QUERY,
            variables: QueryVariables {
                username,
                name: repo,
                cursor: cursor.as_deref(),
            },
        });
        if let Some(token_str) = token {
            request = request.header("Authorization", format!("Bearer {token_str}"));
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("(unable to read response)"));
            return Err(anyhow::Error::new(HttpStatusError::new(
                "SourceHut GraphQL API request",
                status,
            ))
            .context(error_text));
        }

        let body: GraphqlResponse = response
            .json()
            .await
            .with_context(|| format!("decode JSON from {url}"))?;
        let page = references_page(body, username, repo)?;
        tags.extend(page.results.into_iter().filter_map(|reference| {
            reference
                .name
                .strip_prefix("refs/tags/")
                .map(|tag_name| SourcehutReference {
                    name: tag_name.to_owned(),
                })
        }));
        cursor = page.cursor;
        if cursor.is_none() {
            break;
        }
    }

    Ok(tags)
}

/// The references page in a GraphQL response. A missing user or repository is
/// reported as a 404 so callers can tell it from a transient failure.
fn references_page(
    body: GraphqlResponse,
    username: &str,
    repo: &str,
) -> anyhow::Result<ReferenceCursor> {
    if !body.errors.is_empty() {
        let messages: Vec<&str> = body.errors.iter().map(|e| e.message.as_str()).collect();
        anyhow::bail!("SourceHut GraphQL errors: {}", messages.join("; "));
    }
    body.data
        .and_then(|d| d.user)
        .and_then(|u| u.repository)
        .map(|r| r.references)
        .ok_or_else(|| {
            anyhow::Error::new(HttpStatusError::new(
                "SourceHut GraphQL API request",
                StatusCode::NOT_FOUND,
            ))
            .context(format!("SourceHut repository ~{username}/{repo} not found"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper for testing URL parsing with expected owner/repo values.
    ///
    /// Asserts that the URL parses successfully and returns the parsed repo
    /// for further assertions.
    fn assert_parses_to(url: &str, expected_owner: &str, expected_repo: &str) {
        let repo = parse_sourcehut_url(url).expect("URL should parse successfully");
        assert_eq!(repo.owner, expected_owner);
        assert_eq!(repo.repo, expected_repo);
    }

    #[test]
    fn test_parse_sourcehut_url_https() {
        assert_parses_to("https://git.sr.ht/~owner/repo", "~owner", "repo");
    }

    #[test]
    fn test_parse_sourcehut_url_ssh() {
        assert_parses_to("git@git.sr.ht:~owner/repo.git", "~owner", "repo");
    }

    #[test]
    fn test_parse_sourcehut_url_with_path() {
        assert_parses_to(
            "https://git.sr.ht/~owner/repo/archive/v1.0.0.tar.gz",
            "~owner",
            "repo",
        );
    }

    #[test]
    fn test_parse_sourcehut_url_invalid() {
        let url = "https://github.com/owner/repo";
        let result = parse_sourcehut_url(url);
        assert!(result.is_none());
    }

    fn page(json: &str) -> anyhow::Result<ReferenceCursor> {
        references_page(serde_json::from_str(json).unwrap(), "owner", "repo")
    }

    #[test]
    fn references_page_cases() {
        let ok = page(
            r#"{"data":{"user":{"repository":{"references":
                {"results":[{"name":"refs/tags/v1"}],"cursor":"abc"}}}}}"#,
        )
        .unwrap();
        assert_eq!(ok.results.len(), 1);
        assert_eq!(ok.cursor.as_deref(), Some("abc"));

        for missing in [
            r#"{"data":{"user":null}}"#,
            r#"{"data":{"user":{"repository":null}}}"#,
        ] {
            let err = page(missing).unwrap_err();
            assert!(
                HttpStatusError::find(&err).is_some_and(HttpStatusError::is_not_found),
                "{missing}: {err:#}"
            );
        }

        let err = page(r#"{"errors":[{"message":"Unknown argument"}],"data":null}"#).unwrap_err();
        assert!(HttpStatusError::find(&err).is_none());
        assert!(format!("{err:#}").contains("Unknown argument"));
    }

    #[test]
    fn test_parse_sourcehut_url_gitlab() {
        let url = "https://gitlab.com/owner/repo";
        let result = parse_sourcehut_url(url);
        assert!(result.is_none());
    }
}
