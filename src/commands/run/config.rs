//! Configuration structures for run mode operations

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::info;

use crate::database::Database;
use crate::git::PrConfig;

/// Configuration for automated run mode
#[derive(Debug, Clone)]
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

    /// Analyze and report rebuild counts for each update
    pub analyze_rebuilds: bool,

    /// Skip updates that would cause more than N rebuilds
    pub max_rebuilds: Option<usize>,

    /// Disable CVE vulnerability checking
    pub no_cve: bool,

    /// Disable Repology cross-distribution version checking
    pub no_repology: bool,

    /// Whether to include directory diff in PR body
    pub directory_diff: bool,
}

impl RunConfig {
    /// Execute the automated update process
    pub async fn execute(self) -> anyhow::Result<()> {
        let RunConfig {
            file,
            database_path,
            upstream,
            fork,
            run_passthru_tests,
            dry_run,
            concurrent_updates,
            skip_unstable,
            analyze_rebuilds,
            max_rebuilds,
            no_cve,
            no_repology,
            directory_diff,
        } = self;

        info!("Running nix-eval-jobs on: {}", file);

        // Expand tilde in database path
        let expanded_db_path = shellexpand::tilde(&database_path).to_string();

        // Initialize database
        let db = Database::new(&expanded_db_path).await?;
        info!("Database initialized at: {}", expanded_db_path);

        // Calculate concurrency: use provided value or default to CPU cores / 4 (minimum 1)
        let concurrency = concurrent_updates.unwrap_or_else(|| {
            let cpus = num_cpus::get();
            std::cmp::max(1, cpus / 4)
        });
        info!("Running with concurrency level: {}", concurrency);

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
        let checker_handle = tokio::spawn(async move {
            super::checker::release_checker_service(
                file_checker,
                db_checker,
                tx,
                skip_unstable,
                no_repology,
            )
            .await
        });

        // Spawn updater service
        let updater_config = UpdaterServiceConfig {
            eval_entry_point: Arc::from(file_updater), // Convert String to Arc<str>
            pr_config,
            fork: Arc::from(fork), // Convert String to Arc<str>
            run_passthru_tests,
            dry_run,
            concurrency,
            analyze_rebuilds,
            max_rebuilds,
            no_cve,
            directory_diff,
        };
        let updater_handle =
            tokio::spawn(async move { updater_config.run_service(rx, db_updater).await });

        // Wait for both services to complete
        let (checker_result, updater_result) = tokio::join!(checker_handle, updater_handle);

        // Unwrap task results
        let (checked_count, skipped_count, error_count) = checker_result?;
        let (updated_count, failed_count) = updater_result?;

        // Display summary
        info!("All services complete!");
        if error_count > 0 {
            info!("Evaluation errors: {}", error_count);
        }
        if dry_run {
            info!("Update summary (dry-run scan - no changes made):");
        } else {
            info!("Update summary:");
        }
        info!("  Checked: {}", checked_count);
        info!("  Skipped (backoff): {}", skipped_count);
        info!("  Updated: {}", updated_count);
        info!("  Failed: {}", failed_count);

        Ok(())
    }
}

/// Configuration for the updater service
#[derive(Debug, Clone)]
pub struct UpdaterServiceConfig {
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

    /// Analyze and report rebuild counts for each update
    pub analyze_rebuilds: bool,

    /// Skip updates that would cause more than N rebuilds
    pub max_rebuilds: Option<usize>,

    /// Disable CVE vulnerability checking
    pub no_cve: bool,

    /// Whether to include directory diff in PR body
    pub directory_diff: bool,
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
            eval_entry_point,
            pr_config,
            fork,
            run_passthru_tests,
            dry_run,
            concurrency,
            analyze_rebuilds,
            max_rebuilds,
            no_cve,
            directory_diff,
        } = self;

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
                            let eval_entry_point_clone = Arc::clone(&eval_entry_point); // O(1) clone
                            let pr_config_clone = pr_config.clone();
                            let fork_clone = Arc::clone(&fork); // O(1) clone
                            let attr_path_clone = req.attr_path.clone();

                            // Spawn the update task
                            join_set.spawn(async move {
                                let result = super::updater::perform_update(
                                    &db_clone,
                                    &eval_entry_point_clone, // Arc<str> derefs to &str
                                    &req,
                                    pr_config_clone.as_ref(),
                                    &fork_clone, // Arc<str> derefs to &str
                                    run_passthru_tests,
                                    dry_run,
                                    analyze_rebuilds,
                                    max_rebuilds,
                                    no_cve,
                                    directory_diff,
                                )
                                .await;
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
