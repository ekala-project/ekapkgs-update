use futures::{StreamExt, pin_mut};
use std::collections::HashMap;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::database::Database;
use crate::git::{PrConfig, cleanup_worktree, create_worktree};
use crate::groups::GroupsData;
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
    skip_unstable: bool,
    grouping_file: Option<String>,
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

    // Load and parse grouping file if provided
    let groups_data = if let Some(ref path) = grouping_file {
        info!("Loading grouping file: {}", path);
        let data = GroupsData::load_from_file(path).await?;
        info!("Loaded grouping configuration");
        Some(data)
    } else {
        None
    };
    let groupings = groups_data.as_ref().map(|data| data.build_index());

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

    // Track grouped packages: group_name -> Vec<NixEvalDrv>
    let mut grouped_packages: HashMap<String, Vec<crate::nix::nix_eval_jobs::NixEvalDrv>> = HashMap::new();

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

                // Check if this package belongs to a group
                if let Some(ref groupings_ref) = groupings {
                    if let Some(group_name) = groupings_ref.group_name(attr_path) {
                        // Add to grouped packages for later batch processing
                        debug!("{}: Adding to group '{}'", attr_path, group_name);
                        grouped_packages
                            .entry(group_name.clone())
                            .or_insert_with(Vec::new)
                            .push(drv.clone());
                        continue;
                    }
                }

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

                // Spawn the update task for individual package
                join_set.spawn(async move {
                    let result = check_and_update_package(
                        &db_clone,
                        &file_clone,
                        &drv_clone,
                        pr_config_clone.as_ref(),
                        &fork_clone,
                        run_passthru_tests,
                        dry_run,
                        skip_unstable,
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

    // Process grouped packages
    if !grouped_packages.is_empty() {
        info!("Processing {} package groups", grouped_packages.len());
        for (group_name, packages) in grouped_packages {
            info!("Processing group '{}' with {} packages", group_name, packages.len());

            let result = process_grouped_update(
                &db,
                &file,
                &group_name,
                packages,
                pr_config.as_ref(),
                &fork,
                run_passthru_tests,
                dry_run,
                skip_unstable,
            )
            .await;

            match result {
                Ok(count) => {
                    info!("Group '{}': Updated {} packages", group_name, count);
                    updated_count += count;
                },
                Err(e) => {
                    warn!("Group '{}': Failed to process: {}", group_name, e);
                    failed_count += 1;
                },
            }
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
    skip_unstable: bool,
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

    // Skip packages with 'unstable' in version if flag is set
    if skip_unstable && current_version.contains("unstable") {
        debug!(
            "{}: Skipping due to --skip-unstable flag (version: {})",
            attr_path, current_version
        );
        return Ok(UpdateResult::Skipped(
            "Version contains 'unstable'".to_string(),
        ));
    }

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

/// Process a group of packages together in one worktree and PR
/// Returns the count of successfully updated packages
async fn process_grouped_update(
    db: &Database,
    eval_entry_point: &str,
    group_name: &str,
    packages: Vec<crate::nix::nix_eval_jobs::NixEvalDrv>,
    pr_config: Option<&PrConfig>,
    fork: &str,
    run_passthru_tests: bool,
    dry_run: bool,
    skip_unstable: bool,
) -> anyhow::Result<usize> {
    if packages.is_empty() {
        return Ok(0);
    }

    info!("Group '{}': Starting batch update for {} packages", group_name, packages.len());

    // Create a single worktree for the entire group
    let worktree_path = create_worktree(group_name).await?;

    let mut successful_updates: Vec<(String, String, String)> = Vec::new(); // (attr_path, old_version, new_version)
    let mut failed_updates: Vec<(String, String)> = Vec::new(); // (attr_path, error)

    // Process each package in the group
    for drv in &packages {
        let attr_path = &drv.attr;
        info!("Group '{}': Processing {}", group_name, attr_path);

        // Check if we should skip this package (backoff, unstable, etc.)
        match db.should_check_update(attr_path).await {
            Ok(false) => {
                debug!("Group '{}': {}: Skipping (in backoff period)", group_name, attr_path);
                continue;
            },
            Ok(true) => {},
            Err(e) => {
                warn!("Group '{}': {}: Database error: {}", group_name, attr_path, e);
            },
        }

        // Extract package metadata
        let metadata = match PackageMetadata::from_attr_path(eval_entry_point, attr_path).await {
            Ok(m) => m,
            Err(e) => {
                debug!("Group '{}': {}: Failed to extract metadata: {}", group_name, attr_path, e);
                failed_updates.push((attr_path.clone(), format!("Could not extract metadata: {}", e)));
                continue;
            },
        };

        let current_version = &metadata.version;

        // Skip packages with 'unstable' in version if flag is set
        if skip_unstable && current_version.contains("unstable") {
            debug!("Group '{}': {}: Skipping (unstable version)", group_name, attr_path);
            continue;
        }

        // Determine upstream source
        let upstream_source = if let Some(ref src_url) = metadata.src_url {
            match UpstreamSource::from_url(src_url) {
                Some(source) => source,
                None => {
                    debug!("Group '{}': {}: Could not parse upstream source", group_name, attr_path);
                    failed_updates.push((attr_path.clone(), "Unsupported source".to_string()));
                    continue;
                },
            }
        } else if let Some(ref pname) = metadata.pname {
            UpstreamSource::PyPI {
                pname: pname.clone(),
            }
        } else {
            debug!("Group '{}': {}: No source URL or pname", group_name, attr_path);
            failed_updates.push((attr_path.clone(), "No source info".to_string()));
            continue;
        };

        // Fetch latest compatible release
        let best_release = match upstream_source
            .get_compatible_release(current_version, SemverStrategy::Latest)
            .await
        {
            Ok(release) => release,
            Err(e) => {
                debug!("Group '{}': {}: Failed to fetch upstream: {}", group_name, attr_path, e);
                failed_updates.push((attr_path.clone(), format!("Could not fetch upstream: {}", e)));
                continue;
            },
        };

        let latest_version = UpstreamSource::get_version(&best_release);

        // Check if update is needed
        if current_version == &latest_version {
            debug!("Group '{}': {}: No update needed ({})", group_name, attr_path, current_version);
            if let Err(e) = db.record_no_update(attr_path, current_version, &latest_version).await {
                warn!("Group '{}': {}: Failed to record no update: {}", group_name, attr_path, e);
            }
            continue;
        }

        // Check if already proposed
        let record = db.get_update_record(attr_path).await?;
        if let Some(ref rec) = record {
            if let Some(ref proposed) = rec.proposed_version {
                if proposed == &latest_version {
                    debug!("Group '{}': {}: Already proposed {}", group_name, attr_path, proposed);
                    continue;
                }
            }
        }

        info!("Group '{}': {}: Update available {} -> {}", group_name, attr_path, current_version, latest_version);

        if dry_run {
            info!("Group '{}': {}: Would update {} -> {}", group_name, attr_path, current_version, latest_version);
            successful_updates.push((attr_path.clone(), current_version.to_string(), latest_version.to_string()));
            continue;
        }

        // Get file location and convert to worktree path
        let file_location = match get_file_location(eval_entry_point, attr_path).await {
            Ok(loc) => loc,
            Err(e) => {
                warn!("Group '{}': {}: Failed to get file location: {}", group_name, attr_path, e);
                failed_updates.push((attr_path.clone(), format!("Could not locate file: {}", e)));
                continue;
            },
        };

        let worktree_file_path = worktree_path.join(&file_location);
        let worktree_file_str = worktree_file_path.to_string_lossy().to_string();

        // Attempt the update
        let update_result = crate::commands::update::update_from_file_path(
            eval_entry_point.to_string(),
            attr_path.to_string(),
            worktree_file_str,
            SemverStrategy::Latest,
            true,                  // Auto-commit each update in the group
            false,                 // Don't create PR yet (handled at group level)
            None,
            "origin".to_string(),
            run_passthru_tests,
            run_passthru_tests,
        )
        .await;

        match update_result {
            Ok(()) => {
                info!("Group '{}': {}: Successfully updated to {}", group_name, attr_path, latest_version);
                successful_updates.push((attr_path.clone(), current_version.to_string(), latest_version.to_string()));

                // Record successful update
                if let Err(e) = db.record_successful_update(attr_path, current_version, &latest_version).await {
                    warn!("Group '{}': {}: Failed to record update: {}", group_name, attr_path, e);
                }
            },
            Err(e) => {
                let error_msg = format!("{:#}", e);
                warn!("Group '{}': {}: Update failed: {}", group_name, attr_path, error_msg);
                failed_updates.push((attr_path.clone(), error_msg.clone()));

                // Record failure
                if let Err(db_err) = db.record_failed_update(
                    &drv.drv_path,
                    attr_path,
                    &error_msg,
                    Some(current_version),
                    Some(&latest_version),
                ).await {
                    warn!("Group '{}': {}: Failed to record failure: {}", group_name, attr_path, db_err);
                }
            },
        }
    }

    // Build all successfully updated packages
    if !successful_updates.is_empty() && !dry_run {
        info!("Group '{}': Building {} updated packages", group_name, successful_updates.len());

        for (attr_path, _, _) in &successful_updates {
            info!("Group '{}': Building {}", group_name, attr_path);

            // Build the package in the worktree
            let build_result = tokio::process::Command::new("nix-build")
                .arg(eval_entry_point)
                .arg("-A")
                .arg(attr_path)
                .current_dir(&worktree_path)
                .output()
                .await;

            match build_result {
                Ok(output) if output.status.success() => {
                    info!("Group '{}': {}: Build successful", group_name, attr_path);
                },
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    warn!("Group '{}': {}: Build failed: {}", group_name, attr_path, stderr);
                    // Note: We continue even if build fails, as per user preference
                },
                Err(e) => {
                    warn!("Group '{}': {}: Failed to run build: {}", group_name, attr_path, e);
                },
            }
        }
    }

    // Create PR if we have successful updates and PR config is available
    if !successful_updates.is_empty() && !dry_run {
        if let Some(config) = pr_config {
            match create_grouped_pr(
                db,
                &worktree_path,
                group_name,
                &successful_updates,
                &failed_updates,
                config,
                fork,
            )
            .await
            {
                Ok((pr_url, pr_number)) => {
                    info!("Group '{}': Created PR #{}: {}", group_name, pr_number, pr_url);
                },
                Err(e) => {
                    warn!("Group '{}': Failed to create PR: {}", group_name, e);
                },
            }
        }
    }

    // Clean up worktree
    if let Err(e) = cleanup_worktree(&worktree_path).await {
        warn!("Group '{}': Failed to clean up worktree: {}", group_name, e);
    }

    let success_count = successful_updates.len();
    if !failed_updates.is_empty() {
        info!("Group '{}': {} succeeded, {} failed", group_name, success_count, failed_updates.len());
    }

    Ok(success_count)
}

/// Create a pull request for a group of updates
async fn create_grouped_pr(
    db: &Database,
    worktree_path: &std::path::Path,
    group_name: &str,
    successful_updates: &[(String, String, String)], // (attr_path, old_version, new_version)
    failed_updates: &[(String, String)],             // (attr_path, error)
    config: &PrConfig,
    fork: &str,
) -> anyhow::Result<(String, i64)> {
    // Get GitHub token from environment
    let github_token = std::env::var("GITHUB_TOKEN")
        .map_err(|_| anyhow::anyhow!("GITHUB_TOKEN environment variable not set"))?;

    // Create and push branch with group name
    let branch_name = format!("update/{}", group_name);
    info!("Creating branch: {}", branch_name);

    // Create new branch (commits were already created by update_from_file_path)
    let output = tokio::process::Command::new("git")
        .current_dir(worktree_path)
        .args(["checkout", "-b", &branch_name])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create branch '{}': {}", branch_name, stderr);
    }

    // Push to remote
    let push_target = format!("{}:{}", branch_name, branch_name);
    let output = tokio::process::Command::new("git")
        .current_dir(worktree_path)
        .args(["push", "-u", fork, &push_target])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to push branch '{}': {}", branch_name, stderr);
    }

    // Build PR title and body
    let title = format!("Update {} packages", group_name);

    let mut body = String::new();
    body.push_str("## Summary\n\n");
    body.push_str(&format!("This PR updates {} packages in the `{}` group:\n\n", successful_updates.len(), group_name));

    for (attr_path, old_version, new_version) in successful_updates {
        body.push_str(&format!("- **{}**: {} → {}\n", attr_path, old_version, new_version));
    }

    if !failed_updates.is_empty() {
        body.push_str(&format!("\n### Failed Updates ({})\n\n", failed_updates.len()));
        body.push_str("The following packages could not be updated:\n\n");
        for (attr_path, error) in failed_updates {
            body.push_str(&format!("- **{}**: {}\n", attr_path, error));
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

    // Record PR info for each successfully updated package
    for (attr_path, _, new_version) in successful_updates {
        if let Err(e) = db.record_pr_info(attr_path, &pr.html_url, pr.number).await {
            warn!("Failed to record PR info for {}: {}", attr_path, e);
        }
        // Also record the proposed version
        if let Err(e) = db.record_successful_update(attr_path, "", new_version).await {
            warn!("Failed to update proposed version for {}: {}", attr_path, e);
        }
    }

    Ok((pr.html_url, pr.number))
}


