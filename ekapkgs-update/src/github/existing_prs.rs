//! Scan a repository's open pull requests for an existing update of a package
//! before proposing a new one.
//!
//! Only PRs opened by the configured author (`--pr-author` / `[github]
//! pr_author`, else the GitHub App's bot, else the token's user) count, so a contributor's PR
//! for the same package never suppresses ours, and ours are found whatever
//! branch they were pushed from (another fork, another deployment, an older
//! branch naming scheme).

use std::cmp::Ordering;
use std::sync::{Mutex, OnceLock};

use anyhow::Context;
use reqwest::Client;
use serde::Deserialize;
use tracing::warn;

use super::github_request_builder;
use crate::vcs_sources::compare_version_components;

/// Author configured on the command line or in the config file.
static CONFIGURED_AUTHOR: OnceLock<String> = OnceLock::new();
/// Login of the token's user, looked up once when no author is configured
/// (`None` inside: the lookup failed, don't retry).
static TOKEN_AUTHOR: Mutex<Option<Option<String>>> = Mutex::new(None);

/// Set the PR author for this process. The first call wins; empty values are
/// ignored.
pub fn set_pr_author(author: Option<String>) {
    if let Some(author) = author.map(|a| a.trim().to_owned()) {
        if !author.is_empty() {
            let _ = CONFIGURED_AUTHOR.set(author);
        }
    }
}

/// The author whose PRs count as ours: the configured one, else the bot of
/// the configured GitHub App (`<slug>[bot]`, PRs are opened as it), else the
/// login of the token's user (`GET /user`). `None` when none is known.
pub async fn pr_author(token: &str) -> Option<String> {
    if let Some(author) = CONFIGURED_AUTHOR.get() {
        return Some(author.clone());
    }
    if let Some(app) = super::auth::app() {
        return match app.bot_identity().await {
            Ok(identity) => Some(identity.name),
            Err(e) => {
                warn!("Could not determine the GitHub App's bot login ({e:#}); set --pr-author");
                None
            },
        };
    }
    if let Some(cached) = TOKEN_AUTHOR.lock().ok().and_then(|g| g.clone()) {
        return cached;
    }
    let login = match token_login(token).await {
        Ok(login) => Some(login),
        Err(e) => {
            warn!(
                "Could not determine the GitHub user of the token ({e:#}); set --pr-author \
                 (or [github] pr_author) to scan for existing PRs"
            );
            None
        },
    };
    if let Ok(mut guard) = TOKEN_AUTHOR.lock() {
        *guard = Some(login.clone());
    }
    login
}

#[derive(Debug, Deserialize)]
struct GithubUser {
    login: String,
}

async fn token_login(token: &str) -> anyhow::Result<String> {
    let url = "https://api.github.com/user";
    let client = Client::new();
    let response = github_request_builder(&client, reqwest::Method::GET, url, Some(token))
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !response.status().is_success() {
        anyhow::bail!("GET {url} failed with status {}", response.status());
    }
    let user: GithubUser = response
        .json()
        .await
        .with_context(|| format!("decode JSON from {url}"))?;
    Ok(user.login)
}

/// An open PR found by the scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPr {
    pub number: i64,
    pub html_url: String,
    pub title: String,
    pub author: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    items: Vec<SearchItem>,
}

#[derive(Debug, Deserialize)]
struct SearchItem {
    number: i64,
    html_url: String,
    title: String,
    user: GithubUser,
    /// Present only for pull requests (the search covers issues too).
    pull_request: Option<serde_json::Value>,
}

/// Open PRs of `owner/repo` by `author` whose title mentions `attr_path`
/// (search API; the caller matches titles exactly with [`decide`]).
pub async fn search_open_update_prs(
    owner: &str,
    repo: &str,
    attr_path: &str,
    author: &str,
    token: &str,
) -> anyhow::Result<Vec<OpenPr>> {
    let url = "https://api.github.com/search/issues";
    let query = search_query(owner, repo, attr_path, author);
    let client = Client::new();
    let response = github_request_builder(&client, reqwest::Method::GET, url, Some(token))
        .query(&[("q", query.as_str()), ("per_page", "100")])
        .send()
        .await
        .with_context(|| format!("GET {url}?q={query}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .with_context(|| format!("read error body from {url}"))?;
        anyhow::bail!("GitHub PR search failed with status {status}: {body}");
    }
    let result: SearchResponse = response
        .json()
        .await
        .with_context(|| format!("decode JSON from {url}"))?;
    Ok(result
        .items
        .into_iter()
        .filter(|item| item.pull_request.is_some())
        .map(|item| OpenPr {
            number: item.number,
            html_url: item.html_url,
            title: item.title,
            author: item.user.login,
        })
        .collect())
}

fn search_query(owner: &str, repo: &str, attr_path: &str, author: &str) -> String {
    // Quotes inside a phrase can't be escaped in search syntax (`"389-ds-base"`
    // attr segments); the exact match happens afterwards anyway.
    let phrase = attr_path.replace('"', "");
    format!("repo:{owner}/{repo} is:pr is:open author:{author} in:title \"{phrase}\"")
}

/// Whether `login` (from the API) is `author` (as configured). GitHub Apps are
/// searched as `app/<name>` but their PRs are authored by `<name>[bot]`.
fn is_author(login: &str, author: &str) -> bool {
    let author = author.strip_prefix("app/").unwrap_or(author);
    let login = login.strip_suffix("[bot]").unwrap_or(login);
    let author = author.strip_suffix("[bot]").unwrap_or(author);
    login.eq_ignore_ascii_case(author)
}

/// The version an update title (`<attr>: <old> -> <new>`, see
/// [`crate::git::update_title`]) proposes for `attr_path`.
fn proposed_version<'a>(title: &'a str, attr_path: &str) -> Option<&'a str> {
    let rest = title.strip_prefix(attr_path)?.strip_prefix(": ")?;
    let (_, new) = rest.rsplit_once(" -> ")?;
    let new = new.split_whitespace().next()?;
    Some(new)
}

/// What to do about our open PRs for a package.
#[derive(Debug, PartialEq, Eq)]
pub enum ExistingPrs {
    /// None of ours.
    None,
    /// One already proposes this version or a newer one: don't open another.
    Covered(OpenPr),
    /// Ours propose older versions only; the new PR supersedes them.
    Superseded(Vec<OpenPr>),
}

impl ExistingPrs {
    /// Numbers of the superseded PRs (empty unless [`ExistingPrs::Superseded`]).
    #[must_use]
    pub fn superseded_numbers(&self) -> Vec<i64> {
        match self {
            Self::Superseded(prs) => prs.iter().map(|pr| pr.number).collect(),
            Self::None | Self::Covered(_) => Vec::new(),
        }
    }
}

/// Match `prs` (by `author`, titled as an update of exactly `attr_path`)
/// against the version about to be proposed.
pub fn decide(prs: &[OpenPr], attr_path: &str, new_version: &str, author: &str) -> ExistingPrs {
    let mut covered: Option<(&OpenPr, &str)> = None;
    let mut older = Vec::new();
    for pr in prs.iter().filter(|pr| is_author(&pr.author, author)) {
        let Some(version) = proposed_version(&pr.title, attr_path) else {
            continue;
        };
        if compare_version_components(version, new_version) == Ordering::Less {
            older.push(pr);
        } else if covered.is_none_or(|(_, v)| compare_version_components(version, v).is_gt()) {
            covered = Some((pr, version));
        }
    }
    match covered {
        Some((pr, _)) => ExistingPrs::Covered(pr.clone()),
        None if older.is_empty() => ExistingPrs::None,
        None => ExistingPrs::Superseded(older.into_iter().cloned().collect()),
    }
}

/// Scan `owner/repo` for our open PRs updating `attr_path` before proposing
/// `new_version`. Failures (no author known, search errors or rate limits)
/// are logged and treated as [`ExistingPrs::None`], so a PR is still opened.
pub async fn scan(
    owner: &str,
    repo: &str,
    attr_path: &str,
    new_version: &str,
    token: &str,
) -> ExistingPrs {
    let Some(author) = pr_author(token).await else {
        return ExistingPrs::None;
    };
    match search_open_update_prs(owner, repo, attr_path, &author, token).await {
        Ok(prs) => decide(&prs, attr_path, new_version, &author),
        Err(e) => {
            warn!("{attr_path}: scan for existing PRs failed: {e:#}");
            ExistingPrs::None
        },
    }
}

/// `Supersedes #1, #2` for older open update PRs of the same package.
#[must_use]
pub fn supersedes_section(numbers: &[i64]) -> String {
    if numbers.is_empty() {
        return String::new();
    }
    let list = numbers
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("\n\nSupersedes {list}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: i64, title: &str, author: &str) -> OpenPr {
        OpenPr {
            number,
            html_url: format!("https://github.com/o/r/pull/{number}"),
            title: title.to_owned(),
            author: author.to_owned(),
        }
    }

    #[test]
    fn proposed_version_cases() {
        let cases = [
            ("foo: 1.0 -> 1.1", "foo", Some("1.1")),
            (
                "python3Packages.foo: 1.0 -> 2.0",
                "python3Packages.foo",
                Some("2.0"),
            ),
            ("python3Packages.foo: 1.0 -> 2.0", "foo", None),
            ("foo-cli: 1.0 -> 1.1", "foo", None),
            ("foo: 1.0 -> 1.1 (draft)", "foo", Some("1.1")),
            ("foo: fix build", "foo", None),
            ("foo: 1.0 -> 1.1", "foo.bar", None),
        ];
        for (title, attr, expected) in cases {
            assert_eq!(proposed_version(title, attr), expected, "{title} / {attr}");
        }
    }

    #[test]
    fn is_author_cases() {
        let cases = [
            ("ekapkgs-bot", "ekapkgs-bot", true),
            ("EkaPkgs-Bot", "ekapkgs-bot", true),
            ("someone", "ekapkgs-bot", false),
            ("ekapkgs[bot]", "app/ekapkgs", true),
            ("ekapkgs[bot]", "ekapkgs[bot]", true),
        ];
        for (login, author, expected) in cases {
            assert_eq!(is_author(login, author), expected, "{login} / {author}");
        }
    }

    #[test]
    fn decide_cases() {
        let prs = [
            pr(1, "foo: 1.0 -> 1.1", "bot"),
            pr(2, "foo: 1.0 -> 1.3", "human"),
            pr(3, "foo-cli: 1.0 -> 9.0", "bot"),
        ];
        assert_eq!(
            decide(&prs, "foo", "1.2", "bot"),
            ExistingPrs::Superseded(vec![prs[0].clone()])
        );
        assert_eq!(
            decide(&prs, "foo", "1.1", "bot"),
            ExistingPrs::Covered(prs[0].clone())
        );
        assert_eq!(
            decide(&prs, "foo", "1.0.9", "bot"),
            ExistingPrs::Covered(prs[0].clone())
        );
        // Someone else's PR never counts.
        assert_eq!(decide(&prs, "foo", "1.2", "other"), ExistingPrs::None);
        assert_eq!(decide(&prs, "bar", "1.2", "bot"), ExistingPrs::None);

        let newest = [
            pr(4, "foo: 1.0 -> 1.4", "bot"),
            pr(5, "foo: 1.0 -> 1.5", "bot"),
            pr(6, "foo: 1.0 -> 1.1", "bot"),
        ];
        assert_eq!(
            decide(&newest, "foo", "1.3", "bot"),
            ExistingPrs::Covered(newest[1].clone())
        );
    }

    #[test]
    fn supersedes_section_cases() {
        assert_eq!(supersedes_section(&[]), "");
        assert_eq!(supersedes_section(&[3]), "\n\nSupersedes #3");
        assert_eq!(supersedes_section(&[3, 7]), "\n\nSupersedes #3, #7");
    }

    #[test]
    fn search_query_strips_quotes() {
        assert_eq!(
            search_query("o", "r", "\"389-ds-base\"", "bot"),
            "repo:o/r is:pr is:open author:bot in:title \"389-ds-base\""
        );
    }
}
