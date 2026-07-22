use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::preservation::preserve_failure;
use super::types::{UpdateRequest, UpdateResult};
use crate::commands::pr_enhancements::PrEnhancementsConfig;
use crate::commands::update::errors::UpdateError;
use crate::commands::update::types::UpdatePhase;
use crate::database::Database;
use crate::git::{PrConfig, cleanup_worktree, create_worktree};
use crate::nix::{eval_nix_expr, normalize_entry_point};
use crate::package::PackageMetadata;
use crate::vcs_sources::SemverStrategy;

/// Perform a package update
pub(super) async fn perform_update(
    db: &Database,
    session_id: &str,
    eval_entry_point: &str,
    req: &UpdateRequest,
    pr_config: Option<&PrConfig>,
    fork: &str,
    run_passthru_tests: bool,
    dry_run: bool,
    pr_enhancements: &PrEnhancementsConfig,
    interactive: bool,
    preserve_failures: bool,
) -> anyhow::Result<UpdateResult> {
    let attr_path = &req.attr_path;
    let current_version = &req.current_version;
    let new_version = &req.new_version;

    // If dry-run mode, report the update without performing it
    if dry_run {
        return Ok(UpdateResult::DryRun {
            current_version: current_version.clone(),
            new_version: new_version.clone(),
        });
    }

    // Create a worktree for this update
    let worktree_path = match create_worktree(attr_path).await {
        Ok(path) => path,
        Err(e) => {
            warn!("{}: Failed to create worktree: {}", attr_path, e);
            return Ok(UpdateResult::Skipped(format!(
                "Worktree creation failed: {e}"
            )));
        },
    };

    // Get file location from meta.position (in the main repository)
    let file_location = match get_file_location(eval_entry_point, attr_path).await {
        Ok(loc) => loc,
        Err(e) => {
            warn!("{}: Failed to get file location: {}", attr_path, e);
            cleanup_worktree(&worktree_path).await.ok();
            return Ok(UpdateResult::Skipped("Could not locate file".to_owned()));
        },
    };

    debug!("{}: File location: {}", attr_path, file_location);

    // Convert the file path to be relative to the worktree
    let worktree_file_path = worktree_path.join(&file_location);
    let worktree_file_str = worktree_file_path.to_string_lossy().to_string();

    // Attempt the update in the worktree
    let version_config = crate::commands::update::VersionConfig::new(SemverStrategy::Latest);
    let update_config = crate::commands::update::UpdateConfig {
        commit: false,             // Don't auto-commit in run mode
        create_pr: false,          // Don't create PR here (handled separately)
        upstream: None,            // upstream - not needed in run mode
        fork: "origin".to_owned(), // fork - not used since create_pr is false
        run_passthru_tests,
        src_only: false, // Update all dependencies (not src-only)
        format: false,   // No formatting in run mode (worktree cleanup would lose it)
        // Directory diff is handled separately in run mode using the worktree-aware method.
        pr_enhancements: PrEnhancementsConfig::default(),
    };

    let update_result = update_config
        .update_from_file_path(
            eval_entry_point.to_owned(),
            attr_path.clone(),
            worktree_file_str,
            version_config,
        )
        .await;

    match update_result {
        Ok((removed_patches, test_result)) => {
            // Update succeeded (build passed; tests may or may not have)
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

            let tests_failed = test_result.failed();

            // Analyze rebuild impact if requested
            let rebuild_analysis = if pr_enhancements.analyze_rebuilds {
                match crate::nix::rebuild_count::calculate_rebuild_count(
                    eval_entry_point,
                    &worktree_path,
                )
                .await
                {
                    Ok(analysis) => {
                        info!("{}: {}", attr_path, analysis.summary());

                        // Check if rebuild count exceeds threshold
                        if pr_enhancements.should_skip_for_rebuilds(analysis.rebuild_count) {
                            let threshold = pr_enhancements.max_rebuilds.unwrap_or_default();
                            info!(
                                "{}: Skipping update - rebuild count {} exceeds threshold {}",
                                attr_path, analysis.rebuild_count, threshold
                            );

                            // Clean up the worktree
                            if let Err(e) = cleanup_worktree(&worktree_path).await {
                                warn!("{}: Failed to clean up worktree: {}", attr_path, e);
                            }

                            return Ok(UpdateResult::Skipped(format!(
                                "Rebuild count {} exceeds threshold {}",
                                analysis.rebuild_count, threshold
                            )));
                        }

                        Some(analysis)
                    },
                    Err(e) => {
                        warn!("{}: Failed to calculate rebuild count: {}", attr_path, e);
                        None
                    },
                }
            } else {
                None
            };

            // Record successful update first (with rebuild count if available)
            let db_rebuild_count = rebuild_analysis.as_ref().map(|a| a.rebuild_count);
            if let Err(e) = db
                .record_successful_update_with_rebuild_count(
                    attr_path,
                    current_version,
                    new_version,
                    db_rebuild_count,
                )
                .await
            {
                warn!("{}: Failed to record successful update: {}", attr_path, e);
            }

            // Create PR if configured
            if let Some(config) = pr_config {
                // In interactive mode, show PR details and prompt for confirmation
                let should_create_pr = if interactive {
                    match prompt_for_pr_confirmation(
                        &worktree_path,
                        attr_path,
                        current_version,
                        new_version,
                        config,
                        fork,
                        rebuild_analysis.as_ref(),
                        pr_enhancements,
                    )
                    .await
                    {
                        Ok(confirmed) => confirmed,
                        Err(e) => {
                            warn!("{}: Failed to prepare PR preview: {}", attr_path, e);
                            false
                        },
                    }
                } else {
                    true
                };

                if should_create_pr {
                    match create_pr_for_update(
                        db,
                        &worktree_path,
                        attr_path,
                        current_version,
                        new_version,
                        config,
                        fork,
                        rebuild_analysis.as_ref(),
                        pr_enhancements,
                        &test_result,
                    )
                    .await
                    {
                        Ok((pr_url, pr_number)) => {
                            if tests_failed {
                                info!(
                                    "{}: Created draft PR #{} (passthru.tests failed): {}",
                                    attr_path, pr_number, pr_url
                                );
                            } else {
                                info!("{}: Created PR #{}: {}", attr_path, pr_number, pr_url);
                            }
                        },
                        Err(e) => {
                            warn!("{}: Failed to create PR: {}", attr_path, e);
                            // Don't fail the update if PR creation fails
                        },
                    }
                } else {
                    info!("{}: Skipping PR creation (user declined)", attr_path);
                }
            }

            // Clean up the worktree
            if let Err(e) = cleanup_worktree(&worktree_path).await {
                warn!("{}: Failed to clean up worktree: {}", attr_path, e);
            }

            Ok(UpdateResult::Updated {
                old_version: current_version.clone(),
                new_version: new_version.clone(),
            })
        },
        Err(e) => {
            // Update failed - record the failure log
            let error_message = format!("{e:#}");
            warn!("{}: Update failed: {}", attr_path, error_message);

            // Preserve failure artifacts if requested
            let artifacts_path = if preserve_failures {
                // Create a structured error from the anyhow error
                // For now, use a generic InfrastructureError since we don't have granular error
                // info
                let update_error = UpdateError::InfrastructureError {
                    phase: UpdatePhase::Build, // Default to Build phase
                    component: "update".to_string(),
                    details: error_message.clone(),
                };

                match preserve_failure(
                    session_id,
                    attr_path,
                    UpdatePhase::Build, // We don't know exact phase, default to Build
                    &worktree_path,
                    &update_error,
                    Some(error_message.clone()), // Build/update error message
                    None,                        // Test output requires deeper capture plumbing
                )
                .await
                {
                    Ok(artifacts) => {
                        info!("{}: Preserved failure artifacts", attr_path);
                        Some(
                            artifacts
                                .worktree_path
                                .parent()
                                .and_then(|p| p.parent())
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| {
                                    artifacts.worktree_path.to_string_lossy().to_string()
                                }),
                        )
                    },
                    Err(preserve_err) => {
                        warn!(
                            "{}: Failed to preserve failure artifacts: {}",
                            attr_path, preserve_err
                        );
                        None
                    },
                }
            } else {
                None
            };

            // Clean up the worktree (only if not preserved)
            if artifacts_path.is_none() {
                if let Err(cleanup_err) = cleanup_worktree(&worktree_path).await {
                    warn!(
                        "{}: Failed to clean up worktree: {}",
                        attr_path, cleanup_err
                    );
                }
            } else {
                debug!(
                    "{}: Skipping worktree cleanup (preserved for inspection)",
                    attr_path
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
            Ok(UpdateResult::Skipped(format!("Update failed: {e}")))
        },
    }
}

/// Do additional processing depending on the result of the update
pub(super) fn handle_result(result: anyhow::Result<UpdateResult>, attr_path: &str) {
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

/// Perform a package update directly in the working tree (no worktree isolation).
/// Used in commit mode where changes are committed directly to the repo.
/// On failure, reverts dirty changes with `git checkout -- .`.
pub(super) async fn perform_direct_update(
    db: &Database,
    _session_id: &str,
    eval_entry_point: &str,
    req: &UpdateRequest,
    run_passthru_tests: bool,
    git_mutex: Arc<Mutex<()>>,
) -> anyhow::Result<UpdateResult> {
    let attr_path = &req.attr_path;
    let current_version = &req.current_version;
    let new_version = &req.new_version;

    // Get file location from meta.position
    let file_location = match get_file_location(eval_entry_point, attr_path).await {
        Ok(loc) => loc,
        Err(e) => {
            warn!("{}: Failed to get file location: {}", attr_path, e);
            return Ok(UpdateResult::Skipped("Could not locate file".to_owned()));
        },
    };

    debug!("{}: File location: {}", attr_path, file_location);

    // Perform the update directly in the working tree (commit handled separately under mutex)
    let version_config = crate::commands::update::VersionConfig::new(SemverStrategy::Latest);
    let update_config = crate::commands::update::UpdateConfig {
        commit: false,
        create_pr: false,
        upstream: None,
        fork: "origin".to_owned(),
        run_passthru_tests,
        src_only: false,
        format: false,
        pr_enhancements: crate::commands::pr_enhancements::PrEnhancementsConfig::default(),
    };

    let update_result = update_config
        .update_from_file_path(
            eval_entry_point.to_owned(),
            attr_path.clone(),
            file_location.clone(),
            version_config,
        )
        .await;

    // Determine the package directory for git operations
    let pkg_dir = std::path::Path::new(&file_location)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| file_location.clone());

    match update_result {
        Ok((removed_patches, test_result)) => {
            // In branch mode, test failures should fail the update so the
            // changes are reverted and recorded for an agent to fix later.
            if let crate::commands::update::TestResult::Failed(ref stderr) = test_result {
                warn!("{}: passthru.tests failed after update", attr_path);

                // Revert changes under the mutex
                {
                    let _lock = git_mutex.lock().await;
                    info!("{}: Reverting {}...", attr_path, pkg_dir);
                    let revert_output = tokio::process::Command::new("git")
                        .args(["checkout", "--", &pkg_dir])
                        .output()
                        .await;

                    match revert_output {
                        Ok(output) if output.status.success() => {
                            debug!("{}: Successfully reverted changes", attr_path);
                        },
                        Ok(output) => {
                            let stderr_msg = String::from_utf8_lossy(&output.stderr);
                            warn!("{}: Failed to revert changes: {}", attr_path, stderr_msg);
                        },
                        Err(revert_err) => {
                            warn!("{}: Failed to run git checkout: {}", attr_path, revert_err);
                        },
                    }
                }

                // Record as a test failure (distinct from build failure)
                let error_message =
                    format!("passthru.tests failed after update to {new_version}:\n{stderr}");
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
                    warn!("{}: Failed to record test failure: {}", attr_path, db_err);
                }

                return Ok(UpdateResult::Skipped(format!(
                    "passthru.tests failed after update to {new_version}"
                )));
            }

            info!("{}: Successfully updated to {}", attr_path, new_version);

            if !removed_patches.is_empty() {
                info!(
                    "{}: Removed {} obsolete patch(es): {}",
                    attr_path,
                    removed_patches.len(),
                    removed_patches.join(", ")
                );
            }

            // Serialize git operations under the mutex to prevent concurrent
            // commits from staging each other's changes
            {
                let _lock = git_mutex.lock().await;

                let add_output = tokio::process::Command::new("git")
                    .args(["add", &pkg_dir])
                    .output()
                    .await;

                match &add_output {
                    Ok(output) if !output.status.success() => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        warn!("{}: git add failed: {}", attr_path, stderr);
                    },
                    Err(e) => warn!("{}: Failed to stage changes: {}", attr_path, e),
                    _ => {},
                }

                let commit_msg = format!("{attr_path}: {current_version} -> {new_version}");
                let commit_output = tokio::process::Command::new("git")
                    .args(["commit", "-m", &commit_msg])
                    .output()
                    .await;

                match commit_output {
                    Ok(output) if output.status.success() => {
                        info!("{}: Committed: {}", attr_path, commit_msg);
                    },
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        warn!("{}: git commit failed: {}", attr_path, stderr);
                    },
                    Err(e) => {
                        warn!("{}: Failed to run git commit: {}", attr_path, e);
                    },
                }
            }

            // Record successful update in database
            if let Err(e) = db
                .record_successful_update_with_rebuild_count(
                    attr_path,
                    current_version,
                    new_version,
                    None,
                )
                .await
            {
                warn!("{}: Failed to record successful update: {}", attr_path, e);
            }

            Ok(UpdateResult::Updated {
                old_version: current_version.clone(),
                new_version: new_version.clone(),
            })
        },
        Err(e) => {
            let error_message = format!("{e:#}");
            warn!("{}: Update failed: {}", attr_path, error_message);

            // Revert only this package's files under the mutex
            {
                let _lock = git_mutex.lock().await;
                info!("{}: Reverting {}...", attr_path, pkg_dir);
                let revert_output = tokio::process::Command::new("git")
                    .args(["checkout", "--", &pkg_dir])
                    .output()
                    .await;

                match revert_output {
                    Ok(output) if output.status.success() => {
                        debug!("{}: Successfully reverted changes", attr_path);
                    },
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        warn!("{}: Failed to revert changes: {}", attr_path, stderr);
                    },
                    Err(revert_err) => {
                        warn!("{}: Failed to run git checkout: {}", attr_path, revert_err);
                    },
                }
            }

            // Record failure in database
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
            Ok(UpdateResult::Skipped(format!("Update failed: {e}")))
        },
    }
}

/// Get the file location for a package from meta.position
async fn get_file_location(eval_entry_point: &str, attr_path: &str) -> anyhow::Result<String> {
    let normalized_entry = normalize_entry_point(eval_entry_point);
    let position_expr = format!("with import {normalized_entry} {{ }}; {attr_path}.meta.position");

    let position = eval_nix_expr(&position_expr).await?;

    if position.is_empty() {
        anyhow::bail!("Empty position returned from meta.position");
    }

    // Parse position string (format: "file:line")
    let (file_path, _line_str) = position
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {position}"))?;

    Ok(file_path.to_owned())
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
    rebuild_analysis: Option<&crate::nix::rebuild_count::RebuildAnalysis>,
    pr_enhancements: &PrEnhancementsConfig,
    test_result: &crate::commands::update::TestResult,
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

    // Analyze CVE changes if metadata is available and CVE checking is enabled
    let cve_analysis = if let Some(ref meta) = metadata {
        crate::cve::analyze_cve_changes(
            db.pool(),
            meta,
            old_version,
            new_version,
            pr_enhancements.skip_cve_check,
        )
        .await
        .ok()
    } else {
        None
    };

    // Push build outputs to Cachix (if enabled) before the directory diff
    // step, since the diff pass runs `git checkout HEAD~1` and would
    // invalidate the worktree state we want to push from. Failures are
    // swallowed: Cachix is a non-blocking enhancement, and the result is
    // intentionally not surfaced in the PR body — reviewers of nixpkgs-style
    // updates don't typically need the raw store paths or `cachix use`
    // boilerplate, and the cache name is already visible in the deployment.
    if let Err(e) = pr_enhancements
        .perform_worktree_cachix_push(worktree_path, &eval_entry_point, attr_path)
        .await
    {
        warn!("{}: Cachix push raised an error: {:#}", attr_path, e);
    }

    // Audit the built package if requested
    let audit_markdown = match pr_enhancements
        .perform_worktree_audit(worktree_path, &eval_entry_point, attr_path)
        .await
    {
        Ok(md) => md,
        Err(e) => {
            warn!("{}: Audit failed: {:#}", attr_path, e);
            None
        },
    };

    // Perform directory diff if requested
    let diff_markdown = pr_enhancements
        .perform_worktree_directory_diff(worktree_path, &eval_entry_point, attr_path)
        .await
        .ok()
        .flatten();

    // Create PR title and body
    let title = format!("Update {attr_path} from {old_version} to {new_version}");
    let mut body = format!(
        "## Summary\n\nThis PR updates `{attr_path}` from version {old_version} to \
         {new_version}.\n\n## Changes\n\n- Updated package version\n- Updated source hash"
    );

    // Add optional metadata fields if available
    if let Some(meta) = metadata.as_ref() {
        body.push_str("\n\n## Package Information");

        if let Some(description) = &meta.description {
            body.push_str(&format!("\n\n**Description:** {description}"));
        }

        if let Some(homepage) = &meta.homepage {
            body.push_str(&format!("\n\n**Homepage:** {homepage}"));
        }

        if let Some(changelog) = &meta.changelog {
            body.push_str(&format!("\n\n**Changelog:** {changelog}"));
        }
    }

    // Add rebuild impact analysis if available
    if let Some(analysis) = rebuild_analysis {
        body.push_str("\n\n## Rebuild Impact\n\n");
        body.push_str(&format!(
            "- **Packages affected:** {}\n",
            analysis.rebuild_count
        ));
        body.push_str(&format!(
            "- **Impact:** {:.1}% of {} total packages\n",
            analysis.rebuild_percentage(),
            analysis.total_packages
        ));

        if !analysis.new_packages.is_empty() {
            body.push_str(&format!(
                "- **New packages:** {}\n",
                analysis.new_packages.len()
            ));
        }

        if !analysis.removed_packages.is_empty() {
            body.push_str(&format!(
                "- **Removed packages:** {}\n",
                analysis.removed_packages.len()
            ));
        }

        // List affected packages if the count is reasonable
        if analysis.rebuild_count > 0 && analysis.rebuild_count <= 20 {
            body.push_str("\n### Affected packages:\n");
            for pkg in &analysis.rebuilt_packages {
                body.push_str(&format!("- `{pkg}`\n"));
            }
        } else if analysis.rebuild_count > 20 {
            body.push_str(&format!(
                "\n<details>\n<summary>Show all {} affected packages</summary>\n\n",
                analysis.rebuild_count
            ));
            for pkg in &analysis.rebuilt_packages {
                body.push_str(&format!("- `{pkg}`\n"));
            }
            body.push_str("\n</details>\n");
        }
    }

    // Add CVE analysis if available
    if let Some(analysis) = cve_analysis {
        if let Some(cve_section) = analysis.to_markdown() {
            body.push_str("\n\n");
            body.push_str(&cve_section);
        }
    }

    // Add directory diff if available
    if let Some(diff) = diff_markdown {
        body.push_str("\n\n");
        body.push_str(&diff);
    }

    // Add audit results if available
    if let Some(audit) = audit_markdown {
        body.push_str("\n\n");
        body.push_str(&audit);
    }

    // Add test results section
    let draft = match test_result {
        crate::commands::update::TestResult::Passed => {
            body.push_str("\n\n## Tests\n\n✅ `passthru.tests` passed");
            false
        },
        crate::commands::update::TestResult::Failed(stderr) => {
            body.push_str("\n\n## Tests\n\n❌ `passthru.tests` **failed**\n\n");
            body.push_str(
                "This PR is opened as a draft because the package's passthru.tests did not pass \
                 after the update.\n\n",
            );
            // Truncate very long output
            let truncated = if stderr.len() > 4000 {
                format!("{}...\n\n(truncated)", &stderr[..4000])
            } else {
                stderr.clone()
            };
            body.push_str(&format!(
                "<details>\n<summary>Test output</summary>\n\n```\n{truncated}\n```\n\n</details>"
            ));
            true
        },
        _ => false,
    };

    body.push_str("\n\n🤖 Generated with ekapkgs-update");

    // Create PR via GitHub API (draft if tests failed)
    let pr = crate::github::create_pull_request_with_options(
        &config.owner,
        &config.repo,
        &title,
        &body,
        &branch_name,
        &config.base_branch,
        &github_token,
        draft,
    )
    .await?;

    // Record PR info in database
    db.record_pr_info(attr_path, &pr.html_url, pr.number)
        .await?;

    Ok((pr.html_url, pr.number))
}

/// Prompt user for confirmation before creating a PR (interactive mode)
async fn prompt_for_pr_confirmation(
    worktree_path: &Path,
    attr_path: &str,
    old_version: &str,
    new_version: &str,
    config: &PrConfig,
    fork: &str,
    rebuild_analysis: Option<&crate::nix::rebuild_count::RebuildAnalysis>,
    _pr_enhancements: &PrEnhancementsConfig,
) -> anyhow::Result<bool> {
    use std::io::{self, Write};

    use tokio::process::Command;

    // Create branch name (same logic as in create_pr_for_update)
    let sanitized_attr = attr_path.replace(['.', '/'], "-");
    let branch_name = format!("update/{sanitized_attr}/{new_version}");

    // Create PR title
    let title = format!("Update {attr_path} from {old_version} to {new_version}");

    // Build PR body (same logic as in create_pr_for_update, but simplified for preview)
    let mut body = format!(
        "## Summary\n\nThis PR updates `{attr_path}` from version {old_version} to \
         {new_version}.\n\n## Changes\n\n- Updated package version\n- Updated source hash"
    );

    // Add rebuild impact if available
    if let Some(analysis) = rebuild_analysis {
        body.push_str("\n\n## Rebuild Impact\n\n");
        body.push_str(&format!(
            "- **Packages affected:** {}\n",
            analysis.rebuild_count
        ));
        body.push_str(&format!(
            "- **Impact:** {:.1}% of {} total packages\n",
            analysis.rebuild_percentage(),
            analysis.total_packages
        ));
    }

    body.push_str("\n\n🤖 Generated with ekapkgs-update");

    // Get commits in the worktree
    let commits_output = Command::new("git")
        .args(["-C", &worktree_path.to_string_lossy(), "log", "--oneline"])
        .output()
        .await?;

    let commits = if commits_output.status.success() {
        String::from_utf8_lossy(&commits_output.stdout).to_string()
    } else {
        "(Unable to retrieve commits)".to_owned()
    };

    // Display PR information
    println!("\n{}", "=".repeat(80));
    println!("PR PREVIEW for {}", attr_path);
    println!("{}", "=".repeat(80));
    println!("\nTitle: {}", title);
    println!(
        "\nTarget: {}/{} (branch: {})",
        config.owner, config.repo, config.base_branch
    );
    println!("Push to: {} (as branch: {})", fork, branch_name);
    println!("\n--- PR Body ---");
    println!("{}", body);
    println!("\n--- Commits ---");
    println!("{}", commits.trim());
    println!("{}", "=".repeat(80));

    // Prompt for confirmation
    print!("\nCreate this PR? [y/N]: ");
    io::stdout().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;

    let response = response.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}
