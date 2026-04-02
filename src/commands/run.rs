use futures::{StreamExt, pin_mut};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::database::Database;
use crate::git::{PrConfig, cleanup_worktree, create_worktree};
use crate::nix;
use crate::nix::nix_eval_jobs::NixEvalItem;
use crate::nix::{eval_nix_expr, normalize_entry_point};
use crate::package::PackageMetadata;
use crate::vcs_sources::{SemverStrategy, UpstreamSource};

/// Message sent from release checker service to updater service
#[derive(Debug, Clone)]
struct UpdateRequest {
    attr_path: String,
    drv: crate::nix::nix_eval_jobs::NixEvalDrv,
    current_version: String,
    new_version: String,
}

/// Service that monitors packages for new upstream releases
async fn release_checker_service(
    file: String,
    db: Database,
    tx: mpsc::UnboundedSender<UpdateRequest>,
    skip_unstable: bool,
) -> (usize, usize, usize) {
    let stream = nix::run_eval::run_nix_eval_jobs(file.clone());
    pin_mut!(stream);

    let mut error_count = 0;
    let mut skipped_count = 0;
    let mut checked_count = 0;

    // Consume the stream, checking each package for updates
    while let Some(result) = stream.next().await {
        match result {
            Ok(NixEvalItem::Drv(drv)) => {
                let attr_path = &drv.attr;

                // Check backoff period
                match db.should_check_update(attr_path).await {
                    Ok(false) => {
                        debug!("{}: Skipping (in backoff period)", attr_path);
                        skipped_count += 1;
                        continue;
                    },
                    Ok(true) => {
                        debug!("{}: Checking for updates", attr_path);
                    },
                    Err(e) => {
                        warn!(
                            "{}: Database error checking update status: {}",
                            attr_path, e
                        );
                        // Continue checking anyway
                    },
                }

                checked_count += 1;

                // Clone data for async task
                let db_clone = db.clone();
                let drv_clone = drv.clone();
                let attr_path_clone = attr_path.clone();
                let file_clone = file.clone();
                let tx_clone = tx.clone();

                // Spawn task to check this package
                tokio::spawn(async move {
                    if let Err(e) = check_for_update(
                        &db_clone,
                        &file_clone,
                        &drv_clone,
                        &attr_path_clone,
                        tx_clone,
                        skip_unstable,
                    )
                    .await
                    {
                        debug!("{}: Error checking for update: {}", attr_path_clone, e);
                    }
                });
            },
            Ok(NixEvalItem::Error(e)) => {
                debug!("Evaluation error: {:?}", e);
                error_count += 1;
            },
            Err(e) => {
                warn!("Stream error: {}", e);
                break;
            },
        }
    }

    info!("Release checker service complete");
    (checked_count, skipped_count, error_count)
}

/// Check a single package for upstream updates
async fn check_for_update(
    db: &Database,
    eval_entry_point: &str,
    drv: &crate::nix::nix_eval_jobs::NixEvalDrv,
    attr_path: &str,
    tx: mpsc::UnboundedSender<UpdateRequest>,
    skip_unstable: bool,
) -> anyhow::Result<()> {
    // Extract package metadata
    let metadata = match PackageMetadata::from_attr_path(eval_entry_point, attr_path).await {
        Ok(m) => m,
        Err(e) => {
            debug!("{}: Failed to extract metadata: {}", attr_path, e);
            return Ok(());
        },
    };

    let current_version = &metadata.version;
    debug!("{}: Current version: {}", attr_path, current_version);

    // Skip packages with 'unstable' in version if flag is set
    if skip_unstable && current_version.contains("unstable") {
        debug!(
            "{}: Skipping due to --skip-unstable flag (version: {})",
            attr_path, current_version
        );
        return Ok(());
    }

    // Determine upstream source
    let upstream_source = if let Some(ref src_url) = metadata.src_url {
        match UpstreamSource::from_url(src_url) {
            Some(source) => source,
            None => {
                debug!("{}: Could not parse upstream source from URL", attr_path);
                return Ok(());
            },
        }
    } else if let Some(ref pname) = metadata.pname {
        UpstreamSource::PyPI {
            pname: pname.clone(),
        }
    } else {
        debug!("{}: No source URL or pname found", attr_path);
        return Ok(());
    };

    // Fetch latest compatible release
    let best_release = match upstream_source
        .get_compatible_release(current_version, SemverStrategy::Latest)
        .await
    {
        Ok(release) => release,
        Err(e) => {
            debug!("{}: Failed to fetch upstream release: {}", attr_path, e);
            // Record no update available
            if let Err(db_err) = db
                .record_no_update(attr_path, current_version, "unknown")
                .await
            {
                warn!("{}: Failed to record no update: {}", attr_path, db_err);
            }
            return Ok(());
        },
    };

    let latest_version = UpstreamSource::get_version(&best_release);
    debug!("{}: Latest version: {}", attr_path, latest_version);

    // Check if update is needed
    if current_version == &latest_version {
        // No update needed - record in database
        if let Err(e) = db
            .record_no_update(attr_path, current_version, &latest_version)
            .await
        {
            warn!(
                "{}: Failed to record no update in database: {}",
                attr_path, e
            );
        }
        return Ok(());
    }

    // Check if there's a proposed version that differs from latest
    let record = db.get_update_record(attr_path).await?;
    if let Some(ref rec) = record {
        if let Some(ref proposed) = rec.proposed_version {
            if proposed == &latest_version {
                // Already proposed this version, still waiting for merge
                debug!(
                    "{}: Already proposed version {}, waiting for merge",
                    attr_path, proposed
                );
                if let Err(e) = db
                    .record_no_update(attr_path, current_version, &latest_version)
                    .await
                {
                    warn!("{}: Failed to update database: {}", attr_path, e);
                }
                return Ok(());
            } else {
                // Proposed version differs from latest - attempt new update
                info!(
                    "{}: New version {} available (previously proposed {})",
                    attr_path, latest_version, proposed
                );
            }
        }
    }

    // Update is available - send to updater service
    info!(
        "{}: Update available: {} -> {}",
        attr_path, current_version, latest_version
    );

    let update_req = UpdateRequest {
        attr_path: attr_path.to_string(),
        drv: drv.clone(),
        current_version: current_version.to_string(),
        new_version: latest_version.to_string(),
    };

    if let Err(e) = tx.send(update_req) {
        warn!("{}: Failed to send update request: {}", attr_path, e);
    }

    Ok(())
}

/// Service that performs package updates
async fn updater_service(
    eval_entry_point: String,
    mut rx: mpsc::UnboundedReceiver<UpdateRequest>,
    db: Database,
    pr_config: Option<PrConfig>,
    fork: String,
    run_passthru_tests: bool,
    dry_run: bool,
    concurrency: usize,
) -> (usize, usize) {
    let mut join_set: JoinSet<(anyhow::Result<UpdateResult>, String)> = JoinSet::new();
    let mut updated_count = 0;
    let mut failed_count = 0;

    // Helper function to process a completed task result
    let mut process_result = |result: anyhow::Result<UpdateResult>, attr_path: &str| {
        match result {
            Ok(UpdateResult::Updated { .. }) | Ok(UpdateResult::DryRun { .. }) => {
                updated_count += 1
            },
            Err(_) => failed_count += 1,
            _ => {},
        }
        handle_result(result, attr_path);
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
                        let eval_entry_point_clone = eval_entry_point.clone();
                        let pr_config_clone = pr_config.clone();
                        let fork_clone = fork.clone();
                        let attr_path_clone = req.attr_path.clone();

                        // Spawn the update task
                        join_set.spawn(async move {
                            let result = perform_update(
                                &db_clone,
                                &eval_entry_point_clone,
                                &req,
                                pr_config_clone.as_ref(),
                                &fork_clone,
                                run_passthru_tests,
                                dry_run,
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

/// Perform a package update
async fn perform_update(
    db: &Database,
    eval_entry_point: &str,
    req: &UpdateRequest,
    pr_config: Option<&PrConfig>,
    fork: &str,
    run_passthru_tests: bool,
    dry_run: bool,
) -> anyhow::Result<UpdateResult> {
    let attr_path = &req.attr_path;
    let current_version = &req.current_version;
    let new_version = &req.new_version;

    // If dry-run mode, report the update without performing it
    if dry_run {
        return Ok(UpdateResult::DryRun {
            current_version: current_version.to_string(),
            new_version: new_version.to_string(),
        });
    }

    // Create a worktree for this update
    let worktree_path = match create_worktree(attr_path).await {
        Ok(path) => path,
        Err(e) => {
            warn!("{}: Failed to create worktree: {}", attr_path, e);
            return Ok(UpdateResult::Skipped(format!(
                "Worktree creation failed: {}",
                e
            )));
        },
    };

    // Get file location from meta.position (in the main repository)
    let file_location = match get_file_location(eval_entry_point, attr_path).await {
        Ok(loc) => loc,
        Err(e) => {
            warn!("{}: Failed to get file location: {}", attr_path, e);
            cleanup_worktree(&worktree_path).await.ok();
            return Ok(UpdateResult::Skipped("Could not locate file".to_string()));
        },
    };

    debug!("{}: File location: {}", attr_path, file_location);

    // Convert the file path to be relative to the worktree
    let worktree_file_path = worktree_path.join(&file_location);
    let worktree_file_str = worktree_file_path.to_string_lossy().to_string();

    // Attempt the update in the worktree
    let update_result = crate::commands::update::update_from_file_path(
        eval_entry_point.to_string(),
        attr_path.to_string(),
        worktree_file_str,
        SemverStrategy::Latest,
        false,                // Don't auto-commit in run mode
        false,                // Don't create PR here (handled separately by create_pr_for_update)
        None,                 // upstream - not needed in run mode, PR handled separately
        "origin".to_string(), // fork - not used since create_pr is false
        run_passthru_tests,
        run_passthru_tests, // Fail on test errors in run mode
    )
    .await;

    match update_result {
        Ok(()) => {
            // Update succeeded
            info!("{}: Successfully updated to {}", attr_path, new_version);

            // Record successful update first
            if let Err(e) = db
                .record_successful_update(attr_path, current_version, new_version)
                .await
            {
                warn!("{}: Failed to record successful update: {}", attr_path, e);
            }

            // Create PR if configured
            if let Some(config) = pr_config {
                match create_pr_for_update(
                    db,
                    &worktree_path,
                    attr_path,
                    current_version,
                    new_version,
                    config,
                    fork,
                )
                .await
                {
                    Ok((pr_url, pr_number)) => {
                        info!("{}: Created PR #{}: {}", attr_path, pr_number, pr_url);
                    },
                    Err(e) => {
                        warn!("{}: Failed to create PR: {}", attr_path, e);
                        // Don't fail the update if PR creation fails
                    },
                }
            }

            // Clean up the worktree
            if let Err(e) = cleanup_worktree(&worktree_path).await {
                warn!("{}: Failed to clean up worktree: {}", attr_path, e);
            }

            Ok(UpdateResult::Updated {
                old_version: current_version.to_string(),
                new_version: new_version.to_string(),
            })
        },
        Err(e) => {
            // Update failed - record the failure log
            let error_message = format!("{:#}", e);
            warn!("{}: Update failed: {}", attr_path, error_message);

            // Clean up the worktree
            if let Err(cleanup_err) = cleanup_worktree(&worktree_path).await {
                warn!(
                    "{}: Failed to clean up worktree: {}",
                    attr_path, cleanup_err
                );
            }

            if let Err(db_err) = db
                .record_failed_update(
                    &req.drv.drv_path,
                    attr_path,
                    &error_message,
                    Some(current_version),
                    Some(new_version),
                )
                .await
            {
                warn!("{}: Failed to record update failure: {}", attr_path, db_err);
            }

            // Return as skipped so it doesn't count as a successful update
            Ok(UpdateResult::Skipped(format!("Update failed: {}", e)))
        },
    }
}

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

/// Do additional processing depending on the result of the update
fn handle_result(result: anyhow::Result<UpdateResult>, attr_path: &str) {
    match result {
        Ok(UpdateResult::Updated {
            old_version,
            new_version,
        }) => {
            info!(
                "{}: Updated from {} to {}",
                attr_path, old_version, new_version
            );
        },
        Ok(UpdateResult::Skipped(reason)) => {
            debug!("{}: Skipped - {}", attr_path, reason);
        },
        Ok(UpdateResult::DryRun {
            current_version,
            new_version,
        }) => {
            info!(
                "{}: Would update {} -> {}",
                attr_path, current_version, new_version
            );
        },
        Err(e) => {
            warn!("{}: Failed to check for updates: {}", attr_path, e);
        },
    }
}

#[derive(Debug)]
enum UpdateResult {
    Updated {
        old_version: String,
        new_version: String,
    },
    Skipped(String),
    DryRun {
        current_version: String,
        new_version: String,
    },
}

/// Get the file location for a package from meta.position
async fn get_file_location(eval_entry_point: &str, attr_path: &str) -> anyhow::Result<String> {
    let normalized_entry = normalize_entry_point(eval_entry_point);
    let position_expr = format!(
        "with import {} {{ }}; {}.meta.position",
        normalized_entry, attr_path
    );

    let position = eval_nix_expr(&position_expr).await?;

    if position.is_empty() {
        anyhow::bail!("Empty position returned from meta.position");
    }

    // Parse position string (format: "file:line")
    let (file_path, _line_str) = position
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {}", position))?;

    Ok(file_path.to_string())
}

/// Create a pull request for a successful update
async fn create_pr_for_update(
    db: &Database,
    worktree_path: &std::path::Path,
    attr_path: &str,
    old_version: &str,
    new_version: &str,
    config: &PrConfig,
    fork: &str,
) -> anyhow::Result<(String, i64)> {
    // Get GitHub token from environment
    let github_token = std::env::var("GITHUB_TOKEN")
        .map_err(|_| anyhow::anyhow!("GITHUB_TOKEN environment variable not set"))?;

    // Create and push branch
    let branch_name = crate::git::create_and_push_branch(
        worktree_path,
        attr_path,
        old_version,
        new_version,
        fork,
    )
    .await?;

    // Fetch package metadata for PR body
    let eval_entry_point = normalize_entry_point("<nixpkgs>");
    let metadata = PackageMetadata::from_attr_path(&eval_entry_point, attr_path)
        .await
        .ok();

    // Create PR title and body
    let title = format!(
        "Update {} from {} to {}",
        attr_path, old_version, new_version
    );
    let mut body = format!(
        "## Summary\n\nThis PR updates `{}` from version {} to {}.\n\n## Changes\n\n- Updated \
         package version\n- Updated source hash",
        attr_path, old_version, new_version
    );

    // Add optional metadata fields if available
    if let Some(meta) = metadata.as_ref() {
        if let Some(description) = meta.description.as_ref() {
            body.push_str(&format!(
                "\n\n## Package Information\n\n**Description:** {}",
                description
            ));
        } else {
            body.push_str("\n\n## Package Information");
        }
        if let Some(homepage) = meta.homepage.as_ref() {
            body.push_str(&format!("\n\n**Homepage:** {}", homepage));
        }
        if let Some(changelog) = meta.changelog.as_ref() {
            body.push_str(&format!("\n\n**Changelog:** {}", changelog));
        }
    }

    body.push_str("\n\n🤖 Generated with ekapkgs-update");

    // Create PR via GitHub API
    let pr = crate::github::create_pull_request(
        &config.owner,
        &config.repo,
        &title,
        &body,
        &branch_name,
        &config.base_branch,
        &github_token,
    )
    .await?;

    // Record PR info in database
    db.record_pr_info(attr_path, &pr.html_url, pr.number)
        .await?;

    Ok((pr.html_url, pr.number))
}
