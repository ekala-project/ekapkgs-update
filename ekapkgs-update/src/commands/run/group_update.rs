//! Update groups: every member of a [`GroupRequest`] is updated, one commit
//! per member in dependency order, then the whole group (every member and the
//! root) is built and tested.
//!
//! - Worktree mode ([`perform_group_update`]): in one worktree, proposed as
//!   one PR on a stable branch (`update/group/<root>`).
//! - Branch mode ([`perform_branch_group_update`]): committed on the current
//!   branch, no PR.
//!
//! Both share the member loop, verification and failure attribution
//! ([`GroupWorkspace`] says where members are edited and how they are
//! committed). Groups are all-or-nothing: when a member's update or the
//! verification fails, nothing is proposed (branch mode also drops the
//! group's commits). The member that failed records its own error; the
//! others record "group <root> failed" and back off.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use tokio::process::Command;
use tracing::{debug, info, warn};

use tokio::sync::Mutex;

use super::branch_git::GroupCheckpoint;
use super::claude_fix::{ClaudeFixConfig, ClaudeFixResult};
use super::preservation::preserve_failure;
use super::types::{GroupRequest, UpdateRequest, UpdateResult};
use super::updater::{
    Attempt, SetupFailure, entry_point_in_repo, get_file_location, get_repo_dir_from_entry_point,
    log_unfixed, record_agent_fix, relative_to_repo,
};
use crate::commands::pr_enhancements::PrEnhancementsConfig;
use crate::commands::update::errors::UpdateError;
use crate::commands::update::instrumentation::PhaseTracker;
use crate::commands::update::types::UpdatePhase;
use crate::commands::update::{AlreadyAtVersion, ComponentUpdate, TestResult, VersionConfig};
use crate::database::Database;
use crate::git::{PrConfig, cleanup_worktree, create_worktree};
use crate::nix::normalize_entry_point;
use crate::package::PackageMetadata;
use crate::vcs_sources::SemverStrategy;

/// One finished member: its result and attr path (as reported by the
/// updater service).
pub(super) type MemberOutput = (anyhow::Result<UpdateResult>, String);

/// Everything a group update needs besides the group itself.
pub(super) struct GroupUpdateOptions<'a> {
    pub db: &'a Database,
    pub session_id: &'a str,
    pub eval_entry_point: &'a str,
    pub pr_config: Option<&'a PrConfig>,
    pub fork: &'a str,
    pub run_passthru_tests: bool,
    pub dry_run: bool,
    pub pr_enhancements: &'a PrEnhancementsConfig,
    pub preserve_failures: bool,
    pub claude_fix: Option<&'a ClaudeFixConfig>,
}

/// A member updated (and committed) in the group worktree.
struct Updated {
    index: usize,
    removed_patches: Vec<String>,
}

/// Why the group failed.
struct GroupFailure {
    /// Attr path that failed (a member, or a verified attr such as the root).
    attr_path: String,
    error: UpdateError,
    error_log: String,
}

impl GroupFailure {
    fn of(attr_path: &str, error: UpdateError, error_log: String) -> Box<Self> {
        Box::new(Self {
            attr_path: attr_path.to_owned(),
            error,
            error_log,
        })
    }
}

/// Update every member of `group` in one worktree and propose it as one PR.
///
/// Returns one result per member, in the order of `group.members`.
pub(super) async fn perform_group_update(
    opts: &GroupUpdateOptions<'_>,
    group: &GroupRequest,
) -> Vec<MemberOutput> {
    if opts.dry_run {
        return dry_run_outputs(group);
    }

    let attempts: Vec<Attempt<'_>> = group
        .members
        .iter()
        .map(|req| Attempt::new(opts.db, opts.session_id, req))
        .collect();
    let mut results: Vec<Option<UpdateResult>> = group.members.iter().map(|_| None).collect();

    // Worktree setup (as in `perform_update`).
    let repo_dir = match get_repo_dir_from_entry_point(opts.eval_entry_point).await {
        Ok(dir) => dir,
        Err(e) => {
            let failure =
                SetupFailure::Transient(format!("Repository directory determination failed: {e}"));
            return setup_failed(&attempts, &failure).await;
        },
    };
    let relative_entry_point = match entry_point_in_repo(opts.eval_entry_point, &repo_dir) {
        Ok(relative) => relative,
        Err(failure) => return setup_failed(&attempts, &failure).await,
    };
    let worktree_key = super::preservation::repo_scoped_name(
        group.lead().repo(),
        &format!("group-{}", group.root),
    );
    let worktree_path = match create_worktree(&repo_dir, &worktree_key).await {
        Ok(path) => std::fs::canonicalize(&path).unwrap_or(path),
        Err(e) => {
            let failure = SetupFailure::Transient(format!("Worktree creation failed: {e}"));
            return setup_failed(&attempts, &failure).await;
        },
    };
    let worktree_entry = worktree_path.join(&relative_entry_point);
    let worktree_entry_str = worktree_entry.to_string_lossy().to_string();

    let workspace = GroupWorkspace {
        path: &worktree_path,
        entry: &worktree_entry_str,
        committer: Committer::Worktree,
    };
    let GroupRun { updated, outcome } =
        run_group(&NixSteps, opts, group, &attempts, &mut results, &workspace).await;

    // Verified as a whole: every member and the root.
    let (test_result, fixed_by_claude) = match outcome {
        Ok(None) => {
            cleanup(&repo_dir, &worktree_path, &group.root).await;
            return collect(group, results);
        },
        Ok(Some(verified)) => (verified.tests, verified.fixed_by_claude),
        Err(failure) => {
            let preserved = preserve(
                opts,
                group,
                &failure,
                &repo_dir,
                &worktree_path,
                &worktree_entry,
            )
            .await;
            record_group_failure(group, &attempts, &mut results, &failure).await;
            if !preserved {
                cleanup(&repo_dir, &worktree_path, &group.root).await;
            }
            return collect(group, results);
        },
    };

    log_removed_patches(group, &updated);
    // Rebuild impact of the whole group.
    let rebuild_analysis = if opts.pr_enhancements.analyze_rebuilds {
        match crate::nix::rebuild_count::calculate_rebuild_count(
            opts.eval_entry_point,
            &worktree_path,
        )
        .await
        {
            Ok(analysis) => {
                info!("group {}: {}", group.root, analysis.summary());
                if opts
                    .pr_enhancements
                    .should_skip_for_rebuilds(analysis.rebuild_count)
                {
                    let threshold = opts.pr_enhancements.max_rebuilds.unwrap_or_default();
                    info!(
                        "group {}: skipping - rebuild count {} exceeds threshold {}",
                        group.root, analysis.rebuild_count, threshold
                    );
                    cleanup(&repo_dir, &worktree_path, &group.root).await;
                    let reason = format!(
                        "Rebuild count {} exceeds threshold {}",
                        analysis.rebuild_count, threshold
                    );
                    for u in &updated {
                        results[u.index] = Some(UpdateResult::Skipped(reason.clone()));
                    }
                    return collect(group, results);
                }
                Some(analysis)
            },
            Err(e) => {
                warn!(
                    "group {}: failed to calculate rebuild count: {e:#}",
                    group.root
                );
                None
            },
        }
    } else {
        None
    };
    let rebuild_count = rebuild_analysis.as_ref().map(|a| a.rebuild_count);

    let updated_members: Vec<&UpdateRequest> =
        updated.iter().map(|u| &group.members[u.index]).collect();

    if let Some(config) = opts.pr_config {
        let pr_tracker = PhaseTracker::new(
            opts.db.clone(),
            opts.session_id,
            group.lead().repo(),
            &group.root,
        );
        let pr = pr_tracker
            .run(
                UpdatePhase::PrCreation,
                create_group_pr(
                    opts,
                    group,
                    &updated_members,
                    &worktree_path,
                    &worktree_entry_str,
                    config,
                    rebuild_analysis.as_ref(),
                    &test_result,
                    fixed_by_claude,
                ),
            )
            .await;
        match &pr {
            Ok((url, number)) => info!(
                "group {}: {} PR #{}: {}",
                group.root,
                if test_result.failed() {
                    "proposed draft"
                } else {
                    "proposed"
                },
                number,
                url
            ),
            Err(e) => warn!("group {}: failed to propose PR: {e:#}", group.root),
        }
        for u in &updated {
            let attempt = &attempts[u.index];
            let outcome = match &pr {
                Ok(ok) => Ok(ok.clone()),
                Err(e) => Err(anyhow::anyhow!("{e:#}")),
            };
            results[u.index] = Some(
                match attempt.record_pr_outcome(rebuild_count, outcome).await {
                    Ok(_) => updated_result(attempt.req),
                    Err(failed) => failed,
                },
            );
        }
    } else {
        // Nothing is proposed without a PR (see `perform_update`): the
        // members stay `outdated`.
        for u in &updated {
            let req = attempts[u.index].req;
            results[u.index] = Some(super::updater::not_proposed_without_pr_config(
                &req.attr_path,
                &req.new_version,
            ));
        }
    }

    cleanup(&repo_dir, &worktree_path, &group.root).await;
    collect(group, results)
}

/// Where a group's members are updated and committed.
pub(super) struct GroupWorkspace<'a> {
    /// Repository root the members' files are resolved against: the group
    /// worktree, or the checkout itself in branch mode.
    pub path: &'a Path,
    /// Entry point inside `path`.
    pub entry: &'a str,
    pub committer: Committer<'a>,
}

/// How a group's commits are made.
pub(super) enum Committer<'a> {
    /// Worktree mode: commit everything in the group worktree.
    Worktree,
    /// Branch mode: commit only what the group dirtied (see
    /// [`GroupCheckpoint`]), under the git mutex.
    Branch {
        checkpoint: &'a GroupCheckpoint,
        git_mutex: &'a Mutex<()>,
    },
}

impl GroupWorkspace<'_> {
    /// Commit the changes of `label` (a member, or the root for a fix) as
    /// `title`. `pkg_dir` is its package directory, when known. Returns
    /// `false` when nothing changed.
    async fn commit(
        &self,
        label: &str,
        pkg_dir: Option<&Path>,
        title: &str,
    ) -> anyhow::Result<bool> {
        match &self.committer {
            Committer::Worktree => crate::git::commit_all(self.path, title).await,
            Committer::Branch {
                checkpoint,
                git_mutex,
            } => {
                let _lock = git_mutex.lock().await;
                let pkg_dir = pkg_dir.map(|d| d.to_string_lossy().into_owned());
                checkpoint.commit(label, pkg_dir.as_deref(), title).await
            },
        }
    }
}

/// Members updated (in dependency order) and, unless none needed an update
/// or one failed, the verification of the whole group.
struct GroupRun {
    updated: Vec<Updated>,
    /// `Ok(None)`: no member needed an update (nothing was verified).
    outcome: Result<Option<Verified>, Box<GroupFailure>>,
}

/// Update every member in `workspace`, one commit each, dependencies first,
/// then verify the group. Members already at their target version get their
/// result in `results`; the rest are left to the caller.
async fn run_group(
    steps: &impl GroupSteps,
    opts: &GroupUpdateOptions<'_>,
    group: &GroupRequest,
    attempts: &[Attempt<'_>],
    results: &mut [Option<UpdateResult>],
    workspace: &GroupWorkspace<'_>,
) -> GroupRun {
    let attrs: Vec<String> = group.members.iter().map(|m| m.attr_path.clone()).collect();
    let order = steps.commit_order(workspace.entry, &attrs).await;
    info!(
        "group {}: updating {} member(s) in order: {}",
        group.root,
        order.len(),
        order.join(", ")
    );

    let mut updated: Vec<Updated> = Vec::new();
    for attr in &order {
        let index = attrs.iter().position(|a| a == attr).expect("member");
        let attempt = &attempts[index];
        match update_member(steps, opts, attempt, workspace).await {
            Ok(MemberEdit::Updated {
                removed_patches, ..
            }) => updated.push(Updated {
                index,
                removed_patches,
            }),
            Ok(MemberEdit::AlreadyAtVersion(already)) => {
                results[index] = Some(attempt.already_at_version(&already).await);
            },
            Err(failure) => {
                return GroupRun {
                    updated,
                    outcome: Err(failure),
                };
            },
        }
    }
    if updated.is_empty() {
        info!("group {}: no member needed an update", group.root);
        return GroupRun {
            updated,
            outcome: Ok(None),
        };
    }

    let root_tracker = PhaseTracker::new(
        opts.db.clone(),
        opts.session_id,
        group.lead().repo(),
        &group.root,
    );
    let outcome = steps
        .verify(opts, group, &root_tracker, workspace)
        .await
        .map(Some);
    GroupRun { updated, outcome }
}

/// What a member records when its group failed: the member that failed its
/// own error, every other member `group <root> failed: <attr> (<phase>)`.
fn member_failure(root: &str, failure: &GroupFailure, member: &str) -> (UpdateError, String) {
    if member == failure.attr_path {
        return (failure.error.clone(), failure.error_log.clone());
    }
    let details = format!(
        "group {root} failed: {} ({}): {}",
        failure.attr_path,
        failure.error.phase(),
        failure.error.summary()
    );
    let error = UpdateError::InfrastructureError {
        phase: failure.error.phase(),
        component: "group".to_owned(),
        details: details.clone(),
    };
    (error, format!("{details}\n\n{}", failure.error_log))
}

/// Record a failed group for every member without a result yet (failure
/// log, `failed`, failure backoff).
async fn record_group_failure(
    group: &GroupRequest,
    attempts: &[Attempt<'_>],
    results: &mut [Option<UpdateResult>],
    failure: &GroupFailure,
) {
    warn!(
        "group {}: {} failed ({}): {}",
        group.root,
        failure.attr_path,
        failure.error.phase(),
        failure.error.summary()
    );
    for (attempt, result) in attempts.iter().zip(results.iter_mut()) {
        if result.is_some() {
            continue;
        }
        let (error, log) = member_failure(&group.root, failure, &attempt.req.attr_path);
        *result = Some(attempt.record_failure(&error, &log).await);
    }
}

fn log_removed_patches(group: &GroupRequest, updated: &[Updated]) {
    for u in updated {
        if !u.removed_patches.is_empty() {
            info!(
                "{}: Removed {} obsolete patch(es): {}",
                group.members[u.index].attr_path,
                u.removed_patches.len(),
                u.removed_patches.join(", ")
            );
        }
    }
}

/// Update every member of `group` on the current branch (branch mode): one
/// commit per member in dependency order, then the group is verified. No PR
/// is created; the members are recorded like branch-mode single updates.
///
/// All-or-nothing: if a member or the verification (build or
/// `passthru.tests`) fails, the group's commits are dropped and every path
/// it dirtied is reverted ([`GroupCheckpoint::undo`]). Must only run while no
/// other branch-mode update does (`updater_concurrency`).
///
/// Returns one result per member, in the order of `group.members`.
pub(super) async fn perform_branch_group_update(
    opts: &GroupUpdateOptions<'_>,
    group: &GroupRequest,
    git_mutex: &Mutex<()>,
) -> Vec<MemberOutput> {
    branch_group_update(&NixSteps, opts, group, git_mutex).await
}

/// [`perform_branch_group_update`] with its nix steps passed in.
async fn branch_group_update(
    steps: &impl GroupSteps,
    opts: &GroupUpdateOptions<'_>,
    group: &GroupRequest,
    git_mutex: &Mutex<()>,
) -> Vec<MemberOutput> {
    if opts.dry_run {
        return dry_run_outputs(group);
    }
    let attempts: Vec<Attempt<'_>> = group
        .members
        .iter()
        .map(|req| Attempt::new(opts.db, opts.session_id, req))
        .collect();
    let mut results: Vec<Option<UpdateResult>> = group.members.iter().map(|_| None).collect();

    let repo_dir = match get_repo_dir_from_entry_point(opts.eval_entry_point).await {
        Ok(dir) => dir,
        Err(e) => {
            let failure =
                SetupFailure::Transient(format!("Repository directory determination failed: {e}"));
            return setup_failed(&attempts, &failure).await;
        },
    };
    // Like a branch-mode single's snapshot: a repository-level failure.
    let checkpoint = {
        let _lock = git_mutex.lock().await;
        GroupCheckpoint::take(&repo_dir).await
    };
    let checkpoint = match checkpoint {
        Ok(checkpoint) => checkpoint,
        Err(e) => {
            let failure = SetupFailure::Transient(format!("Working tree snapshot failed: {e:#}"));
            return setup_failed(&attempts, &failure).await;
        },
    };

    let workspace = GroupWorkspace {
        path: &repo_dir,
        entry: opts.eval_entry_point,
        committer: Committer::Branch {
            checkpoint: &checkpoint,
            git_mutex,
        },
    };
    let GroupRun { updated, outcome } =
        run_group(steps, opts, group, &attempts, &mut results, &workspace).await;

    let failure = match outcome {
        Ok(None) => return collect(group, results),
        Ok(Some(verified)) => verified.test_failure(),
        Err(failure) => Some(failure),
    };
    if let Some(failure) = failure {
        let undone = {
            let _lock = git_mutex.lock().await;
            checkpoint.undo(&group.root).await
        };
        if let Err(e) = undone {
            tracing::error!(
                "group {}: could not undo the group's commits ({}) and changes: {e:#}. The \
                 branch and working tree are left as they are and need manual cleanup.",
                group.root,
                checkpoint.created().join(", ")
            );
        }
        record_group_failure(group, &attempts, &mut results, &failure).await;
        return collect(group, results);
    }

    log_removed_patches(group, &updated);
    for u in &updated {
        let attempt = &attempts[u.index];
        attempt.record_success(None).await;
        results[u.index] = Some(updated_result(attempt.req));
    }
    info!(
        "group {}: committed {} member(s) on the current branch",
        group.root,
        updated.len()
    );
    collect(group, results)
}

/// Every member reported as a dry run (nothing is edited or committed).
fn dry_run_outputs(group: &GroupRequest) -> Vec<MemberOutput> {
    group
        .members
        .iter()
        .map(|req| (Ok(UpdateResult::dry_run(req)), req.attr_path.clone()))
        .collect()
}

/// A group that could not be set up: every member reports the failure
/// (lasting ones are recorded and back off, transient ones are skipped).
async fn setup_failed(attempts: &[Attempt<'_>], failure: &SetupFailure) -> Vec<MemberOutput> {
    let mut outputs = Vec::with_capacity(attempts.len());
    for attempt in attempts {
        let result = attempt.setup_failed(failure).await;
        outputs.push((Ok(result), attempt.req.attr_path.clone()));
    }
    outputs
}

fn updated_result(req: &UpdateRequest) -> UpdateResult {
    UpdateResult::Updated {
        old_version: req.current_version.clone(),
        new_version: req.new_version.clone(),
    }
}

/// Pair results with attr paths; a member without a result was not reached.
fn collect(group: &GroupRequest, results: Vec<Option<UpdateResult>>) -> Vec<MemberOutput> {
    group
        .members
        .iter()
        .zip(results)
        .map(|(req, result)| {
            let result = result.unwrap_or_else(|| {
                UpdateResult::Skipped(format!("group {} was not completed", group.root))
            });
            (Ok(result), req.attr_path.clone())
        })
        .collect()
}

async fn cleanup(repo_dir: &Path, worktree_path: &Path, root: &str) {
    if let Err(e) = cleanup_worktree(repo_dir, worktree_path).await {
        warn!("group {}: failed to clean up worktree: {e:#}", root);
    }
}

/// Preserve the failed group worktree under the member that failed (or the
/// first member when a non-member, such as the root, failed verification).
/// Returns whether the worktree was preserved: moved into the artifacts
/// (nothing left to clean up), or copied with the original kept because the
/// copy's git metadata still points at it.
async fn preserve(
    opts: &GroupUpdateOptions<'_>,
    group: &GroupRequest,
    failure: &GroupFailure,
    repo_dir: &Path,
    worktree_path: &Path,
    worktree_entry: &Path,
) -> bool {
    if !opts.preserve_failures {
        return false;
    }
    let attr = group
        .members
        .iter()
        .find(|m| m.attr_path == failure.attr_path)
        .unwrap_or_else(|| group.lead());
    let test_output = match &failure.error {
        UpdateError::TestError { output, .. } => Some(output.clone()),
        _ => None,
    };
    match preserve_failure(
        opts.session_id,
        attr.repo(),
        &attr.attr_path,
        failure.error.phase(),
        repo_dir,
        worktree_path,
        worktree_entry,
        &failure.error,
        Some(failure.error_log.clone()),
        test_output,
    )
    .await
    {
        Ok(artifacts) => {
            let path = artifacts.package_dir().to_string_lossy().to_string();
            if let Err(e) = opts
                .db
                .set_failed_phase_artifacts(opts.session_id, attr.repo(), &attr.attr_path, &path)
                .await
            {
                warn!(
                    "{}: Failed to link failure artifacts: {}",
                    attr.attr_path, e
                );
            }
            true
        },
        Err(e) => {
            warn!(
                "{}: Failed to preserve failure artifacts: {e:#}",
                attr.attr_path
            );
            false
        },
    }
}

/// A member's update in the workspace, before it is committed.
enum MemberEdit {
    Updated {
        removed_patches: Vec<String>,
        components: Vec<ComponentUpdate>,
        /// The member's package directory (staged with its commit).
        pkg_dir: Option<PathBuf>,
    },
    AlreadyAtVersion(AlreadyAtVersion),
}

/// The steps of a group update that need nix: the commit order, a member's
/// update and the group's verification. [`NixSteps`] runs them; tests of the
/// orchestration around them (commits, undo, recording) substitute their own.
trait GroupSteps {
    /// The order members are updated and committed in.
    async fn commit_order(&self, entry: &str, members: &[String]) -> Vec<String>;

    /// Update one member in the workspace (not committed yet).
    async fn edit_member(
        &self,
        opts: &GroupUpdateOptions<'_>,
        attempt: &Attempt<'_>,
        workspace: &GroupWorkspace<'_>,
    ) -> Result<MemberEdit, Box<GroupFailure>>;

    /// Build (and test) the whole group once every member is committed.
    async fn verify(
        &self,
        opts: &GroupUpdateOptions<'_>,
        group: &GroupRequest,
        tracker: &PhaseTracker,
        workspace: &GroupWorkspace<'_>,
    ) -> Result<Verified, Box<GroupFailure>>;
}

/// The real steps: nix evaluation, the update workflow and `nix-build`.
struct NixSteps;

impl GroupSteps for NixSteps {
    async fn commit_order(&self, entry: &str, members: &[String]) -> Vec<String> {
        commit_order(entry, members).await
    }

    /// Run the update workflow for one member in the workspace. A Claude
    /// fix of the member's own build failure is part of its edit.
    async fn edit_member(
        &self,
        opts: &GroupUpdateOptions<'_>,
        attempt: &Attempt<'_>,
        workspace: &GroupWorkspace<'_>,
    ) -> Result<MemberEdit, Box<GroupFailure>> {
        let (worktree_path, worktree_entry) = (workspace.path, workspace.entry);
        let req = attempt.req;
        let attr_path = &req.attr_path;
        let fail = |error, error_log| GroupFailure::of(attr_path, error, error_log);
        let infra = |phase: UpdatePhase, component: &str, details: String| {
            UpdateError::InfrastructureError {
                phase,
                component: component.to_owned(),
                details,
            }
        };

        // Located in the worktree: earlier members' edits are already there.
        let file_location = match get_file_location(worktree_entry, attr_path).await {
            Ok(loc) => loc,
            Err(e) => {
                let error = infra(
                    UpdatePhase::MetadataExtraction,
                    "meta.position",
                    format!("{e:#}"),
                );
                attempt.tracker.record_failure(error.clone()).await;
                return Err(fail(error, format!("{e:#}")));
            },
        };
        let Some(relative_file) = relative_to_repo(Path::new(&file_location), worktree_path) else {
            let details = format!("package file {file_location} is outside the repository");
            return Err(fail(
                infra(
                    UpdatePhase::MetadataExtraction,
                    "meta.position",
                    details.clone(),
                ),
                details,
            ));
        };
        let file_path = worktree_path.join(&relative_file);

        let version_config = VersionConfig::new(SemverStrategy::Latest)
            .with_explicit_version(Some(req.new_version.clone()));
        // passthru.tests run once for the whole group, after every member.
        let result = attempt
            .update_config(false)
            .update_from_file_path(
                worktree_entry.to_owned(),
                attr_path.clone(),
                file_path.to_string_lossy().to_string(),
                version_config,
            )
            .await;

        let (removed_patches, components) = match result {
            Ok(outcome) => (outcome.removed_patches, outcome.components),
            Err(e) => {
                if let Some(already) = e.downcast_ref::<AlreadyAtVersion>() {
                    return Ok(MemberEdit::AlreadyAtVersion(already.clone()));
                }
                let message = format!("{e:#}");
                warn!("{}: Update failed: {}", attr_path, message);
                let error = attempt.failure(&message);
                let fixed = match opts.claude_fix {
                    Some(cfg) if super::claude_fix::should_attempt_claude_fix(&message) => {
                        let content = tokio::fs::read_to_string(&file_path)
                            .await
                            .unwrap_or_default();
                        let handle = attempt.tracker.start(UpdatePhase::AgentFix).await;
                        let fix = super::claude_fix::attempt_claude_fix(
                            worktree_path,
                            worktree_entry,
                            attr_path,
                            &message,
                            Some(&message),
                            &relative_file.to_string_lossy(),
                            &content,
                            &req.current_version,
                            &req.new_version,
                            cfg,
                        )
                        .await;
                        record_agent_fix(handle, &fix).await;
                        log_unfixed(attr_path, &fix);
                        matches!(fix, ClaudeFixResult::Fixed)
                    },
                    _ => false,
                };
                if !fixed {
                    return Err(fail(error, message));
                }
                info!("{}: Claude Code fixed the build", attr_path);
                (Vec::new(), Vec::new())
            },
        };

        Ok(MemberEdit::Updated {
            removed_patches,
            components,
            pkg_dir: file_path.parent().map(Path::to_path_buf),
        })
    }

    async fn verify(
        &self,
        opts: &GroupUpdateOptions<'_>,
        group: &GroupRequest,
        tracker: &PhaseTracker,
        workspace: &GroupWorkspace<'_>,
    ) -> Result<Verified, Box<GroupFailure>> {
        verify_group(opts, group, tracker, workspace).await
    }
}

/// Update one member and commit it on its own.
async fn update_member(
    steps: &impl GroupSteps,
    opts: &GroupUpdateOptions<'_>,
    attempt: &Attempt<'_>,
    workspace: &GroupWorkspace<'_>,
) -> Result<MemberEdit, Box<GroupFailure>> {
    let edit = steps.edit_member(opts, attempt, workspace).await?;
    let MemberEdit::Updated {
        components,
        pkg_dir,
        ..
    } = &edit
    else {
        return Ok(edit);
    };
    let req = attempt.req;
    let attr_path = &req.attr_path;
    let title = crate::git::update_title(attr_path, &req.current_version, &req.new_version);
    let message = format!(
        "{title}{}",
        crate::commands::update::components_commit_lines(components)
    );
    match workspace
        .commit(attr_path, pkg_dir.as_deref(), &message)
        .await
    {
        Ok(true) => {},
        Ok(false) => debug!("{}: update changed no files", attr_path),
        Err(e) => {
            let error = UpdateError::GitError {
                phase: UpdatePhase::PrCreation,
                operation: "commit".to_owned(),
                details: format!("{e:#}"),
            };
            attempt.tracker.record_failure(error.clone()).await;
            return Err(GroupFailure::of(attr_path, error, format!("{e:#}")));
        },
    }
    Ok(edit)
}

/// Outcome of a successful group verification.
struct Verified {
    tests: TestResult,
    fixed_by_claude: bool,
    /// First verified attr whose passthru.tests failed.
    failed_test: Option<String>,
}

impl Verified {
    /// Failed passthru.tests as a group failure (branch mode, which has no
    /// draft PR to report them in, fails the group like a single update).
    fn test_failure(self) -> Option<Box<GroupFailure>> {
        let TestResult::Failed(output) = self.tests else {
            return None;
        };
        let attr_path = self.failed_test.unwrap_or_default();
        Some(Box::new(GroupFailure {
            error_log: format!("passthru.tests failed with the updated group:\n{output}"),
            error: UpdateError::TestError {
                phase: UpdatePhase::Testing,
                test_name: None,
                output,
                exit_code: 1,
            },
            attr_path,
        }))
    }
}

/// Build every attr in `group.verify` (and its passthru.tests when enabled).
///
/// A root that does not evaluate (a label, not a package) is skipped. On a
/// build failure, one Claude fix attempt is allowed for the whole group; its
/// changes are committed as `<root>: fix build with updated dependencies`
/// and the verification restarts.
async fn verify_group(
    opts: &GroupUpdateOptions<'_>,
    group: &GroupRequest,
    tracker: &PhaseTracker,
    workspace: &GroupWorkspace<'_>,
) -> Result<Verified, Box<GroupFailure>> {
    let worktree_entry = workspace.entry;
    let members: HashSet<&str> = group.members.iter().map(|m| m.attr_path.as_str()).collect();
    let mut targets: Vec<&str> = Vec::new();
    for attr in &group.verify {
        if targets.contains(&attr.as_str()) {
            continue;
        }
        if !members.contains(attr.as_str()) && instantiate(worktree_entry, attr).await.is_err() {
            debug!(
                "group {}: {} does not evaluate; not verified",
                group.root, attr
            );
            continue;
        }
        targets.push(attr);
    }

    let mut fix_attempted = false;
    let mut fixed_by_claude = false;
    'verify: loop {
        let handle = tracker.start(UpdatePhase::Build).await;
        for attr in &targets {
            info!("group {}: verifying {}", group.root, attr);
            let (ok, _stdout, stderr) =
                match crate::commands::update::build_nix_expr(worktree_entry, attr, None).await {
                    Ok(out) => out,
                    Err(e) => (false, String::new(), format!("{e:#}")),
                };
            if ok {
                continue;
            }
            let error = UpdateError::BuildError {
                phase: UpdatePhase::Build,
                exit_code: 1,
                stderr: stderr.clone(),
                suspected_cause: None,
            };
            handle.fail(error.clone()).await;

            if let Some(cfg) = opts.claude_fix {
                if !fix_attempted && super::claude_fix::should_attempt_claude_fix(&stderr) {
                    fix_attempted = true;
                    if claude_fix_group(cfg, group, tracker, workspace, attr, &stderr).await {
                        fixed_by_claude = true;
                        continue 'verify;
                    }
                }
            }
            return Err(Box::new(GroupFailure {
                attr_path: (*attr).to_owned(),
                error,
                error_log: format!("{attr} failed to build with the updated group:\n\n{stderr}"),
            }));
        }
        handle.success().await;
        break;
    }

    // passthru.tests of every verified attr.
    let mut tests = TestResult::NotRequested;
    let mut failed_test = None;
    if opts.run_passthru_tests {
        let handle = tracker.start(UpdatePhase::Testing).await;
        let mut failed = Vec::new();
        let mut passed = false;
        for attr in &targets {
            match crate::commands::update::run_package_tests(worktree_entry, attr, true).await {
                Ok(TestResult::Passed) => passed = true,
                Ok(TestResult::Failed(stderr)) => {
                    failed_test.get_or_insert_with(|| (*attr).to_owned());
                    failed.push(format!("{attr}:\n{stderr}"));
                },
                Ok(_) => {},
                Err(e) => warn!("{}: failed to run passthru.tests: {e:#}", attr),
            }
        }
        tests = if !failed.is_empty() {
            let output = failed.join("\n\n");
            handle
                .fail(UpdateError::TestError {
                    phase: UpdatePhase::Testing,
                    test_name: None,
                    output: output.clone(),
                    exit_code: 1,
                })
                .await;
            TestResult::Failed(output)
        } else if passed {
            handle.success().await;
            TestResult::Passed
        } else {
            handle.skipped().await;
            TestResult::NoTests
        };
    }

    Ok(Verified {
        tests,
        fixed_by_claude,
        failed_test,
    })
}

/// Ask Claude to fix `attr`'s build against the updated group and commit
/// the fix on its own. Returns whether the build was fixed.
async fn claude_fix_group(
    cfg: &ClaudeFixConfig,
    group: &GroupRequest,
    tracker: &PhaseTracker,
    workspace: &GroupWorkspace<'_>,
    attr: &str,
    stderr: &str,
) -> bool {
    let (worktree_path, worktree_entry) = (workspace.path, workspace.entry);
    let (relative_file, content) = match get_file_location(worktree_entry, attr).await {
        Ok(loc) => match relative_to_repo(Path::new(&loc), worktree_path) {
            Some(rel) => {
                let content = tokio::fs::read_to_string(worktree_path.join(&rel))
                    .await
                    .unwrap_or_default();
                (rel, content)
            },
            None => (PathBuf::new(), String::new()),
        },
        Err(_) => (PathBuf::new(), String::new()),
    };
    let summary = group
        .members
        .iter()
        .map(|m| format!("{} {} -> {}", m.attr_path, m.current_version, m.new_version))
        .collect::<Vec<_>>()
        .join(", ");
    let message =
        format!("{attr} fails to build after updating its update group ({summary}):\n\n{stderr}");
    let handle = tracker.start(UpdatePhase::AgentFix).await;
    let fix = super::claude_fix::attempt_claude_fix(
        worktree_path,
        worktree_entry,
        attr,
        &message,
        Some(stderr),
        &relative_file.to_string_lossy(),
        &content,
        "(unchanged)",
        "(unchanged)",
        cfg,
    )
    .await;
    record_agent_fix(handle, &fix).await;
    log_unfixed(attr, &fix);
    if !matches!(fix, ClaudeFixResult::Fixed) {
        return false;
    }
    let title = format!("{}: fix build with updated dependencies", group.root);
    let pkg_dir = (!relative_file.as_os_str().is_empty())
        .then(|| worktree_path.join(&relative_file))
        .and_then(|file| file.parent().map(Path::to_path_buf));
    match workspace
        .commit(&group.root, pkg_dir.as_deref(), &title)
        .await
    {
        Ok(_) => true,
        Err(e) => {
            warn!("group {}: failed to commit Claude fix: {e:#}", group.root);
            false
        },
    }
}

/// `nix-instantiate <entry> -A <attr>`: the attr's derivation path.
async fn instantiate(entry: &str, attr: &str) -> anyhow::Result<String> {
    let entry = normalize_entry_point(entry);
    let output = Command::new("nix-instantiate")
        .arg(entry.as_ref())
        .arg("-A")
        .arg(attr)
        .output()
        .await
        .context("run nix-instantiate")?;
    if !output.status.success() {
        anyhow::bail!(
            "nix-instantiate -A {attr} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let drv = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    // `drv!out` style paths from multi-output attrs.
    Ok(drv.split('!').next().unwrap_or_default().to_owned())
}

/// The order members are committed in: dependencies first (from the
/// derivation graph), falling back to alphabetical order.
async fn commit_order(entry: &str, members: &[String]) -> Vec<String> {
    let mut drvs: HashMap<String, String> = HashMap::new();
    for attr in members {
        match instantiate(entry, attr).await {
            Ok(drv) => {
                drvs.insert(drv, attr.clone());
            },
            Err(e) => debug!("{}: {e:#}; ordering without it", attr),
        }
    }
    let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
    for (drv, attr) in &drvs {
        let output = Command::new("nix-store")
            .args(["--query", "--requisites", drv])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                let needs = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter_map(|line| drvs.get(line.trim()).cloned())
                    .collect();
                deps.insert(attr.clone(), needs);
            },
            Ok(out) => debug!(
                "{}: nix-store --query --requisites failed: {}",
                attr,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => debug!("{}: nix-store: {e}", attr),
        }
    }
    crate::groups::dependency_order(members, &deps)
}

/// The PR title of a group: the single member's update title, or
/// `<root> group: update <n> packages`.
fn group_title(root: &str, updated: &[&UpdateRequest]) -> String {
    match updated {
        [only] => {
            crate::git::update_title(&only.attr_path, &only.current_version, &only.new_version)
        },
        _ => format!("{root} group: update {} packages", updated.len()),
    }
}

/// Push the group branch and open (or refresh) its PR.
async fn create_group_pr(
    opts: &GroupUpdateOptions<'_>,
    group: &GroupRequest,
    updated: &[&UpdateRequest],
    worktree_path: &Path,
    worktree_entry: &str,
    config: &PrConfig,
    rebuild_analysis: Option<&crate::nix::rebuild_count::RebuildAnalysis>,
    test_result: &TestResult,
    fixed_by_claude: bool,
) -> anyhow::Result<(String, i64)> {
    let token = std::env::var("GITHUB_TOKEN")
        .map_err(|_| anyhow::anyhow!("GITHUB_TOKEN environment variable not set"))?;

    let branch = crate::git::group_branch_name(&group.root);
    crate::git::force_push_branch(worktree_path, &branch, opts.fork).await?;

    let entry = normalize_entry_point(worktree_entry);
    let mut body = format!(
        "## Summary\n\nThis PR updates the `{}` update group. Each package is updated in its \
         own commit, in dependency order.\n\n| Package | From | To |\n| --- | --- | --- |\n",
        group.root
    );
    for req in updated {
        body.push_str(&format!(
            "| `{}` | {} | {} |\n",
            req.attr_path, req.current_version, req.new_version
        ));
    }

    let updated_attrs: HashSet<&str> = updated.iter().map(|r| r.attr_path.as_str()).collect();
    let mut verified: Vec<&str> = group
        .verify
        .iter()
        .map(String::as_str)
        .filter(|a| !updated_attrs.contains(a))
        .collect();
    verified.sort_unstable();
    verified.dedup();
    if !verified.is_empty() {
        body.push_str(&format!(
            "\nAlso built against the updated packages: {}\n",
            verified
                .iter()
                .map(|a| format!("`{a}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if fixed_by_claude {
        body.push_str(&format!(
            "\nThe group did not build as is; the last commit (`{}: fix build with updated \
             dependencies`) was made by Claude Code.\n",
            group.root
        ));
    }

    // Per-member package information and CVE changes.
    let mut info_sections = String::new();
    let mut cve_sections = String::new();
    let mut audit_sections = String::new();
    for req in updated {
        let Ok(meta) = PackageMetadata::from_attr_path(&entry, &req.attr_path).await else {
            continue;
        };
        let mut lines = Vec::new();
        if let Some(d) = &meta.description {
            lines.push(format!("**Description:** {d}"));
        }
        if let Some(h) = &meta.homepage {
            lines.push(format!("**Homepage:** {h}"));
        }
        if let Some(c) = &meta.changelog {
            lines.push(format!("**Changelog:** {c}"));
        }
        if !lines.is_empty() {
            info_sections.push_str(&format!(
                "\n\n### `{}`\n\n{}",
                req.attr_path,
                lines.join("\n\n")
            ));
        }

        if let Ok(analysis) = crate::cve::analyze_cve_changes(
            opts.db.pool(),
            &meta,
            &req.current_version,
            &req.new_version,
            opts.pr_enhancements.skip_cve_check,
        )
        .await
        {
            if let Err(e) = opts
                .db
                .record_cve_counts(
                    req.repo(),
                    &req.attr_path,
                    analysis.resolved.len(),
                    analysis.introduced.len(),
                )
                .await
            {
                warn!("{}: Failed to record CVE counts: {}", req.attr_path, e);
            }
            if let Some(md) = analysis.to_markdown() {
                cve_sections.push_str(&format!("\n\n<!-- {} -->\n{md}", req.attr_path));
            }
        }

        if let Err(e) = opts
            .pr_enhancements
            .perform_worktree_cachix_push(worktree_path, &entry, &req.attr_path)
            .await
        {
            warn!("{}: Cachix push raised an error: {:#}", req.attr_path, e);
        }
        match opts
            .pr_enhancements
            .perform_worktree_audit(worktree_path, &entry, &req.attr_path)
            .await
        {
            Ok(Some(md)) => audit_sections.push_str(&format!("\n\n{md}")),
            Ok(None) => {},
            Err(e) => warn!("{}: Audit failed: {:#}", req.attr_path, e),
        }
    }
    if !info_sections.is_empty() {
        body.push_str("\n## Package Information");
        body.push_str(&info_sections);
    }
    if let Some(analysis) = rebuild_analysis {
        body.push_str(&super::updater::rebuild_impact_section(analysis));
    }
    body.push_str(&cve_sections);
    body.push_str(&audit_sections);
    let (tests, draft) = super::updater::test_section(test_result);
    body.push_str(&tests);
    body.push_str(
        "\n\n---\n\nThis branch is regenerated from the base branch (and force-pushed) whenever \
         a member of the group has a new release.\n\n🤖 Generated with ekapkgs-update",
    );

    let title = group_title(&group.root, updated);

    let push_owner = match crate::git::remote_github_owner(worktree_path, opts.fork).await {
        Ok(owner) => Some(owner),
        Err(e) => {
            debug!(
                "group {}: owner of remote '{}' unknown: {e:#}",
                group.root, opts.fork
            );
            None
        },
    };
    let head_owner = push_owner.as_deref().unwrap_or(&config.owner);
    let qualified_head = format!("{head_owner}:{branch}");

    if let Some(existing) =
        crate::github::find_open_pull_request(&config.owner, &config.repo, &qualified_head, &token)
            .await?
    {
        let pr = crate::github::update_pull_request(
            &config.owner,
            &config.repo,
            existing.number,
            &title,
            &body,
            &token,
        )
        .await?;
        info!("group {}: refreshed PR #{}", group.root, pr.number);
        return Ok((pr.html_url, pr.number));
    }

    let head = crate::git::pr_head(&config.owner, push_owner.as_deref(), &branch);
    let pr = crate::github::create_pull_request_with_options(
        &config.owner,
        &config.repo,
        &title,
        &body,
        &head,
        &config.base_branch,
        &token,
        draft,
    )
    .await?;
    Ok((pr.html_url, pr.number))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(attr: &str, old: &str, new: &str) -> UpdateRequest {
        UpdateRequest {
            attr_path: attr.to_owned(),
            drv_path: None,
            current_version: old.to_owned(),
            new_version: new.to_owned(),
            repo_name: None,
            eval_entry_point: None,
            pr_config: None,
            fork: None,
        }
    }

    #[test]
    fn titles_groups() {
        let a = req("aws-c-io", "0.14.0", "0.15.0");
        let b = req("aws-crt-cpp", "0.29.0", "0.30.0");
        assert_eq!(
            group_title("aws-crt-cpp", &[&a]),
            "aws-c-io: 0.14.0 -> 0.15.0"
        );
        assert_eq!(
            group_title("aws-crt-cpp", &[&a, &b]),
            "aws-crt-cpp group: update 2 packages"
        );
    }

    /// An entry point outside the repository fails every member (recorded,
    /// backing off); a worktree that cannot be created skips them all.
    #[tokio::test]
    async fn group_setup_failure_reaches_every_member() {
        use crate::database::{SessionMode, UpdateStatus};

        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let db = Database::from(pool);
        let session = db.create_session(SessionMode::Watch, None).await.unwrap();
        let members = [req("aws-c-io", "1", "2"), req("aws-crt-cpp", "1", "2")];
        let attempts: Vec<Attempt<'_>> = members
            .iter()
            .map(|m| Attempt::new(&db, &session, m))
            .collect();

        let transient = SetupFailure::Transient("Worktree creation failed: locked".to_owned());
        let outputs = setup_failed(&attempts, &transient).await;
        assert_eq!(outputs.len(), 2);
        assert!(
            outputs
                .iter()
                .all(|(r, _)| matches!(r, Ok(UpdateResult::Skipped(_))))
        );
        for m in &members {
            assert!(
                db.get_update_record("", &m.attr_path)
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        let lasting = entry_point_in_repo("/elsewhere/default.nix", Path::new("/repo"))
            .expect_err("outside the repository");
        let outputs = setup_failed(&attempts, &lasting).await;
        assert_eq!(
            outputs.iter().map(|(_, a)| a.as_str()).collect::<Vec<_>>(),
            ["aws-c-io", "aws-crt-cpp"]
        );
        for (result, attr) in &outputs {
            assert!(matches!(result, Ok(UpdateResult::Failed(_))), "{attr}");
            let record = db.get_update_record("", attr).await.unwrap().unwrap();
            assert_eq!(record.status, UpdateStatus::Failed, "{attr}");
            assert!(record.in_backoff(chrono::Utc::now()), "{attr}");
        }
    }

    #[test]
    fn failure_is_attributed_to_the_member_that_failed() {
        let failure = GroupFailure {
            attr_path: "aws-c-io".to_owned(),
            error: UpdateError::BuildError {
                phase: UpdatePhase::Build,
                exit_code: 1,
                stderr: "boom".to_owned(),
                suspected_cause: None,
            },
            error_log: "build log".to_owned(),
        };
        // (member, component of its error (None: the failure itself), log)
        let cases = [
            ("aws-c-io", None, "build log"),
            (
                "aws-crt-cpp",
                Some("group"),
                "group aws-crt-cpp failed: aws-c-io (build): ",
            ),
        ];
        for (member, component, log) in cases {
            let (error, error_log) = member_failure("aws-crt-cpp", &failure, member);
            assert_eq!(error.phase(), UpdatePhase::Build, "{member}");
            match (component, &error) {
                (None, UpdateError::BuildError { stderr, .. }) => assert_eq!(stderr, "boom"),
                (
                    Some(expected),
                    UpdateError::InfrastructureError {
                        component, details, ..
                    },
                ) => {
                    assert_eq!(component, expected);
                    assert!(details.starts_with(log), "{details}");
                },
                other => panic!("{member}: unexpected {other:?}"),
            }
            assert!(error_log.starts_with(log), "{member}: {error_log}");
            assert!(error_log.ends_with("build log"), "{member}: {error_log}");
        }
    }

    #[test]
    fn failed_tests_fail_a_branch_group() {
        let verified = |tests, failed_test: Option<&str>| Verified {
            tests,
            fixed_by_claude: false,
            failed_test: failed_test.map(str::to_owned),
        };
        assert!(verified(TestResult::Passed, None).test_failure().is_none());
        assert!(verified(TestResult::NoTests, None).test_failure().is_none());
        assert!(
            verified(TestResult::NotRequested, None)
                .test_failure()
                .is_none()
        );
        let failure = verified(TestResult::Failed("x:\nfailed".to_owned()), Some("x"))
            .test_failure()
            .expect("failed tests fail the group");
        assert_eq!(failure.attr_path, "x");
        assert_eq!(failure.error.phase(), UpdatePhase::Testing);
        assert!(failure.error_log.contains("x:\nfailed"));
    }

    #[test]
    fn dry_run_reports_members() {
        let group = GroupRequest {
            root: "r".to_owned(),
            members: vec![req("a", "1", "2"), req("b", "1", "2")],
            verify: vec![],
        };
        let outputs = dry_run_outputs(&group);
        assert_eq!(
            outputs.iter().map(|(_, a)| a.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(
            outputs
                .iter()
                .all(|(r, _)| matches!(r, Ok(UpdateResult::DryRun { .. })))
        );
    }

    // ── Branch-mode groups, with the nix steps faked ───────────────

    /// Stands in for nix: members are "updated" by writing their new version
    /// into `pkgs/<attr>/default.nix`, in the order given.
    #[derive(Default)]
    struct FakeSteps {
        order: Vec<&'static str>,
        /// Member whose update fails (after editing its file).
        failing_member: Option<&'static str>,
        /// Verified attr whose build fails.
        failing_build: Option<&'static str>,
        /// Member already at its target version.
        already: Option<&'static str>,
    }

    fn build_error(stderr: &str) -> UpdateError {
        UpdateError::BuildError {
            phase: UpdatePhase::Build,
            exit_code: 1,
            stderr: stderr.to_owned(),
            suspected_cause: None,
        }
    }

    impl GroupSteps for FakeSteps {
        async fn commit_order(&self, _entry: &str, _members: &[String]) -> Vec<String> {
            self.order.iter().map(|a| (*a).to_owned()).collect()
        }

        async fn edit_member(
            &self,
            _opts: &GroupUpdateOptions<'_>,
            attempt: &Attempt<'_>,
            workspace: &GroupWorkspace<'_>,
        ) -> Result<MemberEdit, Box<GroupFailure>> {
            let req = attempt.req;
            if self.already == Some(req.attr_path.as_str()) {
                return Ok(MemberEdit::AlreadyAtVersion(AlreadyAtVersion {
                    current: req.new_version.clone(),
                    target: req.new_version.clone(),
                }));
            }
            let dir = workspace.path.join("pkgs").join(&req.attr_path);
            std::fs::write(dir.join("default.nix"), &req.new_version).unwrap();
            if self.failing_member == Some(req.attr_path.as_str()) {
                let log = format!("{} does not build", req.attr_path);
                return Err(GroupFailure::of(&req.attr_path, build_error(&log), log));
            }
            Ok(MemberEdit::Updated {
                removed_patches: Vec::new(),
                components: Vec::new(),
                pkg_dir: Some(dir),
            })
        }

        async fn verify(
            &self,
            _opts: &GroupUpdateOptions<'_>,
            _group: &GroupRequest,
            _tracker: &PhaseTracker,
            _workspace: &GroupWorkspace<'_>,
        ) -> Result<Verified, Box<GroupFailure>> {
            if let Some(attr) = self.failing_build {
                let log = format!("{attr} does not build with the group");
                return Err(GroupFailure::of(attr, build_error(&log), log));
            }
            Ok(Verified {
                tests: TestResult::NotRequested,
                fixed_by_claude: false,
                failed_test: None,
            })
        }
    }

    fn git_out(root: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
    }

    /// A repository with `pkgs/{a,b,c}` at version 1 and uncommitted user
    /// work (`notes.txt`).
    fn group_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        git_out(&root, &["init", "-q"]);
        git_out(&root, &["config", "user.email", "test@example.com"]);
        git_out(&root, &["config", "user.name", "test"]);
        std::fs::write(root.join("default.nix"), "{ }").unwrap();
        for attr in ["a", "b", "c"] {
            std::fs::create_dir_all(root.join("pkgs").join(attr)).unwrap();
            std::fs::write(root.join("pkgs").join(attr).join("default.nix"), "1").unwrap();
        }
        git_out(&root, &["add", "-A"]);
        git_out(&root, &["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("notes.txt"), "mine").unwrap();
        (dir, root)
    }

    struct BranchGroupRun {
        outputs: Vec<MemberOutput>,
        /// `(subject, files)` of the commits the group left, oldest first.
        commits: Vec<(String, String)>,
        status: String,
        files: Vec<String>,
        db: Database,
    }

    /// Run a group of `a` and `b` (root `b`, both `1 -> 2`) on the current
    /// branch with `steps`.
    async fn run_branch_group(steps: FakeSteps) -> BranchGroupRun {
        let (_dir, root) = group_repo();
        let start = git_out(&root, &["rev-parse", "HEAD"]);
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let db = Database::from(pool);
        let session = db
            .create_session(crate::database::SessionMode::Run, None)
            .await
            .unwrap();
        let entry = root.join("default.nix").to_string_lossy().into_owned();
        let pr_enhancements = PrEnhancementsConfig::default();
        let opts = GroupUpdateOptions {
            db: &db,
            session_id: &session,
            eval_entry_point: &entry,
            pr_config: None,
            fork: "origin",
            run_passthru_tests: false,
            dry_run: false,
            pr_enhancements: &pr_enhancements,
            preserve_failures: false,
            claude_fix: None,
        };
        let group = GroupRequest {
            root: "b".to_owned(),
            members: vec![req("a", "1", "2"), req("b", "1", "2")],
            verify: vec!["a".to_owned(), "b".to_owned()],
        };
        let outputs = branch_group_update(&steps, &opts, &group, &Mutex::new(())).await;

        let log = git_out(
            &root,
            &[
                "log",
                "--reverse",
                "--format=%x00%s",
                "--name-only",
                &format!("{start}.."),
            ],
        );
        let commits = log
            .split('\0')
            .filter(|c| !c.is_empty())
            .map(|c| {
                let (subject, files) = c.split_once('\n').unwrap_or((c, ""));
                (subject.to_owned(), files.trim().to_owned())
            })
            .collect();
        let status = git_out(&root, &["status", "--porcelain=v1"]);
        let files = ["a", "b", "c"]
            .iter()
            .map(|a| {
                std::fs::read_to_string(root.join("pkgs").join(a).join("default.nix")).unwrap()
            })
            .collect();
        BranchGroupRun {
            outputs,
            commits,
            status,
            files,
            db,
        }
    }

    /// `(error_type, error_log)` of a member's failure log.
    async fn failure_log(db: &Database, attr: &str) -> (String, String) {
        sqlx::query_as("SELECT error_type, error_log FROM update_logs WHERE attr_path = ?")
            .bind(attr)
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    /// Members are committed one by one in dependency order (not member
    /// order), touching only their own files, and recorded as proposed.
    #[tokio::test]
    async fn branch_group_commits_members_in_order() {
        use crate::database::UpdateStatus;

        let run = run_branch_group(FakeSteps {
            order: vec!["b", "a"],
            ..FakeSteps::default()
        })
        .await;

        assert_eq!(
            run.commits,
            [
                ("b: 1 -> 2".to_owned(), "pkgs/b/default.nix".to_owned()),
                ("a: 1 -> 2".to_owned(), "pkgs/a/default.nix".to_owned()),
            ]
        );
        assert_eq!(run.status, "?? notes.txt");
        assert_eq!(run.files, ["2", "2", "1"]);
        assert_eq!(
            run.outputs
                .iter()
                .map(|(_, a)| a.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        for (result, attr) in &run.outputs {
            assert!(matches!(result, Ok(UpdateResult::Updated { .. })), "{attr}");
            let record = run.db.get_update_record("", attr).await.unwrap().unwrap();
            assert_eq!(record.status, UpdateStatus::Proposed, "{attr}");
            assert_eq!(record.proposed_version.as_deref(), Some("2"), "{attr}");
        }
    }

    /// A member already at its target is recorded as observed and not
    /// committed; the rest of the group goes ahead.
    #[tokio::test]
    async fn branch_group_skips_member_already_at_version() {
        use crate::database::UpdateStatus;

        let run = run_branch_group(FakeSteps {
            order: vec!["a", "b"],
            already: Some("a"),
            ..FakeSteps::default()
        })
        .await;

        assert_eq!(
            run.commits,
            [("b: 1 -> 2".to_owned(), "pkgs/b/default.nix".to_owned())]
        );
        let a = run.db.get_update_record("", "a").await.unwrap().unwrap();
        assert_eq!(a.status, UpdateStatus::UpToDate);
        let b = run.db.get_update_record("", "b").await.unwrap().unwrap();
        assert_eq!(b.status, UpdateStatus::Proposed);
    }

    /// A failing member (after another was committed) or a failing
    /// verification drops the group's commits and edits, keeps the user's
    /// work, and records the failing attr's own error and `group <root>
    /// failed: <attr> (<phase>)` for the others.
    #[tokio::test]
    async fn branch_group_failure_undoes_and_attributes() {
        use crate::database::UpdateStatus;

        // (steps, attr that failed, its error_log)
        let cases = [
            (
                FakeSteps {
                    order: vec!["b", "a"],
                    failing_member: Some("a"),
                    ..FakeSteps::default()
                },
                "a",
                "a does not build",
            ),
            (
                FakeSteps {
                    order: vec!["a", "b"],
                    failing_build: Some("b"),
                    ..FakeSteps::default()
                },
                "b",
                "b does not build with the group",
            ),
        ];
        for (steps, failed, own_log) in cases {
            let run = run_branch_group(steps).await;

            assert!(run.commits.is_empty(), "{failed}: {:?}", run.commits);
            assert_eq!(run.status, "?? notes.txt", "{failed}");
            assert_eq!(run.files, ["1", "1", "1"], "{failed}");
            for (result, attr) in &run.outputs {
                assert!(
                    matches!(result, Ok(UpdateResult::Failed(_))),
                    "{failed}: {attr}"
                );
                let record = run.db.get_update_record("", attr).await.unwrap().unwrap();
                assert_eq!(record.status, UpdateStatus::Failed, "{failed}: {attr}");
                assert_eq!(
                    record.failed_version.as_deref(),
                    Some("2"),
                    "{failed}: {attr}"
                );
                assert!(record.in_backoff(chrono::Utc::now()), "{failed}: {attr}");

                let (error_type, log) = failure_log(&run.db, attr).await;
                if attr == failed {
                    assert_eq!((error_type.as_str(), log.as_str()), ("BuildError", own_log));
                } else {
                    assert_eq!(error_type, "InfrastructureError", "{failed}: {attr}");
                    let prefix = format!("group b failed: {failed} (build): ");
                    assert!(log.starts_with(&prefix), "{failed}: {attr}: {log}");
                    assert!(log.ends_with(own_log), "{failed}: {attr}: {log}");
                }
            }
        }
    }
}
