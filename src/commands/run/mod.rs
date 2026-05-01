mod checker;
mod types;
mod updater;

use tokio::sync::mpsc;
use tracing::info;

use crate::database::Database;

use checker::release_checker_service;
use updater::updater_service;

/// Run the automated update process
pub async fn run(
    file: String,
    database_path: String,
    upstream: Option<String>,
    fork: String,
    run_passthru_tests: bool,
    dry_run: bool,
    concurrent_updates: Option<usize>,
    skip_unstable: bool,
) -> anyhow::Result<()> {
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
        release_checker_service(file_checker, db_checker, tx, skip_unstable).await
    });

    // Spawn updater service
    let updater_handle = tokio::spawn(async move {
        updater_service(
            file_updater,
            rx,
            db_updater,
            pr_config,
            fork,
            run_passthru_tests,
            dry_run,
            concurrency,
        )
        .await
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
