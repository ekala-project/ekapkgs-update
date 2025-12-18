use futures::{StreamExt, pin_mut};
use tracing::{debug, info, warn};

use crate::database::Database;
use crate::git::{PrConfig, cleanup_worktree, create_worktree};
use crate::nix;
use crate::nix::eval_nix_expr;
use crate::nix::nix_eval_jobs::NixEvalItem;
use crate::package::PackageMetadata;
use crate::vcs_sources::{SemverStrategy, UpstreamSource};

pub async fn run(
    file: String,
    database_path: String,
    pr_repo: Option<String>,
) -> anyhow::Result<()> {
    info!("Running nix-eval-jobs on: {}", file);

    // Expand tilde in database path
    let expanded_db_path = shellexpand::tilde(&database_path).to_string();

    // Initialize database
    let db = Database::new(&expanded_db_path).await?;
    info!("Database initialized at: {}", expanded_db_path);

    // Determine PR configuration: use CLI override or auto-detect from git
    let pr_config = if let Some(repo_str) = pr_repo {
        parse_pr_config(&repo_str).await.ok()
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

                // Attempt to check for updates
                match check_and_update_package(&db, &file, &drv, pr_config.as_ref()).await {
                    Ok(UpdateResult::Updated {
                        old_version,
                        new_version,
                    }) => {
                        info!(
                            "{}: Updated from {} to {}",
                            attr_path, old_version, new_version
                        );
                        updated_count += 1;
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
                    Err(e) => {
                        warn!("{}: Failed to check for updates: {}", attr_path, e);
                        failed_count += 1;
                    },
                }
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

    // Display summary
    info!("Evaluation complete!");
    info!("Total derivations: {}", drvs.len());
    if error_count > 0 {
        info!("Evaluation errors: {}", error_count);
    }
    info!("Update summary:");
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
}

/// Check if a package needs updating and attempt to update it
async fn check_and_update_package(
    db: &Database,
    eval_entry_point: &str,
    drv: &crate::nix::nix_eval_jobs::NixEvalDrv,
    pr_config: Option<&PrConfig>,
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
        false, // Don't auto-commit in run mode
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
    let position_expr = format!(
        "with import ./{} {{ }}; {}.meta.position",
        eval_entry_point, attr_path
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
        "origin", // Push to origin remote
    )
    .await?;

    // Create PR title and body
    let title = format!(
        "Update {} from {} to {}",
        attr_path, old_version, new_version
    );
    let body = format!(
        "## Summary\n\nThis PR updates `{}` from version {} to {}.\n\n## Changes\n\n- Updated \
         package version\n- Updated source hash\n\n🤖 Generated with ekapkgs-update",
        attr_path, old_version, new_version
    );

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

/// Parse PR repository configuration from "owner/repo" format
/// Auto-detects the base branch from git configuration
async fn parse_pr_config(repo_str: &str) -> anyhow::Result<PrConfig> {
    let parts: Vec<&str> = repo_str.split('/').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid format. Expected 'owner/repo', got '{}'", repo_str);
    }

    // Try to auto-detect base branch from git
    let base_branch = crate::git::get_pr_config_from_git()
        .await
        .ok()
        .map(|config| config.base_branch)
        .unwrap_or_else(|| "master".to_string());

    Ok(PrConfig {
        owner: parts[0].to_string(),
        repo: parts[1].to_string(),
        base_branch,
    })
}
