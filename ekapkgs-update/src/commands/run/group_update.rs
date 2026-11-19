//! Update groups in worktree mode: every member of a [`GroupRequest`] is
//! updated in one worktree, one commit per member in dependency order, then
//! the whole group (every member and the root) is built and proposed as one
//! PR on a stable branch (`update/group/<root>`).
//!
//! Groups are all-or-nothing: when a member's update or the final
//! verification fails, nothing is proposed. The member that failed records
//! its own error; the others record "group <root> failed" and back off.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use tokio::process::Command;
use tracing::{debug, info, warn};

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
use crate::commands::update::{AlreadyAtVersion, TestResult, VersionConfig};
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

/// Update every member of `group` in one worktree and propose it as one PR.
///
/// Returns one result per member, in the order of `group.members`.
pub(super) async fn perform_group_update(
    opts: &GroupUpdateOptions<'_>,
    group: &GroupRequest,
) -> Vec<MemberOutput> {
    let label = |req: &UpdateRequest| req.attr_path.clone();
    if opts.dry_run {
        return group
            .members
            .iter()
            .map(|req| {
                (
                    Ok(UpdateResult::DryRun {
                        current_version: req.current_version.clone(),
                        new_version: req.new_version.clone(),
                    }),
                    label(req),
                )
            })
            .collect();
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

    let attrs: Vec<String> = group.members.iter().map(|m| m.attr_path.clone()).collect();
    let order = commit_order(&worktree_entry_str, &attrs).await;
    info!(
        "group {}: updating {} member(s) in order: {}",
        group.root,
        order.len(),
        order.join(", ")
    );

    let mut updated: Vec<Updated> = Vec::new();
    let mut failure: Option<Box<GroupFailure>> = None;

    for attr in &order {
        let index = attrs.iter().position(|a| a == attr).expect("member");
        let attempt = &attempts[index];
        match update_member(opts, attempt, &worktree_path, &worktree_entry_str).await {
            Ok(MemberUpdate::Updated(removed_patches)) => updated.push(Updated {
                index,
                removed_patches,
            }),
            Ok(MemberUpdate::AlreadyAtVersion(already)) => {
                results[index] = Some(attempt.already_at_version(&already).await);
            },
            Err(f) => {
                failure = Some(f);
                break;
            },
        }
    }

    if failure.is_none() && updated.is_empty() {
        info!("group {}: no member needed an update", group.root);
        cleanup(&repo_dir, &worktree_path, &group.root).await;
        return collect(group, results);
    }

    // Verify the group as a whole: every member and the root.
    let mut test_result = TestResult::NotRequested;
    let mut fixed_by_claude = false;
    if failure.is_none() {
        let root_tracker = PhaseTracker::new(
            opts.db.clone(),
            opts.session_id,
            group.lead().repo(),
            &group.root,
        );
        match verify_group(
            opts,
            group,
            &root_tracker,
            &worktree_path,
            &worktree_entry_str,
        )
        .await
        {
            Ok(verified) => {
                test_result = verified.tests;
                fixed_by_claude = verified.fixed_by_claude;
            },
            Err(f) => failure = Some(f),
        }
    }

    if let Some(failure) = failure {
        warn!(
            "group {}: {} failed ({}): {}",
            group.root,
            failure.attr_path,
            failure.error.phase(),
            failure.error.summary()
        );
        let preserved = preserve(
            opts,
            group,
            &failure,
            &repo_dir,
            &worktree_path,
            &worktree_entry,
        )
        .await;
        for (index, attempt) in attempts.iter().enumerate() {
            if results[index].is_some() {
                continue;
            }
            let result = if attempt.req.attr_path == failure.attr_path {
                attempt
                    .record_failure(&failure.error, &failure.error_log)
                    .await
            } else {
                let details = format!(
                    "group {} failed: {} ({}): {}",
                    group.root,
                    failure.attr_path,
                    failure.error.phase(),
                    failure.error.summary()
                );
                let error = UpdateError::InfrastructureError {
                    phase: failure.error.phase(),
                    component: "group".to_owned(),
                    details: details.clone(),
                };
                attempt
                    .record_failure(&error, &format!("{details}\n\n{}", failure.error_log))
                    .await
            };
            results[index] = Some(result);
        }
        if !preserved {
            cleanup(&repo_dir, &worktree_path, &group.root).await;
        }
        return collect(group, results);
    }

    for u in &updated {
        if !u.removed_patches.is_empty() {
            info!(
                "{}: Removed {} obsolete patch(es): {}",
                attrs[u.index],
                u.removed_patches.len(),
                u.removed_patches.join(", ")
            );
        }
    }

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

enum MemberUpdate {
    Updated(Vec<String>),
    AlreadyAtVersion(AlreadyAtVersion),
}

/// Update one member in the group worktree and commit it on its own. A
/// Claude fix of the member's own build failure is part of its commit.
async fn update_member(
    opts: &GroupUpdateOptions<'_>,
    attempt: &Attempt<'_>,
    worktree_path: &Path,
    worktree_entry: &str,
) -> Result<MemberUpdate, Box<GroupFailure>> {
    let req = attempt.req;
    let attr_path = &req.attr_path;
    let fail = |error: UpdateError, error_log: String| {
        Box::new(GroupFailure {
            attr_path: attr_path.clone(),
            error,
            error_log,
        })
    };
    let infra =
        |phase: UpdatePhase, component: &str, details: String| UpdateError::InfrastructureError {
            phase,
            component: component.to_owned(),
            details,
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
                return Ok(MemberUpdate::AlreadyAtVersion(already.clone()));
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

    let title = crate::git::update_title(attr_path, &req.current_version, &req.new_version);
    let message = format!(
        "{title}{}",
        crate::commands::update::components_commit_lines(&components)
    );
    match crate::git::commit_all(worktree_path, &message).await {
        Ok(true) => {},
        Ok(false) => debug!("{}: update changed no files", attr_path),
        Err(e) => {
            let error = UpdateError::GitError {
                phase: UpdatePhase::PrCreation,
                operation: "commit".to_owned(),
                details: format!("{e:#}"),
            };
            attempt.tracker.record_failure(error.clone()).await;
            return Err(fail(error, format!("{e:#}")));
        },
    }
    Ok(MemberUpdate::Updated(removed_patches))
}

/// Outcome of a successful group verification.
struct Verified {
    tests: TestResult,
    fixed_by_claude: bool,
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
    worktree_path: &Path,
    worktree_entry: &str,
) -> Result<Verified, Box<GroupFailure>> {
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
                    if claude_fix_group(
                        cfg,
                        group,
                        tracker,
                        worktree_path,
                        worktree_entry,
                        attr,
                        &stderr,
                    )
                    .await
                    {
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
    if opts.run_passthru_tests {
        let handle = tracker.start(UpdatePhase::Testing).await;
        let mut failed = Vec::new();
        let mut passed = false;
        for attr in &targets {
            match crate::commands::update::run_package_tests(worktree_entry, attr, true).await {
                Ok(TestResult::Passed) => passed = true,
                Ok(TestResult::Failed(stderr)) => failed.push(format!("{attr}:\n{stderr}")),
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
    })
}

/// Ask Claude to fix `attr`'s build against the updated group and commit
/// the fix on its own. Returns whether the build was fixed.
async fn claude_fix_group(
    cfg: &ClaudeFixConfig,
    group: &GroupRequest,
    tracker: &PhaseTracker,
    worktree_path: &Path,
    worktree_entry: &str,
    attr: &str,
    stderr: &str,
) -> bool {
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
    match crate::git::commit_all(worktree_path, &title).await {
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
}
