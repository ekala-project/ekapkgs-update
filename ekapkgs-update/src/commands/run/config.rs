//! Configuration structures for run mode operations

use std::sync::Arc;

use serde::Serialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::cli::CommitStrategy;
use crate::commands::pr_enhancements::PrEnhancementsConfig;
use crate::database::Database;
use crate::git::PrConfig;

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
        let session_id = db.create_session(config_json).await?;
        info!("Created session: {}", session_id);

        // Calculate concurrency: use provided value or default to CPU cores / 4 (minimum 1)
        // In interactive mode, force single-threaded execution
        let is_branch_mode = commit_strategy == CommitStrategy::Branch;
        let concurrency = if interactive {
            1
        } else if is_branch_mode {
            // Branch mode defaults to 1 but allows --concurrent-updates override
            concurrent_updates.unwrap_or(1)
        } else {
            concurrent_updates.unwrap_or_else(|| {
                let cpus = num_cpus::get();
                std::cmp::max(1, cpus / 4)
            })
        };
        if is_branch_mode {
            info!(
                "Running in branch mode (concurrency: {}, direct commits)",
                concurrency
            );
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
        let (updated_count, failed_count) = updater_result?;

        // Calculate total attempted updates
        let packages_attempted = updated_count + failed_count;

        // Update session status based on results
        let session_status = if failed_count > 0 {
            crate::database::SessionStatus::Failed
        } else {
            crate::database::SessionStatus::Completed
        };

        // Finalize session with complete statistics
        if let Err(e) = db
            .finalize_session(
                &session_id,
                session_status,
                packages_attempted,
                updated_count,
                failed_count,
                skipped_count,
            )
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

impl UpdaterServiceConfig {
    /// Run the updater service that processes package update requests
    pub async fn run_service(
        self,
        mut rx: mpsc::UnboundedReceiver<super::types::UpdateRequest>,
        db: Database,
    ) -> (usize, usize) {
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

        // Mutex to serialize git operations in branch mode (concurrent builds
        // modify different files, but git add/commit must not race)
        let git_mutex = Arc::new(tokio::sync::Mutex::new(()));

        let mut join_set: JoinSet<(anyhow::Result<super::types::UpdateResult>, String)> =
            JoinSet::new();
        let mut updated_count = 0;
        let mut failed_count = 0;

        // Helper function to process a completed task result
        let mut process_result = |result: anyhow::Result<super::types::UpdateResult>,
                                  attr_path: &str| {
            match result {
                Ok(super::types::UpdateResult::Updated { .. })
                | Ok(super::types::UpdateResult::DryRun { .. }) => updated_count += 1,
                Err(_) => failed_count += 1,
                _ => {},
            }
            super::updater::handle_result(result, attr_path);
        };

        loop {
            tokio::select! {
                // Receive update requests from release checker
                update_req = rx.recv() => {
                    match update_req {
                        Some(req) => {
                            // Wait if we've reached the concurrency limit
                            while join_set.len() >= concurrency {
                                if let Some(task_result) = join_set.join_next().await {
                                    match task_result {
                                        Ok((result, task_attr_path)) => {
                                            process_result(result, &task_attr_path);
                                        },
                                        Err(e) => {
                                            warn!("Task panicked: {}", e);
                                        },
                                    }
                                }
                            }

                            // Clone data needed for the async task
                            let db_clone = db.clone();
                            let session_id_clone = session_id.clone();
                            let eval_entry_point_clone = Arc::clone(&eval_entry_point); // O(1) clone
                            let pr_config_clone = pr_config.clone();
                            let fork_clone = Arc::clone(&fork); // O(1) clone
                            let pr_enhancements_clone = pr_enhancements.clone();
                            let attr_path_clone = req.attr_path.clone();
                            let claude_fix_clone = claude_fix_config.clone();

                            // Spawn the update task
                            let is_branch_mode = commit_strategy == CommitStrategy::Branch;
                            let git_mutex_clone = Arc::clone(&git_mutex);
                            join_set.spawn(async move {
                                let result = if is_branch_mode {
                                    super::updater::perform_direct_update(
                                        &db_clone,
                                        &session_id_clone,
                                        &eval_entry_point_clone,
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
                                        &eval_entry_point_clone, // Arc<str> derefs to &str
                                        &req,
                                        pr_config_clone.as_ref(),
                                        &fork_clone, // Arc<str> derefs to &str
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
                        None => {
                            // Channel closed - no more updates coming
                            break;
                        }
                    }
                },
                // Also drain completed tasks while waiting for new requests
                Some(task_result) = join_set.join_next(), if !join_set.is_empty() => {
                    match task_result {
                        Ok((result, attr_path)) => {
                            process_result(result, &attr_path);
                        },
                        Err(e) => {
                            warn!("Task panicked: {}", e);
                        },
                    }
                },
            }
        }

        // Wait for all remaining tasks to complete
        while let Some(task_result) = join_set.join_next().await {
            match task_result {
                Ok((result, attr_path)) => {
                    process_result(result, &attr_path);
                },
                Err(e) => {
                    warn!("Task panicked: {}", e);
                },
            }
        }

        info!("Updater service complete");
        (updated_count, failed_count)
    }
}
