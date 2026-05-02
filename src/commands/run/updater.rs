use std::path::Path;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use super::types::{UpdateRequest, UpdateResult};
use crate::database::Database;
use crate::git::{PrConfig, cleanup_worktree, create_worktree};
use crate::nix::{eval_nix_expr, normalize_entry_point};
use crate::package::PackageMetadata;
use crate::vcs_sources::SemverStrategy;

/// Service that performs package updates
pub async fn updater_service(
    mut rx: mpsc::UnboundedReceiver<UpdateRequest>,
    db: Database,
    config: super::UpdaterServiceConfig,
) -> (usize, usize) {
    let super::UpdaterServiceConfig {
        eval_entry_point,
        pr_config,
        fork,
        run_passthru_tests,
        dry_run,
        concurrency,
    } = config;
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
    let version_config = crate::commands::update::VersionConfig::new(SemverStrategy::Latest);
    let update_config = crate::commands::update::UpdateConfig {
        commit: false,              // Don't auto-commit in run mode
        create_pr: false,           // Don't create PR here (handled separately)
        upstream: None,             // upstream - not needed in run mode
        fork: "origin".to_string(), // fork - not used since create_pr is false
        run_passthru_tests,
        src_only: false, // Update all dependencies (not src-only)
        format: false,   // No formatting in run mode (worktree cleanup would lose it)
    };

    let update_result = crate::commands::update::update_from_file_path(
        eval_entry_point.to_string(),
        attr_path.to_string(),
        worktree_file_str,
        version_config,
        update_config,
        run_passthru_tests, // Fail on test errors in run mode
    )
    .await;

    match update_result {
        Ok(removed_patches) => {
            // Update succeeded
            info!("{}: Successfully updated to {}", attr_path, new_version);

            // Log removed patches if any
            if !removed_patches.is_empty() {
                info!(
                    "{}: Removed {} obsolete patch(es): {}",
                    attr_path,
                    removed_patches.len(),
                    removed_patches.join(", ")
                );
            }

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
    worktree_path: &Path,
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
        body.push_str("\n\n## Package Information");

        if let Some(description) = &meta.description {
            body.push_str(&format!("\n\n**Description:** {}", description));
        }

        if let Some(homepage) = &meta.homepage {
            body.push_str(&format!("\n\n**Homepage:** {}", homepage));
        }

        if let Some(changelog) = &meta.changelog {
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
