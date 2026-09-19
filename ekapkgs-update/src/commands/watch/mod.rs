//! Watch command: RSS/event-driven passive update mode.
//!
//! Instead of evaluating all packages via `nix-eval-jobs` on every run, the
//! watch command builds a reverse index mapping upstream sources to attr_paths,
//! then polls RSS/Atom feeds and APIs for new releases.  When a new release is
//! detected, it constructs an [`UpdateRequest`] and feeds it to the existing
//! updater pipeline.

pub mod feeds;
pub mod index;
mod listener;

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tracing::info;

use crate::cli::CommitStrategy;
use crate::commands::pr_enhancements::PrEnhancementsConfig;

/// Configuration for the watch (passive/event-driven) mode.
#[derive(Debug, Clone, Serialize)]
pub struct WatchConfig {
    /// Nix file entry point to evaluate
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

        // Determine PR configuration
        let pr_config = if let Some(ref remote_name) = self.upstream {
            crate::git::get_pr_config_from_remote(remote_name)
                .await
                .ok()
        } else {
            crate::git::get_pr_config_from_git().await.ok()
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
            pr_config,
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

        listener::run_watch_loop(
            &self.file,
            db,
            updater_config,
            poll_interval,
            index_refresh,
            self.dry_run,
        )
        .await
    }
}
