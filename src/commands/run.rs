use futures::{StreamExt, pin_mut};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::database::Database;
use crate::git::{PrConfig, cleanup_worktree, create_worktree};
use crate::nix;
use crate::nix::nix_eval_jobs::NixEvalItem;
use crate::nix::{eval_nix_expr, normalize_entry_point};
use crate::package::PackageMetadata;
use crate::vcs_sources::{SemverStrategy, UpstreamSource};

pub async fn run(
    file: String,
    database_path: String,
    upstream: Option<String>,
    fork: String,
    run_passthru_tests: bool,
    dry_run: bool,
    concurrent_updates: Option<usize>,
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

    let stream = nix::run_eval::run_nix_eval_jobs(file.clone());
    pin_mut!(stream);

    let mut drvs = Vec::new();
    let mut error_count = 0;
    let mut skipped_count = 0;
    let mut checked_count = 0;
    let mut updated_count = 0;
    let mut failed_count = 0;

    // JoinSet for managing concurrent update tasks
    let mut join_set: JoinSet<(anyhow::Result<UpdateResult>, String)> = JoinSet::new();

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

    // Consume the stream, processing each item as it arrives
    while let Some(result) = stream.next().await {
        match result {
            Ok(NixEvalItem::Drv(drv)) => {
                drvs.push(drv.clone());

                // Check if we should attempt an update for this package
                let attr_path = &drv.attr;

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
                let file_clone = file.clone();
                let drv_clone = drv.clone();
                let pr_config_clone = pr_config.clone();
                let fork_clone = fork.clone();
                let attr_path_clone = attr_path.clone();

                // Spawn the update task
                join_set.spawn(async move {
                    let result = check_and_update_package(
                        &db_clone,
                        &file_clone,
                        &drv_clone,
                        pr_config_clone.as_ref(),
                        &fork_clone,
                        run_passthru_tests,
                        dry_run,
                    )
                    .await;
                    (result, attr_path_clone)
                });
            },
            Ok(NixEvalItem::Error(e)) => {
                debug!("Evaluation error: {:?}", e);
                error_count += 1;
            },
            Err(e) => {
                return Err(e);
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

    // Display summary
    info!("Evaluation complete!");
    info!("Total derivations: {}", drvs.len());
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

    // Count by system
    let mut systems = std::collections::HashMap::new();
    for drv in &drvs {
        *systems.entry(&drv.system).or_insert(0) += 1;
    }

    info!("Derivations by system:");
    for (system, count) in systems {
        info!("  {}: {}", system, count);
    }

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
        Ok(UpdateResult::NoUpdateNeeded {
            current_version,
            latest_version,
        }) => {
            debug!(
                "{}: No update needed (current: {}, latest: {})",
                attr_path, current_version, latest_version
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
    NoUpdateNeeded {
        current_version: String,
        latest_version: String,
    },
    Skipped(String),
    DryRun {
        current_version: String,
        new_version: String,
    },
}

/// Check if a package needs updating and attempt to update it
async fn check_and_update_package(
    db: &Database,
    eval_entry_point: &str,
    drv: &crate::nix::nix_eval_jobs::NixEvalDrv,
    pr_config: Option<&PrConfig>,
    fork: &str,
    run_passthru_tests: bool,
    dry_run: bool,
) -> anyhow::Result<UpdateResult> {
    let attr_path = &drv.attr;

    // Extract package metadata to get current version
    let metadata = match PackageMetadata::from_attr_path(eval_entry_point, attr_path).await {
        Ok(m) => m,
        Err(e) => {
            debug!("{}: Failed to extract metadata: {}", attr_path, e);
            return Ok(UpdateResult::Skipped(
                "Could not extract metadata".to_string(),
            ));
        },
    };

    let current_version = &metadata.version;
    debug!("{}: Current version: {}", attr_path, current_version);

    // Determine upstream source
    let upstream_source = if let Some(ref src_url) = metadata.src_url {
        match UpstreamSource::from_url(src_url) {
            Some(source) => source,
            None => {
                debug!("{}: Could not parse upstream source from URL", attr_path);
                return Ok(UpdateResult::Skipped("Unsupported source".to_string()));
            },
        }
    } else if let Some(ref pname) = metadata.pname {
        UpstreamSource::PyPI {
            pname: pname.clone(),
        }
    } else {
        debug!("{}: No source URL or pname found", attr_path);
        return Ok(UpdateResult::Skipped("No source info".to_string()));
    };

    // Fetch latest compatible release (using Latest strategy)
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
            return Ok(UpdateResult::Skipped(
                "Could not fetch upstream".to_string(),
            ));
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
        return Ok(UpdateResult::NoUpdateNeeded {
            current_version: current_version.to_string(),
            latest_version: latest_version.to_string(),
        });
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
                return Ok(UpdateResult::Skipped("Update already proposed".to_string()));
            } else {
                // Proposed version differs from latest - attempt new update
                info!(
                    "{}: New version {} available (previously proposed {})",
                    attr_path, latest_version, proposed
                );
            }
        }
    }

    // Update is needed - attempt the update
    info!(
        "{}: Update available: {} -> {}",
        attr_path, current_version, latest_version
    );

    // If dry-run mode, report the update without performing it
    if dry_run {
        return Ok(UpdateResult::DryRun {
            current_version: current_version.to_string(),
            new_version: latest_version.to_string(),
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
            info!("{}: Successfully updated to {}", attr_path, latest_version);

            // Record successful update first
            if let Err(e) = db
                .record_successful_update(attr_path, current_version, &latest_version)
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
                    &latest_version,
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
                new_version: latest_version.to_string(),
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
                    &drv.drv_path,
                    attr_path,
                    &error_message,
                    Some(current_version),
                    Some(&latest_version),
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
        &fork,
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
