//! Configuration structures for run mode operations

use std::sync::Arc;

use serde::Serialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::cli::CommitStrategy;
use crate::commands::pr_enhancements::PrEnhancementsConfig;
use crate::database::{Database, SessionCounter, SessionMode};
use crate::git::PrConfig;

/// Number of updates the updater service may run at once for `commit_strategy`.
///
/// Branch mode always runs one update at a time, whatever was requested.
/// All branch-mode updates share one working tree. The git mutex serializes
/// only `git add` / `commit` / `checkout`; rewriting, building and
/// `--claude-fix` edits run outside it. `branch_git::DirtySnapshot` treats
/// every path dirtied after an update started as that update's own. With two
/// updates in flight, one would sweep the other's files into its commit, or
/// delete and check them out mid-build when it reverts. Attributing paths per
/// update cannot be made exact: two updates can edit the same shared file, and
/// a fix may touch any path. Serializing whole updates is the only safe
/// choice. Worktree mode isolates each update and keeps the requested
/// concurrency.
#[must_use]
pub fn updater_concurrency(commit_strategy: CommitStrategy, requested: usize) -> usize {
    match commit_strategy {
        CommitStrategy::Branch => 1,
        CommitStrategy::Worktrees => requested.max(1),
    }
}

/// Warn when branch mode ignores a requested `--concurrent-updates` above 1.
pub fn warn_if_branch_concurrency_ignored(commit_strategy: CommitStrategy, requested: usize) {
    if updater_concurrency(commit_strategy, requested) < requested {
        tracing::warn!(
            "--concurrent-updates {} ignored: branch mode runs one update at a time, because \
             concurrent updates in the shared working tree would commit or revert each \
             other's files (use --commit-strategy worktrees for concurrency)",
            requested
        );
    }
}

/// Configuration for automated run mode
#[derive(Debug, Clone, Serialize)]
pub struct RunConfig {
    /// Nix file entry point to evaluate
    pub file: String,

    /// Path to SQLite database for tracking updates
    pub database_path: String,

    /// Upstream git remote for PR creation
    pub upstream: Option<String>,

    /// Fork git remote for PR creation
    pub fork: String,

    /// Whether to run passthru.tests for packages
    pub run_passthru_tests: bool,

    /// Dry-run mode (don't actually perform updates)
    pub dry_run: bool,

    /// Number of concurrent update workers
    pub concurrent_updates: Option<usize>,

    /// Skip packages marked as unstable
    pub skip_unstable: bool,

    /// PR enhancement configuration (CVE, Repology, directory diff, rebuilds)
    pub pr_enhancements: PrEnhancementsConfig,

    /// Interactive mode - prompt before submitting PRs
    pub interactive: bool,

    /// Preserve failed worktrees and artifacts for later inspection
    pub preserve_failures: bool,

    /// Strategy for committing updates
    pub commit_strategy: CommitStrategy,

    /// Invoke Claude Code CLI on build/test failure to attempt a fix
    pub claude_fix: bool,

    /// Maximum agent turns per Claude fix attempt
    pub claude_fix_max_turns: u32,

    /// Timeout in seconds for each Claude fix attempt
    pub claude_fix_timeout: u64,
}

impl RunConfig {
    /// Build a [`RunConfig`] from the flat set of CLI flags accepted by the
    /// `Run` subcommand.
    ///
    /// This keeps `Commands::execute` in `cli.rs` to a tight dispatcher by
    /// pushing the field-shuffling out of the CLI module.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    pub fn from_args(
        file: String,
        database_path: String,
        upstream: Option<String>,
        fork: String,
        run_passthru_tests: bool,
        dry_run: bool,
        concurrent_updates: Option<usize>,
        skip_unstable: bool,
        analyze_rebuilds: bool,
        max_rebuilds: Option<usize>,
        skip_cve: bool,
        skip_repology: bool,
        skip_directory_diff: bool,
        skip_cachix: bool,
        cachix_cache: Option<String>,
        interactive: bool,
        preserve_failures: bool,
        commit_strategy: CommitStrategy,
        audit: bool,
        no_security: bool,
        claude_fix: bool,
        claude_fix_max_turns: u32,
        claude_fix_timeout: u64,
    ) -> Self {
        // Resolve cache precedence: CLI flag → env var → None. The CLI value
        // is moved when present; any whitespace-only value is treated as
        // unset and falls through to `CACHIX_CACHE_NAME`.
        let cachix_cache = match cachix_cache {
            Some(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            _ => crate::cachix::resolve_cache_name(None),
        };
        Self {
            file,
            database_path,
            upstream,
            fork,
            run_passthru_tests,
            dry_run,
            concurrent_updates,
            skip_unstable,
            pr_enhancements: PrEnhancementsConfig {
                directory_diff: !skip_directory_diff,
                skip_cve_check: skip_cve,
                skip_repology,
                analyze_rebuilds,
                max_rebuilds,
                skip_cachix,
                cachix_cache,
                run_audit: audit,
                skip_security: no_security,
            },
            interactive,
            preserve_failures,
            commit_strategy,
            claude_fix,
            claude_fix_max_turns,
            claude_fix_timeout,
        }
    }

    /// Execute the automated update process
    pub async fn execute(self) -> anyhow::Result<()> {
        // Serialize config before destructuring
        let config_json = serde_json::to_string(&self).ok();

        let RunConfig {
            file,
            database_path,
            upstream,
            fork,
            run_passthru_tests,
            dry_run,
            concurrent_updates,
            skip_unstable,
            pr_enhancements,
            interactive,
            preserve_failures,
            commit_strategy,
            claude_fix,
            claude_fix_max_turns,
            claude_fix_timeout,
        } = self;

        info!("Running nix-eval-jobs on: {}", file);

        // Expand tilde in database path
        let expanded_db_path = shellexpand::tilde(&database_path).to_string();

        // Initialize database
        let db = Database::new(&expanded_db_path).await?;
        info!("Database initialized at: {}", expanded_db_path);

        // Create session for tracking this run
        let session_id = db.create_session(SessionMode::Run, config_json).await?;
        info!("Created session: {}", session_id);

        // Calculate concurrency: use provided value or default to CPU cores / 4 (minimum 1)
        // In interactive mode, force single-threaded execution
        let is_branch_mode = commit_strategy == CommitStrategy::Branch;
        let concurrency = if interactive {
            1
        } else if is_branch_mode {
            // Updates are clamped to 1 in `run_service` (see
            // `updater_concurrency`); the requested value still sizes the
            // release checker.
            concurrent_updates.unwrap_or(1)
        } else {
            concurrent_updates.unwrap_or_else(|| {
                let cpus = num_cpus::get();
                std::cmp::max(1, cpus / 4)
            })
        };
        if is_branch_mode {
            warn_if_branch_concurrency_ignored(commit_strategy, concurrency);
            info!("Running in branch mode (one update at a time, direct commits)");
        } else if interactive {
            info!("Running in interactive mode (single-threaded)");
        } else {
            info!("Running with concurrency level: {}", concurrency);
        }

        // Determine PR configuration: use CLI override or auto-detect from git
        let pr_config = if let Some(remote_name) = upstream {
            crate::git::get_pr_config_from_remote(&remote_name)
                .await
                .ok()
        } else {
            crate::git::get_pr_config_from_git().await.ok()
        };

        // Create channel for communication between services
        let (tx, rx) = mpsc::unbounded_channel();

        // Clone data for services
        let db_checker = db.clone();
        let db_updater = db.clone();
        let file_checker = file.clone();
        let file_updater = file.clone();

        // Spawn release checker service
        let skip_repology = pr_enhancements.skip_repology;
        let checker_concurrency = concurrency;
        let checker_session = session_id.clone();
        let checker_handle = tokio::spawn(async move {
            // Always limit checker concurrency to avoid OOM from unbounded
            // parallel nix-instantiate calls. Each checker task spawns up to
            // ~16 nix-instantiate processes for metadata extraction, so even
            // modest concurrency can exhaust memory on constrained machines.
            let max_checkers = std::cmp::max(4, checker_concurrency * 2);
            let max_eval_workers = std::cmp::max(1, checker_concurrency);
            super::checker::release_checker_service_with_limits(
                file_checker,
                db_checker,
                tx,
                skip_unstable,
                skip_repology,
                Some(max_checkers),
                Some(max_eval_workers),
                Some(checker_session),
            )
            .await
        });

        // Spawn updater service
        let updater_config = UpdaterServiceConfig {
            session_id: session_id.clone(),
            eval_entry_point: Arc::from(file_updater), // Convert String to Arc<str>
            pr_config,
            fork: Arc::from(fork), // Convert String to Arc<str>
            run_passthru_tests,
            dry_run,
            concurrency,
            pr_enhancements,
            interactive,
            preserve_failures,
            commit_strategy,
            claude_fix,
            claude_fix_max_turns,
            claude_fix_timeout,
        };
        let updater_handle =
            tokio::spawn(async move { updater_config.run_service(rx, db_updater).await });

        // Wait for both services to complete
        let (checker_result, updater_result) = tokio::join!(checker_handle, updater_handle);

        // Unwrap task results
        let (checked_count, skipped_count, error_count) = checker_result?;
        let stats = updater_result?;
        let updated_count = stats.updated + stats.dry_run;
        let failed_count = stats.failed;
        let packages_attempted = stats.attempted();

        // Update session status based on results
        let session_status = if failed_count > 0 {
            crate::database::SessionStatus::Failed
        } else {
            crate::database::SessionStatus::Completed
        };

        // Finalize session (outcome counters were maintained live)
        if let Err(e) = db
            .finalize_session(&session_id, session_status, checked_count, skipped_count)
            .await
        {
            tracing::warn!("Failed to finalize session: {}", e);
        }

        // Display comprehensive summary
        info!("============================================================");
        info!("All services complete!");
        info!("============================================================");
        info!("");
        if dry_run {
            info!("Update Summary (DRY-RUN - no changes made):");
        } else {
            info!("Update Summary:");
        }
        info!("");
        info!("  Packages evaluated:      {:>6}", checked_count);
        if error_count > 0 {
            info!("  Evaluation errors:       {:>6}", error_count);
        }
        info!("");
        info!("  Breakdown:");
        info!("    Skipped (backoff):     {:>6}", skipped_count);
        info!("    Attempted:             {:>6}", packages_attempted);
        info!("    Succeeded:             {:>6}", updated_count);
        info!("    Failed:                {:>6}", failed_count);
        if stats.skipped > 0 {
            info!("    Skipped (updater):     {:>6}", stats.skipped);
        }
        info!("");

        if packages_attempted > 0 {
            let success_rate = (updated_count as f64 / packages_attempted as f64) * 100.0;
            info!(
                "  Success rate:            {:>5.1}% ({}/{})",
                success_rate, updated_count, packages_attempted
            );
            info!("");
        }

        info!("  Session ID: {}", session_id);
        info!("  Database: {}", expanded_db_path);
        info!("============================================================");

        // Additional warnings/notices
        if skipped_count > packages_attempted * 10 {
            info!("");
            info!(
                "NOTICE: {} packages were skipped due to backoff cooldown",
                skipped_count
            );
            info!("These packages have recently failed and are in cooldown period");
        }

        if failed_count > 0 && preserve_failures {
            info!("");
            info!(
                "NOTICE: {} updates failed - artifacts preserved for inspection",
                failed_count
            );
            info!("Location: ~/.cache/ekapkgs-update/failed/{}/", session_id);
        }

        Ok(())
    }
}

/// Configuration for the updater service
#[derive(Debug, Clone)]
pub struct UpdaterServiceConfig {
    /// Session ID for tracking this run
    pub session_id: String,

    /// Nix file entry point (Arc for cheap cloning in async tasks)
    pub eval_entry_point: Arc<str>,

    /// Pull request configuration (if enabled)
    pub pr_config: Option<PrConfig>,

    /// Fork git remote (Arc for cheap cloning in async tasks)
    pub fork: Arc<str>,

    /// Whether to run passthru.tests
    pub run_passthru_tests: bool,

    /// Dry-run mode
    pub dry_run: bool,

    /// Number of concurrent update workers
    pub concurrency: usize,

    /// PR enhancement configuration (CVE, Repology, directory diff, rebuilds)
    pub pr_enhancements: PrEnhancementsConfig,

    /// Interactive mode - prompt before submitting PRs
    pub interactive: bool,

    /// Preserve failed worktrees and artifacts for later inspection
    pub preserve_failures: bool,

    /// Strategy for committing updates
    pub commit_strategy: CommitStrategy,

    /// Invoke Claude Code CLI on build/test failure to attempt a fix
    pub claude_fix: bool,

    /// Maximum agent turns per Claude fix attempt
    pub claude_fix_max_turns: u32,

    /// Timeout in seconds for each Claude fix attempt
    pub claude_fix_timeout: u64,
}

/// Outcome counts of an updater service run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdaterStats {
    /// Updates built and committed / proposed.
    pub updated: usize,
    /// Dry-run reports (not performed).
    pub dry_run: usize,
    /// Attempts that failed (including unexpected errors and panics).
    pub failed: usize,
    /// Requests that were not attempted (setup failures, rebuild threshold).
    pub skipped: usize,
}

impl UpdaterStats {
    /// Total requests processed.
    pub fn attempted(&self) -> usize {
        self.updated + self.dry_run + self.failed + self.skipped
    }

    /// Count a finished request and return the session counter to bump.
    fn count(&mut self, result: &anyhow::Result<super::types::UpdateResult>) -> SessionCounter {
        use super::types::UpdateResult;
        match result {
            Ok(UpdateResult::Updated { .. }) => {
                self.updated += 1;
                SessionCounter::Succeeded
            },
            Ok(UpdateResult::DryRun { .. }) => {
                self.dry_run += 1;
                SessionCounter::Skipped
            },
            Ok(UpdateResult::Skipped(_)) => {
                self.skipped += 1;
                SessionCounter::Skipped
            },
            Ok(UpdateResult::Failed(_)) | Err(_) => {
                self.failed += 1;
                SessionCounter::Failed
            },
        }
    }
}

type TaskOutput = (anyhow::Result<super::types::UpdateResult>, String);

/// Account for a finished update task: local stats, live session counter
/// and logging.
async fn finish_task(
    db: &Database,
    session_id: &str,
    stats: &mut UpdaterStats,
    task_result: Result<TaskOutput, tokio::task::JoinError>,
) {
    let counter = match task_result {
        Ok((result, attr_path)) => {
            let counter = stats.count(&result);
            super::updater::handle_result(result, &attr_path);
            counter
        },
        Err(e) => {
            tracing::warn!("Task panicked: {}", e);
            stats.failed += 1;
            SessionCounter::Failed
        },
    };
    if let Err(e) = db.increment_session_counter(session_id, counter).await {
        tracing::warn!("Failed to update session counters: {}", e);
    }
}

impl UpdaterServiceConfig {
    /// Run the updater service that processes package update requests.
    ///
    /// Session counters (`packages_attempted` and the outcome counters) are
    /// updated live as requests start and finish.
    pub async fn run_service(
        self,
        mut rx: mpsc::UnboundedReceiver<super::types::UpdateRequest>,
        db: Database,
    ) -> UpdaterStats {
        use tokio::task::JoinSet;
        use tracing::warn;

        let UpdaterServiceConfig {
            session_id,
            eval_entry_point,
            pr_config,
            fork,
            run_passthru_tests,
            dry_run,
            concurrency,
            pr_enhancements,
            interactive,
            preserve_failures,
            commit_strategy,
            claude_fix,
            claude_fix_max_turns,
            claude_fix_timeout,
        } = self;

        // Build Claude fix config if enabled
        let claude_fix_config = if claude_fix {
            Some(Arc::new(super::claude_fix::ClaudeFixConfig {
                max_turns: claude_fix_max_turns,
                timeout_secs: claude_fix_timeout,
            }))
        } else {
            None
        };

        let concurrency = updater_concurrency(commit_strategy, concurrency);

        // Serializes git operations in branch mode. Updates are serialized
        // there too (see `updater_concurrency`); the mutex stays as a
        // second guard for the git commands themselves.
        let git_mutex = Arc::new(tokio::sync::Mutex::new(()));

        let mut join_set: JoinSet<TaskOutput> = JoinSet::new();
        let mut stats = UpdaterStats::default();

        loop {
            tokio::select! {
                // Receive update requests from release checker
                update_req = rx.recv() => {
                    // `None`: channel closed - no more updates coming
                    let Some(req) = update_req else { break };

                    // Wait if we've reached the concurrency limit
                    while join_set.len() >= concurrency {
                        if let Some(task_result) = join_set.join_next().await {
                            finish_task(&db, &session_id, &mut stats, task_result).await;
                        }
                    }

                    if let Err(e) = db
                        .increment_session_counter(&session_id, SessionCounter::Attempted)
                        .await
                    {
                        warn!("Failed to update session counters: {}", e);
                    }

                    // Clone data needed for the async task
                    let db_clone = db.clone();
                    let session_id_clone = session_id.clone();
                    let pr_enhancements_clone = pr_enhancements.clone();
                    let attr_path_clone = req.attr_path.clone();
                    let claude_fix_clone = claude_fix_config.clone();

                    // Use per-request overrides when present (multi-repo routing),
                    // falling back to the global config.
                    let effective_entry_point: Arc<str> = match req.eval_entry_point.as_deref() {
                        Some(ep) => Arc::from(ep),
                        None => Arc::clone(&eval_entry_point),
                    };
                    let effective_pr_config = req.pr_config.clone().or_else(|| pr_config.clone());
                    let effective_fork: Arc<str> = match req.fork.as_deref() {
                        Some(f) => Arc::from(f),
                        None => Arc::clone(&fork),
                    };

                    // Spawn the update task
                    let is_branch_mode = commit_strategy == CommitStrategy::Branch;
                    let git_mutex_clone = Arc::clone(&git_mutex);
                    join_set.spawn(async move {
                        let result = if is_branch_mode {
                            super::updater::perform_direct_update(
                                &db_clone,
                                &session_id_clone,
                                &effective_entry_point,
                                &req,
                                run_passthru_tests,
                                git_mutex_clone,
                                claude_fix_clone.as_deref(),
                            )
                            .await
                        } else {
                            super::updater::perform_update(
                                &db_clone,
                                &session_id_clone,
                                &effective_entry_point,
                                &req,
                                effective_pr_config.as_ref(),
                                &effective_fork,
                                run_passthru_tests,
                                dry_run,
                                &pr_enhancements_clone,
                                interactive,
                                preserve_failures,
                                claude_fix_clone.as_deref(),
                            )
                            .await
                        };
                        (result, attr_path_clone)
                    });
                },
                // Also drain completed tasks while waiting for new requests
                Some(task_result) = join_set.join_next(), if !join_set.is_empty() => {
                    finish_task(&db, &session_id, &mut stats, task_result).await;
                },
            }
        }

        // Wait for all remaining tasks to complete
        while let Some(task_result) = join_set.join_next().await {
            finish_task(&db, &session_id, &mut stats, task_result).await;
        }

        info!("Updater service complete");
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::UpdateResult;
    use super::*;

    #[test]
    fn branch_mode_runs_updates_one_at_a_time() {
        for requested in [0, 1, 2, 8] {
            assert_eq!(updater_concurrency(CommitStrategy::Branch, requested), 1);
        }
    }

    #[test]
    fn worktree_mode_keeps_requested_concurrency() {
        assert_eq!(updater_concurrency(CommitStrategy::Worktrees, 0), 1);
        assert_eq!(updater_concurrency(CommitStrategy::Worktrees, 1), 1);
        assert_eq!(updater_concurrency(CommitStrategy::Worktrees, 8), 8);
    }

    #[test]
    fn updater_stats_count_outcomes() {
        let mut stats = UpdaterStats::default();
        let results: Vec<anyhow::Result<UpdateResult>> = vec![
            Ok(UpdateResult::Updated {
                old_version: "1".into(),
                new_version: "2".into(),
            }),
            Ok(UpdateResult::DryRun {
                current_version: "1".into(),
                new_version: "2".into(),
            }),
            Ok(UpdateResult::Skipped("x".into())),
            Ok(UpdateResult::Failed("x".into())),
            Err(anyhow::anyhow!("boom")),
        ];
        let counters: Vec<SessionCounter> = results.iter().map(|r| stats.count(r)).collect();
        assert_eq!(
            counters,
            vec![
                SessionCounter::Succeeded,
                SessionCounter::Skipped,
                SessionCounter::Skipped,
                SessionCounter::Failed,
                SessionCounter::Failed,
            ]
        );
        assert_eq!(
            stats,
            UpdaterStats {
                updated: 1,
                dry_run: 1,
                failed: 2,
                skipped: 1,
            }
        );
        assert_eq!(stats.attempted(), 5);
    }
}
