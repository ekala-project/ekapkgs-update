//! Configuration structures for run mode operations

use crate::database::Database;
use crate::git::PrConfig;
use tokio::sync::mpsc;
use tracing::info;

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
            super::checker::release_checker_service(file_checker, db_checker, tx, skip_unstable).await
        });

        // Spawn updater service
        let updater_config = UpdaterServiceConfig {
            eval_entry_point: file_updater,
            pr_config,
            fork,
            run_passthru_tests,
            dry_run,
            concurrency,
        };
        let updater_handle = tokio::spawn(async move {
            super::updater::updater_service(rx, db_updater, updater_config).await
        });

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
    /// Nix file entry point
    pub eval_entry_point: String,

    /// Pull request configuration (if enabled)
    pub pr_config: Option<PrConfig>,

    /// Fork git remote
    pub fork: String,

    /// Whether to run passthru.tests
    pub run_passthru_tests: bool,

    /// Dry-run mode
    pub dry_run: bool,

    /// Number of concurrent update workers
    pub concurrency: usize,
}
