//! SourceHut API integration and utilities

use std::sync::OnceLock;

use anyhow::Context;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// SourceHut GraphQL tag/reference information
#[derive(Debug, Deserialize)]
pub struct SourcehutReference {
    pub name: String,
}

/// Response from the references query
#[derive(Debug, Deserialize)]
struct ReferencesData {
    results: Vec<SourcehutReference>,
}

/// Repository data from the GraphQL API
#[derive(Debug, Deserialize)]
struct RepositoryData {
    references: ReferencesData,
}

/// Top-level GraphQL response
#[derive(Debug, Deserialize)]
struct GraphqlResponse {
    data: Option<ResponseData>,
}

/// Data field in the GraphQL response
#[derive(Debug, Deserialize)]
struct ResponseData {
    repository: Option<RepositoryData>,
}

/// GraphQL query variables
#[derive(Debug, Serialize)]
struct QueryVariables {
    owner: String,
    name: String,
}

/// GraphQL query payload
#[derive(Debug, Serialize)]
struct GraphqlQuery {
    query: String,
    variables: QueryVariables,
}

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

    debug!("Fetching tags from SourceHut for {}/{}", owner, repo);

    // GraphQL query to fetch all references
    let query = r#"
        query($owner: String!, $name: String!) {
            repository(owner: $owner, name: $name) {
                references {
                    results {
                        name
                    }
                }
            }
        }
    "#
    .to_string();

    let graphql_query = GraphqlQuery {
        query,
        variables: QueryVariables {
            owner: owner.to_owned(),
            name: repo.to_owned(),
        },
    };

    let client = reqwest::Client::new();
    let mut request = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&graphql_query);

    // Add authorization header if token is provided
    if let Some(token_str) = token {
        request = request.header("Authorization", format!("Bearer {token_str}"));
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| String::from("(unable to read response)"));
        anyhow::bail!("SourceHut GraphQL API request failed with status {status}: {error_text}");
    }

    let graphql_response: GraphqlResponse = response
        .json()
        .await
        .with_context(|| format!("decode JSON from {url}"))?;

    let repository_data = graphql_response
        .data
        .and_then(|d| d.repository)
        .with_context(|| format!("Repository {owner}/{repo} not found"))?;

    // Filter for tags (references starting with "refs/tags/")
    let tags: Vec<SourcehutReference> = repository_data
        .references
        .results
        .into_iter()
        .filter_map(|reference| {
            if let Some(tag_name) = reference.name.strip_prefix("refs/tags/") {
                Some(SourcehutReference {
                    name: tag_name.to_owned(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(tags)
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

    #[test]
    fn test_parse_sourcehut_url_gitlab() {
        let url = "https://gitlab.com/owner/repo";
        let result = parse_sourcehut_url(url);
        assert!(result.is_none());
    }
}
