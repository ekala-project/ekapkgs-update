//! Watch command: RSS/event-driven passive update mode.
//!
//! Instead of evaluating all packages via `nix-eval-jobs` on every run, the
//! watch command builds a reverse index mapping upstream sources to attr_paths,
//! then polls RSS/Atom feeds and APIs for new releases.  When a new release is
//! detected, it constructs an [`UpdateRequest`] and feeds it to the existing
//! updater pipeline.
//!
//! ## Multi-repo mode
//!
//! When `repos` is configured, the watch command maintains persistent checkouts
//! of each repository, builds per-repo upstream indexes, and routes update PRs
//! to the correct upstream repository. Before every (re-)index, managed
//! checkouts are fast-forwarded to their upstream base branch (see [`sync`]).
//!
//! ## Bootstrap (observe-only)
//!
//! The first time a package is observed, the best compatible upstream release
//! is recorded as its `baseline_version` and nothing is dispatched. Afterwards
//! only releases newer than the baseline trigger updates; the pre-existing
//! backlog is left to `ekapkgs-update run`.

pub mod feeds;
pub mod index;
mod listener;
mod sync;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde::Serialize;
use tracing::{info, warn};

use crate::cli::CommitStrategy;
use crate::commands::pr_enhancements::PrEnhancementsConfig;
use crate::config::RepoConfig;
use crate::database::{Database, RepoSyncStatus, WatchedRepoInfo};
use crate::git::PrConfig;
use crate::github::{GithubRepo, parse_github_url};

/// A repository with all runtime information resolved.
#[derive(Debug, Clone)]
pub struct ResolvedRepo {
    /// Human-readable name (from config).
    pub name: String,
    /// Absolute path to the Nix entry point file within the repo.
    pub eval_entry_point: String,
    /// Absolute path to the repo root directory.
    pub repo_dir: PathBuf,
    /// PR configuration (owner/repo/base_branch) for the upstream.
    pub pr_config: Option<PrConfig>,
    /// Git remote to push update branches to.
    pub fork: String,
    /// Upstream repository URL (multi-repo mode only).
    pub upstream_url: Option<String>,
    /// Whether watch owns the checkout and may fetch / fast-forward it.
    /// `false` in legacy single-repo mode (the current directory).
    pub managed: bool,
}

impl ResolvedRepo {
    /// Name used in log messages (`default` for legacy mode).
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            "default"
        } else {
            &self.name
        }
    }

    /// Record this repository's static configuration in `watched_repos`.
    async fn register(&self, db: &Database) -> anyhow::Result<()> {
        let path = std::fs::canonicalize(&self.repo_dir).unwrap_or_else(|_| self.repo_dir.clone());
        let entrypoint = std::fs::canonicalize(&self.eval_entry_point).map_or_else(
            |_| self.eval_entry_point.clone(),
            |p| p.to_string_lossy().to_string(),
        );
        db.upsert_watched_repo(&WatchedRepoInfo {
            name: &self.name,
            upstream: self.upstream_url.as_deref(),
            path: &path.to_string_lossy(),
            entrypoint: &entrypoint,
            base_branch: self.pr_config.as_ref().map(|c| c.base_branch.as_str()),
            fork_remote: Some(&self.fork),
        })
        .await
    }
}

/// Configuration for the watch (passive/event-driven) mode.
#[derive(Debug, Clone, Serialize)]
pub struct WatchConfig {
    /// Nix file entry point to evaluate (single-repo legacy mode)
    pub file: String,
    /// Path to SQLite database for tracking updates
    pub database_path: String,
    /// How often to poll feeds, in minutes
    pub poll_interval_minutes: u64,
    /// How often to rebuild the full upstream index, in hours
    pub index_refresh_hours: u64,
    /// Upstream git remote for PR creation
    pub upstream: Option<String>,
    /// Fork git remote for PR creation
    pub fork: String,
    /// Whether to run passthru.tests for packages
    pub run_passthru_tests: bool,
    /// Dry-run mode
    pub dry_run: bool,
    /// Number of concurrent update workers
    pub concurrent_updates: Option<usize>,
    /// Strategy for committing updates
    pub commit_strategy: CommitStrategy,
    /// Preserve failed worktrees
    pub preserve_failures: bool,
    /// Invoke Claude Code CLI on build/test failure
    pub claude_fix: bool,
    /// Max agent turns per Claude fix attempt
    pub claude_fix_max_turns: u32,
    /// Timeout in seconds for each Claude fix attempt
    pub claude_fix_timeout: u64,
    /// Multi-repo configurations (from config file).
    #[serde(skip)]
    pub repos: Vec<RepoConfig>,
}

impl WatchConfig {
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    pub fn from_args(
        file: String,
        database_path: String,
        poll_interval: u64,
        index_refresh_hours: u64,
        upstream: Option<String>,
        fork: String,
        run_passthru_tests: bool,
        dry_run: bool,
        concurrent_updates: Option<usize>,
        commit_strategy: CommitStrategy,
        preserve_failures: bool,
        claude_fix: bool,
        claude_fix_max_turns: u32,
        claude_fix_timeout: u64,
    ) -> Self {
        Self {
            file,
            database_path,
            poll_interval_minutes: poll_interval,
            index_refresh_hours,
            upstream,
            fork,
            run_passthru_tests,
            dry_run,
            concurrent_updates,
            commit_strategy,
            preserve_failures,
            claude_fix,
            claude_fix_max_turns,
            claude_fix_timeout,
            repos: Vec::new(),
        }
    }

    /// Execute the watch loop.
    pub async fn execute(self) -> anyhow::Result<()> {
        info!(
            "Starting watch mode (poll every {}m, re-index every {}h)",
            self.poll_interval_minutes, self.index_refresh_hours
        );

        let poll_interval = Duration::from_secs(self.poll_interval_minutes * 60);
        let index_refresh = Duration::from_secs(self.index_refresh_hours * 3600);

        let expanded_db = shellexpand::tilde(&self.database_path).to_string();
        let db = crate::database::Database::new(&expanded_db).await?;

        // Resolve repos: multi-repo mode or single-repo legacy mode
        let resolved_repos = if self.repos.is_empty() {
            // Single-repo legacy mode: synthesize one ResolvedRepo from CLI flags
            let pr_config = if let Some(ref remote_name) = self.upstream {
                crate::git::get_pr_config_from_remote(remote_name)
                    .await
                    .ok()
            } else {
                crate::git::get_pr_config_from_git().await.ok()
            };

            vec![ResolvedRepo {
                name: String::new(),
                eval_entry_point: self.file.clone(),
                repo_dir: PathBuf::from("."),
                pr_config,
                fork: self.fork.clone(),
                upstream_url: None,
                managed: false,
            }]
        } else {
            // Multi-repo mode: provision checkouts and resolve each repo
            let mut resolved = Vec::with_capacity(self.repos.len());
            for repo_config in &self.repos {
                match provision_repo(repo_config).await {
                    Ok(r) => {
                        info!(
                            "[{}] Provisioned: {} (entry: {})",
                            r.name,
                            r.repo_dir.display(),
                            r.eval_entry_point
                        );
                        resolved.push(r);
                    },
                    Err(e) => {
                        warn!("[{}] Failed to provision repo: {:#}", repo_config.name, e);
                        record_provision_failure(&db, repo_config, &e).await;
                    },
                }
            }
            if resolved.is_empty() {
                anyhow::bail!("No repositories could be provisioned");
            }
            resolved
        };

        // Close out state left behind by a previous (killed) watch process
        match db.recover_interrupted_watch_state().await {
            Ok(0) => {},
            Ok(n) => info!("Closed {} interrupted watch cycle(s)", n),
            Err(e) => warn!("Failed to recover interrupted watch state: {:#}", e),
        }

        // Register repos for the dashboard and forget repos removed from the
        // config (repos that merely failed to provision are kept).
        let configured: Vec<&str> = if self.repos.is_empty() {
            vec![""]
        } else {
            self.repos.iter().map(|r| r.name.as_str()).collect()
        };
        if let Err(e) = db.retain_watched_repos(&configured).await {
            warn!("Failed to prune watched repos: {:#}", e);
        }
        let names: Vec<&str> = resolved_repos.iter().map(|r| r.name.as_str()).collect();
        for repo in &resolved_repos {
            if let Err(e) = repo.register(&db).await {
                warn!("[{}] Failed to register repo: {:#}", repo.label(), e);
            }
        }
        let session_config = serde_json::json!({
            "mode": "watch",
            "repos": names,
            "dry_run": self.dry_run,
            "commit_strategy": self.commit_strategy,
            "poll_interval_minutes": self.poll_interval_minutes,
            "index_refresh_hours": self.index_refresh_hours,
        });

        // Concurrency
        let is_branch_mode = self.commit_strategy == CommitStrategy::Branch;
        let concurrency = if is_branch_mode {
            // `run_service` clamps this to 1 (see `updater_concurrency`).
            let requested = self.concurrent_updates.unwrap_or(1);
            crate::commands::run::warn_if_branch_concurrency_ignored(
                self.commit_strategy,
                requested,
            );
            requested
        } else {
            self.concurrent_updates.unwrap_or_else(|| {
                let cpus = num_cpus::get();
                std::cmp::max(1, cpus / 4)
            })
        };

        let pr_enhancements = PrEnhancementsConfig::default();

        let updater_config = crate::commands::run::UpdaterServiceConfig {
            session_id: String::new(), // Updated per poll cycle
            eval_entry_point: Arc::from(self.file.as_str()),
            pr_config: None, // Per-request overrides used in multi-repo mode
            fork: Arc::from(self.fork.as_str()),
            run_passthru_tests: self.run_passthru_tests,
            dry_run: self.dry_run,
            concurrency,
            pr_enhancements,
            interactive: false,
            preserve_failures: self.preserve_failures,
            commit_strategy: self.commit_strategy,
            claude_fix: self.claude_fix,
            claude_fix_max_turns: self.claude_fix_max_turns,
            claude_fix_timeout: self.claude_fix_timeout,
        };

        let repos = Arc::new(resolved_repos);

        listener::run_watch_loop(
            repos,
            db,
            updater_config,
            poll_interval,
            index_refresh,
            session_config,
        )
        .await
    }
}

/// Show a repository that could not be provisioned on the dashboard.
async fn record_provision_failure(db: &Database, config: &RepoConfig, error: &anyhow::Error) {
    let path = config
        .path
        .as_deref()
        .map(|p| shellexpand::tilde(p).to_string())
        .unwrap_or_default();
    let entrypoint = Path::new(&path)
        .join(&config.entrypoint)
        .to_string_lossy()
        .to_string();
    let info = WatchedRepoInfo {
        name: &config.name,
        upstream: Some(&config.upstream),
        path: &path,
        entrypoint: &entrypoint,
        base_branch: None,
        fork_remote: Some(&config.fork),
    };
    let message = format!("provisioning failed: {error:#}");
    let result = async {
        db.upsert_watched_repo(&info).await?;
        db.record_repo_sync(
            &config.name,
            RepoSyncStatus::Error,
            Some(&message),
            None,
            None,
        )
        .await
    }
    .await;
    if let Err(e) = result {
        warn!(
            "[{}] Failed to record provisioning failure: {:#}",
            config.name, e
        );
    }
}

/// Parse a repo `upstream` setting: a GitHub URL (`github.com/owner/repo`,
/// `https://…`, `git@github.com:…`) or the `owner/repo` shorthand.
fn parse_upstream(upstream: &str) -> Option<GithubRepo> {
    if let Some(repo) = parse_github_url(upstream) {
        return Some(repo);
    }
    let (owner, repo) = upstream.trim().trim_end_matches('/').split_once('/')?;
    let valid = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (valid(owner) && valid(repo) && !owner.contains('.')).then(|| GithubRepo {
        owner: owner.to_owned(),
        repo: repo.trim_end_matches(".git").to_owned(),
    })
}

/// Provision a repository: clone if missing, set up fork remote.
///
/// Existing checkouts are fetched and fast-forwarded by [`sync`] before
/// each index build.
async fn provision_repo(config: &RepoConfig) -> anyhow::Result<ResolvedRepo> {
    // Parse upstream URL to get owner/repo
    let github_repo = parse_upstream(&config.upstream).ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot parse upstream '{}' as a GitHub repository. Expected owner/repo or \
             github.com/owner/repo",
            config.upstream
        )
    })?;
    let upstream_url = format!(
        "https://github.com/{}/{}",
        github_repo.owner, github_repo.repo
    );

    // Determine checkout path
    let repo_path = match &config.path {
        Some(p) => PathBuf::from(shellexpand::tilde(p).as_ref()),
        None => anyhow::bail!(
            "repos.{}: 'path' is required (where to check out the repo)",
            config.name
        ),
    };

    // Clone if missing
    if repo_path.join(".git").exists() {
        info!(
            "[{}] Repository exists at {}",
            config.name,
            repo_path.display()
        );
    } else {
        let clone_url = format!("{upstream_url}.git");
        info!(
            "[{}] Cloning {} to {}",
            config.name,
            clone_url,
            repo_path.display()
        );

        // Create parent directory
        if let Some(parent) = repo_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create parent directory {}", parent.display()))?;
        }

        let output = tokio::process::Command::new("git")
            .args(["clone", &clone_url, &repo_path.to_string_lossy()])
            .output()
            .await
            .context("git clone")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git clone failed: {stderr}");
        }
    }

    // Ensure fork remote exists
    let fork_remote = &config.fork;
    if fork_remote != "origin" {
        // Check if the remote already exists
        let remote_check = run_git_in(&repo_path, &["remote", "get-url", fork_remote]).await;
        if remote_check.is_err() {
            // Need to create fork and add remote
            if let Ok(token) = std::env::var("GITHUB_TOKEN") {
                match crate::github::ensure_fork_exists(
                    &github_repo.owner,
                    &github_repo.repo,
                    &token,
                )
                .await
                {
                    Ok(fork_url) => {
                        info!(
                            "[{}] Adding fork remote '{}' -> {}",
                            config.name, fork_remote, fork_url
                        );
                        run_git_in(&repo_path, &["remote", "add", fork_remote, &fork_url]).await?;
                    },
                    Err(e) => {
                        warn!(
                            "[{}] Failed to create fork (PRs may not work): {:#}",
                            config.name, e
                        );
                    },
                }
            } else {
                warn!(
                    "[{}] GITHUB_TOKEN not set, cannot create fork remote '{}'",
                    config.name, fork_remote
                );
            }
        }
    }

    // Resolve entry point
    let entry_point = repo_path.join(&config.entrypoint);
    if !entry_point.exists() {
        anyhow::bail!(
            "Entry point {} does not exist in {}",
            config.entrypoint,
            repo_path.display()
        );
    }

    // Determine default branch for PR target
    let base_branch = get_default_branch_local(&repo_path)
        .await
        .unwrap_or_else(|_| "master".to_owned());

    let pr_config = Some(PrConfig {
        owner: github_repo.owner,
        repo: github_repo.repo,
        base_branch,
    });

    Ok(ResolvedRepo {
        name: config.name.clone(),
        eval_entry_point: entry_point.to_string_lossy().to_string(),
        repo_dir: repo_path,
        pr_config,
        fork: config.fork.clone(),
        upstream_url: Some(upstream_url),
        managed: true,
    })
}

/// Run a git command in a specific directory, returning Ok if it succeeds.
async fn run_git_in(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("git {} in {}", args.join(" "), cwd.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            cwd.display(),
            stderr
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Get the default branch from a local repo checkout.
async fn get_default_branch_local(repo_dir: &Path) -> anyhow::Result<String> {
    // Try symbolic-ref for origin/HEAD
    if let Ok(full_ref) = run_git_in(repo_dir, &["symbolic-ref", "refs/remotes/origin/HEAD"]).await
    {
        if let Some(branch) = full_ref.strip_prefix("refs/remotes/origin/") {
            return Ok(branch.to_owned());
        }
    }

    // Fallback: try common names
    for candidate in &["master", "main"] {
        let result = run_git_in(
            repo_dir,
            &["rev-parse", "--verify", &format!("origin/{candidate}")],
        )
        .await;
        if result.is_ok() {
            return Ok((*candidate).to_owned());
        }
    }

    anyhow::bail!("Could not determine default branch")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_upstream_forms() {
        let cases = [
            ("ekala-project/ekapkgs", Some(("ekala-project", "ekapkgs"))),
            ("github.com/o/r", Some(("o", "r"))),
            ("https://github.com/o/r.git", Some(("o", "r"))),
            ("git@github.com:o/r.git", Some(("o", "r"))),
            ("o/r.git", Some(("o", "r"))),
            ("gitlab.com/o/r", None),
            ("just-a-name", None),
            ("/abs/path", None),
        ];
        for (input, expected) in cases {
            let got = parse_upstream(input);
            assert_eq!(
                got.as_ref().map(|r| (r.owner.as_str(), r.repo.as_str())),
                expected,
                "{input}"
            );
        }
    }
}
