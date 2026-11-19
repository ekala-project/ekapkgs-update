//! Which credentials GitHub requests and pushes use.
//!
//! With a GitHub App configured ([`configure`], from `[github_app]` in the
//! config file), API calls and git pushes use installation tokens and commits
//! are made as the app's bot. Otherwise `$GITHUB_TOKEN` (a personal access
//! token) is used as before, with git pushes left to the configured git
//! credential helper.

use std::sync::OnceLock;

use anyhow::Context;
use tracing::{debug, info, warn};

use super::app::GithubApp;
use crate::config::GithubAppConfig;

static APP: OnceLock<GithubApp> = OnceLock::new();

/// Load the GitHub App from the config (with environment overrides), if one
/// is configured. Call once at startup.
pub fn configure(config: &GithubAppConfig) -> anyhow::Result<()> {
    let client_id = env_or("EKAPKGS_GITHUB_APP_CLIENT_ID", config.client_id.as_deref());
    let key_file = env_or(
        "EKAPKGS_GITHUB_APP_KEY_FILE",
        config.private_key_file.as_deref(),
    );
    let (client_id, key_file) = match (client_id, key_file) {
        (None, None) => return Ok(()),
        (Some(id), Some(file)) => (id, file),
        _ => anyhow::bail!(
            "a GitHub App needs both github_app.client_id and github_app.private_key_file"
        ),
    };
    let key_file = shellexpand::tilde(&key_file).to_string();
    let pem = std::fs::read(&key_file)
        .with_context(|| format!("read GitHub App private key {key_file}"))?;
    let app = GithubApp::new(&client_id, &pem)?;
    info!("Authenticating to GitHub as App {}", app.client_id());
    // A second configure (tests) keeps the first app.
    let _ = APP.set(app);
    Ok(())
}

/// The webhook secret (from `github_app.webhook_secret_file` or
/// `$EKAPKGS_GITHUB_WEBHOOK_SECRET_FILE`), trimmed of surrounding
/// whitespace. `None` when not configured.
pub fn webhook_secret(config: &GithubAppConfig) -> anyhow::Result<Option<Vec<u8>>> {
    let Some(file) = env_or(
        "EKAPKGS_GITHUB_WEBHOOK_SECRET_FILE",
        config.webhook_secret_file.as_deref(),
    ) else {
        return Ok(None);
    };
    let file = shellexpand::tilde(&file).to_string();
    let secret = std::fs::read_to_string(&file)
        .with_context(|| format!("read GitHub webhook secret {file}"))?;
    let secret = secret.trim();
    if secret.is_empty() {
        anyhow::bail!("GitHub webhook secret {file} is empty");
    }
    Ok(Some(secret.as_bytes().to_vec()))
}

fn env_or(var: &str, fallback: Option<&str>) -> Option<String> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| fallback.map(str::to_owned))
}

/// The configured GitHub App, if any.
pub fn app() -> Option<&'static GithubApp> {
    APP.get()
}

/// A token that can write to `owner/repo` (create PRs, reviews, forks): an
/// installation token of the app, else `$GITHUB_TOKEN`.
pub async fn token_for(owner: &str, repo: &str) -> anyhow::Result<String> {
    if let Some(app) = app() {
        return app.installation_token(owner, repo).await;
    }
    std::env::var("GITHUB_TOKEN")
        .context("no GitHub credentials: configure a GitHub App ([github_app]) or set GITHUB_TOKEN")
}

/// An optional token for reading public data (higher rate limits):
/// `$GITHUB_TOKEN`, else an installation token of the app, else none.
pub async fn read_token() -> Option<String> {
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        return Some(token);
    }
    let app = app()?;
    match app.any_installation_token().await {
        Ok(token) => Some(token),
        Err(e) => {
            warn!("Could not get a GitHub App token for reading: {e:#}");
            None
        },
    }
}

/// Environment for a `git commit` / `git push` to `owner/repo` with the app:
/// the bot's identity as author and committer, and a credential helper that
/// answers with an installation token (replacing any configured helper).
/// The token travels in an environment variable, never in argv or a URL.
/// Empty without an app (git's own configuration applies).
pub async fn git_env(owner: &str, repo: &str) -> anyhow::Result<Vec<(String, String)>> {
    let Some(app) = app() else {
        return Ok(Vec::new());
    };
    let token = app.installation_token(owner, repo).await?;
    let mut env = git_credential_env(&token);
    match app.bot_identity().await {
        Ok(identity) => {
            for role in ["AUTHOR", "COMMITTER"] {
                env.push((format!("GIT_{role}_NAME"), identity.name.clone()));
                env.push((format!("GIT_{role}_EMAIL"), identity.email.clone()));
            }
        },
        Err(e) => debug!("Committing with the configured git identity: {e:#}"),
    }
    Ok(env)
}

/// `GIT_CONFIG_*` variables installing a github.com credential helper that
/// prints `token` (read from `$EKAPKGS_GIT_TOKEN`). The empty first value
/// clears helpers from other config files.
fn git_credential_env(token: &str) -> Vec<(String, String)> {
    const KEY: &str = "credential.https://github.com.helper";
    const HELPER: &str = r#"!f() { test "$1" = get || exit 0; echo username=x-access-token; echo "password=$EKAPKGS_GIT_TOKEN"; }; f"#;
    [
        ("GIT_CONFIG_COUNT", "2"),
        ("GIT_CONFIG_KEY_0", KEY),
        ("GIT_CONFIG_VALUE_0", ""),
        ("GIT_CONFIG_KEY_1", KEY),
        ("GIT_CONFIG_VALUE_1", HELPER),
        ("EKAPKGS_GIT_TOKEN", token),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The helper answers `get` with the token from the environment, and
    /// nothing for `store` / `erase`. git runs `!<script>` helpers as
    /// `<script> <operation>`.
    #[test]
    fn credential_helper_answers_get_only() {
        let env = git_credential_env("ghs_secret");
        let helper = &env
            .iter()
            .find(|(k, _)| k == "GIT_CONFIG_VALUE_1")
            .unwrap()
            .1;
        let script = helper.strip_prefix('!').unwrap();
        let run = |op: &str| {
            let out = std::process::Command::new("sh")
                .args(["-c", &format!("{script} {op}")])
                .env("EKAPKGS_GIT_TOKEN", "ghs_secret")
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        assert_eq!(run("get"), "username=x-access-token\npassword=ghs_secret\n");
        assert_eq!(run("store"), "");
        assert_eq!(run("erase"), "");
        // The token itself is only in EKAPKGS_GIT_TOKEN, not in git config.
        assert!(
            !env.iter()
                .any(|(k, v)| k.starts_with("GIT_CONFIG") && v.contains("ghs_"))
        );
    }

    #[tokio::test]
    async fn without_an_app_git_is_left_alone() {
        if app().is_none() {
            assert!(git_env("o", "r").await.unwrap().is_empty());
        }
    }
}
