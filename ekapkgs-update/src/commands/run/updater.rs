use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::claude_fix::{ClaudeFixConfig, ClaudeFixResult};
use super::preservation::preserve_failure;
use super::types::{UpdateRequest, UpdateResult};
use crate::commands::pr_enhancements::PrEnhancementsConfig;
use crate::commands::update::errors::UpdateError;
use crate::commands::update::instrumentation::{PhaseHandle, PhaseTracker};
use crate::commands::update::types::UpdatePhase;
use crate::database::{Database, FailedUpdate};
use crate::git::{PrConfig, cleanup_worktree, create_worktree};
use crate::nix::{eval_nix_expr, normalize_entry_point};
use crate::package::PackageMetadata;
use crate::vcs_sources::SemverStrategy;

/// Derive the repository directory from the Nix evaluation entry point
///
/// Extracts the directory containing the Nix file used as the entry point.
/// For example, `/home/user/repo/default.nix` -> `/home/user/repo`
fn get_repo_dir_from_entry_point(eval_entry_point: &str) -> anyhow::Result<PathBuf> {
    let path = std::fs::canonicalize(eval_entry_point)
        .with_context(|| format!("Cannot resolve entry point path: {eval_entry_point}"))?;
    let repo_dir = path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot determine repository directory from entry point: {}",
            path.display()
        )
    })?;
    Ok(repo_dir.to_path_buf())
}

/// Database bookkeeping for one update attempt, shared by worktree and
/// branch mode: workflow phase rows (via [`PhaseTracker`]), the package's
/// `updates` row and its failure log.
struct Attempt<'a> {
    db: &'a Database,
    session_id: &'a str,
    req: &'a UpdateRequest,
    tracker: PhaseTracker,
}

impl<'a> Attempt<'a> {
    fn new(db: &'a Database, session_id: &'a str, req: &'a UpdateRequest) -> Self {
        let tracker = PhaseTracker::new(db.clone(), session_id, req.repo(), &req.attr_path);
        Self {
            db,
            session_id,
            req,
            tracker,
        }
    }

    /// Build an [`crate::commands::update::UpdateConfig`] for running the
    /// update workflow in place (no commit / PR; handled by the caller).
    fn update_config(&self, run_passthru_tests: bool) -> crate::commands::update::UpdateConfig {
        crate::commands::update::UpdateConfig {
            commit: false,             // Don't auto-commit in run mode
            create_pr: false,          // PRs / commits are handled by the caller
            upstream: None,            // upstream - not needed in run mode
            fork: "origin".to_owned(), // fork - not used since create_pr is false
            run_passthru_tests,
            src_only: false, // Update all dependencies (not src-only)
            format: false,   // No formatting in run mode (worktree cleanup would lose it)
            // Directory diff is handled separately in run mode using the worktree-aware method.
            pr_enhancements: PrEnhancementsConfig::default(),
            tracker: self.tracker.clone(),
        }
    }

    /// The structured error of the phase that failed, falling back to a
    /// generic infrastructure error when the failure was not tracked.
    fn failure(&self, error_message: &str) -> UpdateError {
        self.tracker
            .last_failure()
            .unwrap_or_else(|| UpdateError::InfrastructureError {
                phase: UpdatePhase::Build,
                component: "update".to_owned(),
                details: error_message.to_owned(),
            })
    }

    /// Record a successful update (status `proposed`, backoff reset).
    async fn record_success(&self, rebuild_count: Option<usize>) {
        if let Err(e) = self
            .db
            .record_successful_update(
                self.req.repo(),
                &self.req.attr_path,
                &self.req.current_version,
                &self.req.new_version,
                rebuild_count,
                Some(self.session_id),
            )
            .await
        {
            warn!(
                "{}: Failed to record successful update: {}",
                self.req.attr_path, e
            );
        }
    }

    /// Record a failed attempt (failure log, `failed` status, backoff) and
    /// return the result to report.
    async fn record_failure(&self, error: &UpdateError, error_log: &str) -> UpdateResult {
        let record = FailedUpdate {
            repo_name: self.req.repo(),
            attr_path: &self.req.attr_path,
            drv_path: self.req.drv_path.as_deref(),
            session_id: Some(self.session_id),
            old_version: Some(&self.req.current_version),
            new_version: Some(&self.req.new_version),
            error_log,
            phase: Some(error.phase().as_str()),
            error_type: Some(error.error_type_name()),
        };
        if let Err(e) = self.db.record_failed_update(&record).await {
            warn!(
                "{}: Failed to record update failure: {}",
                self.req.attr_path, e
            );
        }
        UpdateResult::Failed(error.summary())
    }

    /// Fail the attempt before the update workflow ran (e.g. the package's
    /// file could not be located).
    async fn fail_setup(
        &self,
        phase: UpdatePhase,
        component: &str,
        error: &anyhow::Error,
    ) -> UpdateResult {
        let message = format!("{error:#}");
        let update_error = UpdateError::InfrastructureError {
            phase,
            component: component.to_owned(),
            details: message.clone(),
        };
        self.tracker.record_failure(update_error.clone()).await;
        self.record_failure(&update_error, &message).await
    }
}

/// Finish an `agent_fix` phase according to the Claude fix outcome.
async fn record_agent_fix(handle: PhaseHandle<'_>, result: &ClaudeFixResult) {
    let infra = |details: &str| UpdateError::InfrastructureError {
        phase: UpdatePhase::AgentFix,
        component: "claude".to_owned(),
        details: details.to_owned(),
    };
    match result {
        ClaudeFixResult::Fixed => handle.success().await,
        ClaudeFixResult::StillBroken { stderr } => {
            handle
                .fail(UpdateError::BuildError {
                    phase: UpdatePhase::AgentFix,
                    exit_code: 1,
                    stderr: stderr.clone(),
                    suspected_cause: None,
                })
                .await;
        },
        ClaudeFixResult::ClaudeNotAvailable => handle.skipped().await,
        ClaudeFixResult::TimedOut => handle.fail(infra("fix attempt timed out")).await,
        ClaudeFixResult::InvocationError { message } => handle.fail(infra(message)).await,
    }
}

/// Log a Claude fix outcome that did not fix the build.
fn log_unfixed(attr_path: &str, result: &ClaudeFixResult) {
    match result {
        ClaudeFixResult::Fixed => {},
        ClaudeFixResult::StillBroken { stderr } => {
            info!(
                "{}: Claude Code fix did not resolve the build failure, continuing",
                attr_path
            );
            let end = stderr
                .char_indices()
                .nth(500)
                .map_or(stderr.len(), |(i, _)| i);
            debug!("{}: Post-fix build stderr: {}", attr_path, &stderr[..end]);
        },
        ClaudeFixResult::ClaudeNotAvailable => {
            warn!(
                "{}: claude CLI not found on PATH, skipping fix attempt",
                attr_path
            );
        },
        ClaudeFixResult::TimedOut => {
            warn!("{}: Claude Code fix attempt timed out", attr_path);
        },
        ClaudeFixResult::InvocationError { message } => {
            warn!("{}: Claude Code invocation error: {}", attr_path, message);
        },
    }
}

/// Perform a package update in an isolated git worktree.
///
/// Failed attempts are recorded in the database and reported as
/// `Ok(UpdateResult::Failed)`.
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
    claude_fix: Option<&ClaudeFixConfig>,
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

    let attempt = Attempt::new(db, session_id, req);

    // Derive repository directory from eval entry point
    let repo_dir = match get_repo_dir_from_entry_point(eval_entry_point) {
        Ok(dir) => dir,
        Err(e) => {
            warn!(
                "{}: Failed to determine repository directory: {}",
                attr_path, e
            );
            return Ok(UpdateResult::Skipped(format!(
                "Repository directory determination failed: {e}"
            )));
        },
    };

    // Create a worktree for this update (repo-qualified so the same attr
    // path in two watched repos doesn't share a worktree)
    let worktree_key = super::preservation::repo_scoped_name(req.repo(), attr_path);
    let worktree_path = match create_worktree(&repo_dir, &worktree_key).await {
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
            cleanup_worktree(&repo_dir, &worktree_path).await.ok();
            return Ok(attempt
                .fail_setup(UpdatePhase::MetadataExtraction, "meta.position", &e)
                .await);
        },
    };

    debug!("{}: File location: {}", attr_path, file_location);

    // Convert the absolute file path to be relative to the repository root,
    // then join with the worktree path so edits happen inside the worktree.
    let file_location_path = Path::new(&file_location);
    let relative_file_location = file_location_path
        .strip_prefix(&repo_dir)
        .unwrap_or(file_location_path);
    let worktree_file_path = worktree_path.join(relative_file_location);
    let worktree_file_str = worktree_file_path.to_string_lossy().to_string();

    // Attempt the update in the worktree.
    // Use the checker's validated new_version as the explicit target so that
    // update_from_file_path does not re-discover with Latest strategy, which
    // could select a version outside the checker's semver constraints.
    let version_config = crate::commands::update::VersionConfig::new(SemverStrategy::Latest)
        .with_explicit_version(Some(new_version.clone()));
    let update_config = attempt.update_config(run_passthru_tests);

    // Resolve the eval entry point relative to the worktree so that all nix
    // builds/evaluations use the worktree's (modified) files, not the originals.
    let entry_point_filename = Path::new(eval_entry_point)
        .file_name()
        .unwrap_or(std::ffi::OsStr::new("default.nix"));
    let worktree_entry_point = worktree_path.join(entry_point_filename);
    let worktree_entry_str = worktree_entry_point.to_string_lossy().to_string();

    let update_result = update_config
        .update_from_file_path(
            worktree_entry_str.clone(),
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
                            if let Err(e) = cleanup_worktree(&repo_dir, &worktree_path).await {
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
            attempt
                .record_success(rebuild_analysis.as_ref().map(|a| a.rebuild_count))
                .await;

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
                    let pr = attempt
                        .tracker
                        .run(
                            UpdatePhase::PrCreation,
                            create_pr_for_update(
                                db,
                                req.repo(),
                                &worktree_path,
                                &worktree_entry_str,
                                attr_path,
                                current_version,
                                new_version,
                                config,
                                fork,
                                rebuild_analysis.as_ref(),
                                pr_enhancements,
                                &test_result,
                            ),
                        )
                        .await;
                    match pr {
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
            if let Err(e) = cleanup_worktree(&repo_dir, &worktree_path).await {
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
            let failure = attempt.failure(&error_message);

            // Attempt Claude Code fix if enabled and error is plausibly fixable
            if let Some(fix_config) = claude_fix {
                if !super::claude_fix::should_attempt_claude_fix(&error_message) {
                    debug!(
                        "{}: Skipping Claude fix — error is not fixable by editing Nix files",
                        attr_path
                    );
                } else {
                    // Read the Nix file content for context
                    let nix_file_content = tokio::fs::read_to_string(&worktree_file_path)
                        .await
                        .unwrap_or_default();
                    let relative_path = relative_file_location.to_string_lossy().to_string();

                    let handle = attempt.tracker.start(UpdatePhase::AgentFix).await;
                    let fix_result = super::claude_fix::attempt_claude_fix(
                        &worktree_path,
                        &worktree_entry_str,
                        attr_path,
                        &error_message,
                        Some(&error_message),
                        &relative_path,
                        &nix_file_content,
                        current_version,
                        new_version,
                        fix_config,
                    )
                    .await;
                    record_agent_fix(handle, &fix_result).await;

                    if matches!(fix_result, ClaudeFixResult::Fixed) {
                        info!(
                            "{}: Claude Code fixed the build! Proceeding with update.",
                            attr_path
                        );

                        attempt.record_success(None).await;

                        // Create PR if configured
                        if let Some(config) = pr_config {
                            let pr = attempt
                                .tracker
                                .run(
                                    UpdatePhase::PrCreation,
                                    create_pr_for_update(
                                        db,
                                        req.repo(),
                                        &worktree_path,
                                        &worktree_entry_str,
                                        attr_path,
                                        current_version,
                                        new_version,
                                        config,
                                        fork,
                                        None, // No rebuild analysis for Claude-fixed updates
                                        pr_enhancements,
                                        &crate::commands::update::TestResult::NotRequested,
                                    ),
                                )
                                .await;
                            match pr {
                                Ok((pr_url, pr_number)) => {
                                    info!(
                                        "{}: Created PR #{} (Claude-fixed): {}",
                                        attr_path, pr_number, pr_url
                                    );
                                },
                                Err(pr_err) => {
                                    warn!("{}: Failed to create PR: {}", attr_path, pr_err);
                                },
                            }
                        }

                        // Clean up the worktree
                        if let Err(cleanup_err) = cleanup_worktree(&repo_dir, &worktree_path).await
                        {
                            warn!(
                                "{}: Failed to clean up worktree: {}",
                                attr_path, cleanup_err
                            );
                        }

                        return Ok(UpdateResult::Updated {
                            old_version: current_version.clone(),
                            new_version: new_version.clone(),
                        });
                    }
                    log_unfixed(attr_path, &fix_result);
                } // else (should_attempt_claude_fix)
            }

            // Preserve failure artifacts if requested
            let artifacts_path = if preserve_failures {
                let test_output = match &failure {
                    UpdateError::TestError { output, .. } => Some(output.clone()),
                    _ => None,
                };
                match preserve_failure(
                    session_id,
                    req.repo(),
                    attr_path,
                    failure.phase(),
                    &worktree_path,
                    &failure,
                    Some(error_message.clone()), // Build/update error message
                    test_output,
                )
                .await
                {
                    Ok(artifacts) => {
                        info!("{}: Preserved failure artifacts", attr_path);
                        let path = artifacts.package_dir().to_string_lossy().to_string();
                        if let Err(db_err) = db
                            .set_failed_phase_artifacts(session_id, req.repo(), attr_path, &path)
                            .await
                        {
                            warn!(
                                "{}: Failed to link failure artifacts: {}",
                                attr_path, db_err
                            );
                        }
                        Some(path)
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
                if let Err(cleanup_err) = cleanup_worktree(&repo_dir, &worktree_path).await {
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

            Ok(attempt.record_failure(&failure, &error_message).await)
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
        Ok(UpdateResult::Failed(reason)) => {
            info!("{}: Failed - {}", attr_path, reason);
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

/// `git commit` the staged changes of a branch-mode update, in the
/// repository containing `pkg_dir` (matching `branch_git`'s staging).
async fn git_commit(pkg_dir: &str, commit_msg: &str) -> anyhow::Result<()> {
    let output = tokio::process::Command::new("git")
        .args(["-C", pkg_dir, "commit", "-m", commit_msg])
        .output()
        .await
        .context("Failed to run git commit")?;
    if !output.status.success() {
        anyhow::bail!(
            "git commit failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Snapshot the working tree before a branch-mode update.
///
/// A failure here (e.g. `git status` failing while another process holds
/// `index.lock`) is a repository-level setup failure, the branch-mode
/// counterpart of worktree mode's "Worktree creation failed": nothing has
/// been attempted or touched yet and the package is not at fault. So it is
/// reported as [`UpdateResult::Skipped`] (session `skipped` counter, warning
/// logged) without `record_failed_update`: a per-package failure backoff
/// would park the version for 2 days, in watch for every package dispatched
/// while the repository was briefly locked. Leaving the `updates` row as is
/// means watch retries on its next poll and `run` on its next pass, bounded
/// by their own cadence rather than looping. (Previously this was an `Err`,
/// which `run_service` counted as failed without recording anything.)
async fn take_snapshot(
    attr_path: &str,
    pkg_dir: &str,
) -> Result<super::branch_git::DirtySnapshot, UpdateResult> {
    super::branch_git::DirtySnapshot::take(Path::new(pkg_dir))
        .await
        .map_err(|e| {
            warn!("{}: Failed to snapshot working tree: {:#}", attr_path, e);
            UpdateResult::Skipped(format!("Working tree snapshot failed: {e:#}"))
        })
}

/// Perform a package update directly in the working tree (no worktree isolation).
/// Used in commit mode where changes are committed directly to the repo.
/// On failure, reverts every path the update dirtied (see `branch_git`).
pub(super) async fn perform_direct_update(
    db: &Database,
    session_id: &str,
    eval_entry_point: &str,
    req: &UpdateRequest,
    run_passthru_tests: bool,
    git_mutex: Arc<Mutex<()>>,
    claude_fix: Option<&ClaudeFixConfig>,
) -> anyhow::Result<UpdateResult> {
    let attr_path = &req.attr_path;
    let current_version = &req.current_version;
    let new_version = &req.new_version;
    let attempt = Attempt::new(db, session_id, req);

    // Get file location from meta.position
    let file_location = match get_file_location(eval_entry_point, attr_path).await {
        Ok(loc) => loc,
        Err(e) => {
            warn!("{}: Failed to get file location: {}", attr_path, e);
            return Ok(attempt
                .fail_setup(UpdatePhase::MetadataExtraction, "meta.position", &e)
                .await);
        },
    };

    debug!("{}: File location: {}", attr_path, file_location);

    // Determine the package directory for git operations
    let pkg_dir = std::path::Path::new(&file_location)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| file_location.clone());

    // Snapshot dirty paths so that edits outside pkg_dir (e.g. new dependency
    // packages created by a Claude fix) are committed or reverted with this update.
    let snapshot = {
        let _lock = git_mutex.lock().await;
        match take_snapshot(attr_path, &pkg_dir).await {
            Ok(snapshot) => snapshot,
            Err(skipped) => return Ok(skipped),
        }
    };

    // Perform the update directly in the working tree (commit handled separately under mutex).
    // Use the checker's validated new_version as the explicit target so that
    // update_from_file_path does not re-discover with Latest strategy, which
    // could select a version outside the checker's semver constraints.
    let version_config = crate::commands::update::VersionConfig::new(SemverStrategy::Latest)
        .with_explicit_version(Some(new_version.clone()));
    let update_config = attempt.update_config(run_passthru_tests);

    let update_result = update_config
        .update_from_file_path(
            eval_entry_point.to_owned(),
            attr_path.clone(),
            file_location.clone(),
            version_config,
        )
        .await;

    let commit_msg = format!("{attr_path}: {current_version} -> {new_version}");

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
                    snapshot.revert(attr_path, &pkg_dir).await;
                }

                // Record as a test failure (distinct from build failure)
                let error_message =
                    format!("passthru.tests failed after update to {new_version}:\n{stderr}");
                let failure = attempt.failure(&error_message);
                return Ok(attempt.record_failure(&failure, &error_message).await);
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

                snapshot.stage(attr_path, &pkg_dir).await;

                match attempt
                    .tracker
                    .run(UpdatePhase::PrCreation, git_commit(&pkg_dir, &commit_msg))
                    .await
                {
                    Ok(()) => info!("{}: Committed: {}", attr_path, commit_msg),
                    Err(e) => warn!("{}: {:#}", attr_path, e),
                }
            }

            attempt.record_success(None).await;

            Ok(UpdateResult::Updated {
                old_version: current_version.clone(),
                new_version: new_version.clone(),
            })
        },
        Err(e) => {
            let error_message = format!("{e:#}");
            warn!("{}: Update failed: {}", attr_path, error_message);
            let failure = attempt.failure(&error_message);

            // Attempt Claude Code fix if enabled (files are still dirty)
            if let Some(fix_config) = claude_fix {
                if !super::claude_fix::should_attempt_claude_fix(&error_message) {
                    debug!(
                        "{}: Skipping Claude fix — error is not fixable by editing Nix files",
                        attr_path
                    );
                } else {
                    let nix_file_content = tokio::fs::read_to_string(&file_location)
                        .await
                        .unwrap_or_default();

                    // In branch mode, use the repo working directory as the "worktree"
                    let repo_dir = std::path::Path::new(&file_location)
                        .parent()
                        .unwrap_or(std::path::Path::new("."));

                    let handle = attempt.tracker.start(UpdatePhase::AgentFix).await;
                    let fix_result = super::claude_fix::attempt_claude_fix(
                        repo_dir,
                        eval_entry_point,
                        attr_path,
                        &error_message,
                        Some(&error_message),
                        &file_location,
                        &nix_file_content,
                        current_version,
                        new_version,
                        fix_config,
                    )
                    .await;
                    record_agent_fix(handle, &fix_result).await;

                    if matches!(fix_result, ClaudeFixResult::Fixed) {
                        info!("{}: Claude Code fixed the build! Committing.", attr_path);

                        // Commit the fix under the git mutex
                        {
                            let _lock = git_mutex.lock().await;

                            snapshot.stage(attr_path, &pkg_dir).await;

                            match attempt
                                .tracker
                                .run(UpdatePhase::PrCreation, git_commit(&pkg_dir, &commit_msg))
                                .await
                            {
                                Ok(()) => {
                                    info!(
                                        "{}: Committed (Claude-fixed): {}",
                                        attr_path, commit_msg
                                    );
                                },
                                Err(commit_err) => warn!("{}: {:#}", attr_path, commit_err),
                            }
                        }

                        attempt.record_success(None).await;

                        return Ok(UpdateResult::Updated {
                            old_version: current_version.clone(),
                            new_version: new_version.clone(),
                        });
                    }

                    log_unfixed(attr_path, &fix_result);
                } // else (should_attempt_claude_fix)
            }

            // Revert everything this update touched under the mutex
            {
                let _lock = git_mutex.lock().await;
                info!("{}: Reverting {}...", attr_path, pkg_dir);
                snapshot.revert(attr_path, &pkg_dir).await;
            }

            Ok(attempt.record_failure(&failure, &error_message).await)
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
///
/// `eval_entry_point` is the worktree's entry point (the updated tree).
async fn create_pr_for_update(
    db: &Database,
    repo_name: &str,
    worktree_path: &Path,
    eval_entry_point: &str,
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
    let metadata = PackageMetadata::from_attr_path(eval_entry_point, attr_path)
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
    if let Some(analysis) = cve_analysis.as_ref() {
        if let Err(e) = db
            .record_cve_counts(
                repo_name,
                attr_path,
                analysis.resolved.len(),
                analysis.introduced.len(),
            )
            .await
        {
            warn!("{}: Failed to record CVE counts: {}", attr_path, e);
        }
    }

    // Push build outputs to Cachix (if enabled) before the directory diff
    // step, since the diff pass runs `git checkout HEAD~1` and would
    // invalidate the worktree state we want to push from. Failures are
    // swallowed: Cachix is a non-blocking enhancement, and the result is
    // intentionally not surfaced in the PR body — reviewers of nixpkgs-style
    // updates don't typically need the raw store paths or `cachix use`
    // boilerplate, and the cache name is already visible in the deployment.
    if let Err(e) = pr_enhancements
        .perform_worktree_cachix_push(worktree_path, eval_entry_point, attr_path)
        .await
    {
        warn!("{}: Cachix push raised an error: {:#}", attr_path, e);
    }

    // Audit the built package if requested
    let audit_markdown = match pr_enhancements
        .perform_worktree_audit(worktree_path, eval_entry_point, attr_path)
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
        .perform_worktree_directory_diff(worktree_path, eval_entry_point, attr_path)
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
    db.record_pr_info(repo_name, attr_path, &pr.html_url, pr.number)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A snapshot failure is a setup failure reported as `Skipped`, not an
    /// `Err` that `run_service` would count as failed without recording it.
    #[tokio::test]
    async fn test_snapshot_failure_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let result = take_snapshot("hello", &dir.path().to_string_lossy()).await;
        match result {
            Err(UpdateResult::Skipped(reason)) => {
                assert!(
                    reason.starts_with("Working tree snapshot failed"),
                    "{reason}"
                );
            },
            other => panic!("expected Skipped, got {other:?}"),
        }
    }
}
