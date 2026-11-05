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
//! to the correct upstream repository.

pub mod feeds;
pub mod index;
mod listener;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde::Serialize;
use tracing::{info, warn};

use crate::cli::CommitStrategy;
use crate::commands::pr_enhancements::PrEnhancementsConfig;
use crate::config::RepoConfig;
use crate::git::PrConfig;
use crate::github::parse_github_url;

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
                    },
                }
            }
            if resolved.is_empty() {
                anyhow::bail!("No repositories could be provisioned");
            }
            resolved
        };

        // Concurrency
        let is_branch_mode = self.commit_strategy == CommitStrategy::Branch;
        let concurrency = if is_branch_mode {
            self.concurrent_updates.unwrap_or(1)
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
            self.dry_run,
        )
        .await
    }
}

/// Provision a repository: clone if missing, fetch if present, set up fork remote.
async fn provision_repo(config: &RepoConfig) -> anyhow::Result<ResolvedRepo> {
    // Parse upstream URL to get owner/repo
    let github_repo = parse_github_url(&config.upstream).ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot parse upstream '{}' as a GitHub URL. Expected format: github.com/owner/repo",
            config.upstream
        )
    })?;

    // Determine checkout path
    let repo_path = match &config.path {
        Some(p) => PathBuf::from(shellexpand::tilde(p).as_ref()),
        None => anyhow::bail!(
            "repos.{}: 'path' is required (where to check out the repo)",
            config.name
        ),
    };

    // Clone or fetch
    if repo_path.join(".git").exists() {
        info!(
            "[{}] Repository exists at {}, fetching...",
            config.name,
            repo_path.display()
        );
        run_git_in(&repo_path, &["fetch", "--all", "--prune"]).await?;
    } else {
        let clone_url = format!(
            "https://github.com/{}/{}.git",
            github_repo.owner, github_repo.repo
        );
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
