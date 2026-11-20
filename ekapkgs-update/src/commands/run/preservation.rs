//! Artifact preservation system for failed updates
//!
//! This module handles preservation of failed update worktrees and associated
//! artifacts (logs, diffs, metadata) for later inspection and remediation.

use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::branch_git::{DirtySnapshot, GroupCheckpoint};
use crate::commands::update::errors::UpdateError;
use crate::commands::update::types::UpdatePhase;

/// Artifacts saved when an update fails
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureArtifacts {
    pub session_id: String,
    /// Repository scope (`''` for `run` / single-repo watch). Missing in
    /// manifests written before multi-repo support.
    #[serde(default)]
    pub repo_name: String,
    pub attr_path: String,
    pub failed_phase: UpdatePhase,
    pub timestamp: DateTime<Utc>,

    // Paths to preserved artifacts
    pub worktree_path: PathBuf,
    pub build_log_path: Option<PathBuf>,
    pub test_output_path: Option<PathBuf>,
    pub diff_path: PathBuf,
    pub metadata_path: PathBuf,
    pub error_context_path: PathBuf,
}

impl FailureArtifacts {
    /// Directory structure:
    /// `~/.cache/ekapkgs-update/failed/{session_id}/[{repo_name}+]{attr_path}/`
    /// (one watch session can fail the same attr in several repos; see
    /// [`repo_scoped_name`]). Lookups read `manifest.json`, not the directory
    /// name, so older `{repo_name}-{attr_path}` directories are still found.
    ///
    /// `failed_root` is [`Self::failed_root`] outside of tests.
    pub fn base_path_in(
        failed_root: &Path,
        session_id: &str,
        repo_name: &str,
        attr_path: &str,
    ) -> PathBuf {
        failed_root
            .join(session_id)
            .join(package_dir_name(repo_name, attr_path))
    }

    /// `~/.cache/ekapkgs-update/failed`: where failures are preserved.
    pub fn failed_root() -> anyhow::Result<PathBuf> {
        let base_dirs = directories::ProjectDirs::from("com", "ekapusta", "ekapkgs-update")
            .context("Failed to determine project directories")?;
        Ok(base_dirs.cache_dir().join("failed"))
    }

    /// The package's artifact directory (`failed/{session_id}/[{repo}-]{attr}/`),
    /// holding the manifest, logs, diff and the `worktree/` copy. This is the
    /// path recorded as `update_phases.artifacts_path` / `autofix_queue.artifacts_path`.
    pub fn package_dir(&self) -> &Path {
        self.worktree_path
            .parent()
            .unwrap_or(self.worktree_path.as_path())
    }
}

/// Separator between the repo name and the attr path in repo-scoped names.
/// Repo names cannot contain it (`config::is_valid_repo_name`), so the first
/// `+` always ends the repo name: the encoding stays unambiguous although
/// both repo names and attr paths may contain `-`.
const REPO_SEPARATOR: char = '+';

/// Unique name for `attr_path` within `repo_name`: the attr path itself for
/// the unnamed scope, `{repo_name}+{attr_path}` otherwise. Used for worktree
/// names and preserved-failure directories, so updates in different repos of
/// one watch cycle never share (and delete) each other's worktree.
pub fn repo_scoped_name(repo_name: &str, attr_path: &str) -> String {
    if repo_name.is_empty() {
        attr_path.to_owned()
    } else {
        format!("{repo_name}{REPO_SEPARATOR}{attr_path}")
    }
}

fn package_dir_name(repo_name: &str, attr_path: &str) -> String {
    repo_scoped_name(repo_name, &attr_path.replace(['.', '/'], "_"))
}

/// Preserve a failed update's context for later inspection.
///
/// The worktree itself ends up in `<artifacts>/worktree`: it is moved there
/// with `git worktree move` (the original location is gone afterwards, and
/// the preserved copy stays a working git worktree, which `autofix` and
/// `apply` rely on). If the move fails (submodules, another filesystem), it
/// is copied instead and the original must be kept, since the copy's `.git`
/// file still points at its git metadata. Either way the caller must not
/// remove the worktree after a successful preservation.
pub async fn preserve_failure(
    session_id: &str,
    repo_name: &str,
    attr_path: &str,
    phase: UpdatePhase,
    repo_dir: &Path,
    worktree_path: &Path,
    entry_point: &Path,
    error: &UpdateError,
    build_log: Option<String>,
    test_output: Option<String>,
) -> anyhow::Result<FailureArtifacts> {
    preserve_failure_in(
        &FailureArtifacts::failed_root()?,
        session_id,
        repo_name,
        attr_path,
        phase,
        repo_dir,
        worktree_path,
        entry_point,
        error,
        build_log,
        test_output,
    )
    .await
}

/// [`preserve_failure`] under `failed_root` instead of the cache's `failed/`.
async fn preserve_failure_in(
    failed_root: &Path,
    session_id: &str,
    repo_name: &str,
    attr_path: &str,
    phase: UpdatePhase,
    repo_dir: &Path,
    worktree_path: &Path,
    entry_point: &Path,
    error: &UpdateError,
    build_log: Option<String>,
    test_output: Option<String>,
) -> anyhow::Result<FailureArtifacts> {
    let base_path = FailureArtifacts::base_path_in(failed_root, session_id, repo_name, attr_path);
    fs::create_dir_all(&base_path).await?;

    info!(
        "{}: Preserving failure artifacts to {}",
        attr_path,
        base_path.display()
    );

    // Save build log if available
    let build_log_path = if let Some(log) = build_log {
        let path = base_path.join("build.log");
        fs::write(&path, log).await?;
        debug!("{}: Build log saved", attr_path);
        Some(path)
    } else {
        None
    };

    // Save test output if available
    let test_output_path = if let Some(output) = test_output {
        let path = base_path.join("test-output.log");
        fs::write(&path, output).await?;
        debug!("{}: Test output saved", attr_path);
        Some(path)
    } else {
        None
    };

    // Generate and save diff
    let diff = generate_diff(worktree_path).await?;
    let diff_path = base_path.join("changes.diff");
    fs::write(&diff_path, diff).await?;
    debug!("{}: Diff generated and saved", attr_path);

    // Save package metadata
    let metadata = extract_package_metadata(entry_point, attr_path).await?;
    let metadata_path = base_path.join("metadata.json");
    fs::write(&metadata_path, serde_json::to_string_pretty(&metadata)?).await?;
    debug!("{}: Metadata saved", attr_path);

    // Save error context (LLM-friendly JSON)
    let error_context = error.to_llm_context();
    let error_context_path = base_path.join("error-context.json");
    fs::write(
        &error_context_path,
        serde_json::to_string_pretty(&error_context)?,
    )
    .await?;
    debug!("{}: Error context saved", attr_path);

    // Move (or copy) the worktree last: the steps above read it in place.
    let preserved_worktree = base_path.join("worktree");
    relocate_worktree(attr_path, repo_dir, worktree_path, &preserved_worktree).await?;

    // Create artifacts manifest
    let artifacts = FailureArtifacts {
        session_id: session_id.to_owned(),
        repo_name: repo_name.to_owned(),
        attr_path: attr_path.to_owned(),
        failed_phase: phase,
        timestamp: Utc::now(),
        worktree_path: preserved_worktree,
        build_log_path,
        test_output_path,
        diff_path,
        metadata_path,
        error_context_path,
    };

    // Save manifest
    let manifest_path = base_path.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_string_pretty(&artifacts)?).await?;
    debug!("{}: Manifest saved", attr_path);

    info!("{}: Successfully preserved failure artifacts", attr_path);

    Ok(artifacts)
}

/// The changes of a failed branch-mode update, still in the checkout.
pub(super) enum BranchChanges<'a> {
    /// A single update: the paths it dirtied (it commits nothing on failure).
    Single(&'a DirtySnapshot),
    /// A group: its member commits and the paths it dirtied.
    Group(&'a GroupCheckpoint),
}

impl BranchChanges<'_> {
    fn root(&self) -> &Path {
        match self {
            Self::Single(snapshot) => snapshot.root(),
            Self::Group(checkpoint) => checkpoint.root(),
        }
    }

    /// The commit the changes are made against.
    async fn base(&self) -> anyhow::Result<String> {
        match self {
            // Nothing was committed: HEAD has not moved.
            Self::Single(snapshot) => git_stdout(snapshot.root(), &["rev-parse", "HEAD"]).await,
            Self::Group(checkpoint) => Ok(checkpoint.start().to_owned()),
        }
    }

    /// Recreate the changes in `dest`, a worktree checked out at [`Self::base`].
    async fn replay_into(&self, dest: &Path) -> anyhow::Result<()> {
        match self {
            Self::Single(snapshot) => snapshot.copy_introduced(dest).await,
            Self::Group(checkpoint) => checkpoint.replay_into(dest).await,
        }
    }
}

/// What to preserve of a failed update.
pub(super) struct FailureContext<'a> {
    pub session_id: &'a str,
    pub repo_name: &'a str,
    /// Attr path the artifacts are filed under.
    pub attr_path: &'a str,
    /// Entry point the update evaluated (in the checkout).
    pub eval_entry_point: &'a str,
    pub error: &'a UpdateError,
    pub error_log: &'a str,
}

/// Preserve a failed branch-mode update (single or group) like a
/// worktree-mode one, before it is undone: a detached worktree is created
/// at the commit the update started from, its changes are recreated there
/// ([`BranchChanges::replay_into`]: a group's member commits are
/// cherry-picked, uncommitted paths it dirtied are copied, the user's
/// pre-existing dirty paths are left out), and that worktree is handed to
/// [`preserve_failure`], so `export`, `apply`, `retry`, `worktrees` and
/// `autofix` find the same artifacts as for a worktree-mode failure.
///
/// Must be called with the git mutex held, before the checkout is reverted.
/// Returns the artifacts directory (linked to the failed phase in the
/// database). Failures are logged and never returned: preservation must not
/// keep the update from being undone or change its recorded outcome.
pub(super) async fn preserve_branch_failure(
    failed_root: anyhow::Result<PathBuf>,
    db: &crate::database::Database,
    ctx: &FailureContext<'_>,
    changes: BranchChanges<'_>,
) -> Option<PathBuf> {
    let attr_path = ctx.attr_path;
    let preserved = async {
        let failed_root = failed_root?;
        preserve_branch_failure_in(&failed_root, ctx, &changes).await
    }
    .await;
    match preserved {
        Ok(artifacts) => {
            let path = artifacts.package_dir().to_path_buf();
            if let Err(e) = db
                .set_failed_phase_artifacts(
                    ctx.session_id,
                    ctx.repo_name,
                    attr_path,
                    &path.to_string_lossy(),
                )
                .await
            {
                warn!("{attr_path}: Failed to link failure artifacts: {e:#}");
            }
            Some(path)
        },
        Err(e) => {
            warn!("{attr_path}: Failed to preserve failure artifacts: {e:#}");
            None
        },
    }
}

async fn preserve_branch_failure_in(
    failed_root: &Path,
    ctx: &FailureContext<'_>,
    changes: &BranchChanges<'_>,
) -> anyhow::Result<FailureArtifacts> {
    let repo_dir = changes.root();
    let relative_entry =
        super::updater::relative_to_repo(Path::new(ctx.eval_entry_point), repo_dir).with_context(
            || {
                format!(
                    "entry point {} is outside repository {}",
                    ctx.eval_entry_point,
                    repo_dir.display()
                )
            },
        )?;
    let base = changes.base().await?;
    let base_path =
        FailureArtifacts::base_path_in(failed_root, ctx.session_id, ctx.repo_name, ctx.attr_path);
    fs::create_dir_all(&base_path).await?;

    // Next to its final place (`<artifacts>/worktree`), so the move is a rename.
    let staging = base_path.join("worktree.staging");
    remove_worktree(repo_dir, &staging).await;
    let mut add = vec!["worktree", "add", "-q", "--detach"];
    let staging_str = staging.to_string_lossy();
    add.push(&staging_str);
    add.push(&base);
    git_stdout(repo_dir, &add).await?;

    let test_output = match ctx.error {
        UpdateError::TestError { output, .. } => Some(output.clone()),
        _ => None,
    };
    let preserved = async {
        changes.replay_into(&staging).await?;
        preserve_failure_in(
            failed_root,
            ctx.session_id,
            ctx.repo_name,
            ctx.attr_path,
            ctx.error.phase(),
            repo_dir,
            &staging,
            &staging.join(&relative_entry),
            ctx.error,
            Some(ctx.error_log.to_owned()),
            test_output,
        )
        .await
    }
    .await;
    if preserved.is_err() {
        remove_worktree(repo_dir, &staging).await;
    }
    preserved
}

/// Remove the worktree at `path` (if any) and forget it in `repo_dir`.
async fn remove_worktree(repo_dir: &Path, path: &Path) {
    if fs::symlink_metadata(path).await.is_ok() {
        if let Err(e) = crate::git::cleanup_worktree(repo_dir, path).await {
            debug!("Failed to remove worktree {}: {e:#}", path.display());
        }
    }
    let _ = git_stdout(repo_dir, &["worktree", "prune"]).await;
}

async fn git_stdout(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Move the worktree at `src` to `dst` with `git worktree move`, falling
/// back to a copy (keeping `src`) when git cannot move it.
async fn relocate_worktree(
    attr_path: &str,
    repo_dir: &Path,
    src: &Path,
    dst: &Path,
) -> anyhow::Result<()> {
    // Left over from an earlier preservation of the same attempt.
    if fs::symlink_metadata(dst).await.is_ok() {
        fs::remove_dir_all(dst).await?;
    }
    let moved = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(["worktree", "move"])
        .arg(src)
        .arg(dst)
        .output()
        .await;
    match moved {
        Ok(output) if output.status.success() => {
            debug!("{}: Worktree moved to {}", attr_path, dst.display());
            return Ok(());
        },
        Ok(output) => debug!(
            "{}: git worktree move failed, copying instead: {}",
            attr_path,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(e) => debug!(
            "{}: git worktree move failed, copying instead: {}",
            attr_path, e
        ),
    }
    copy_dir_all(src, dst).await?;
    debug!(
        "{}: Worktree copied (original kept at {})",
        attr_path,
        src.display()
    );
    Ok(())
}

/// Recursively copy a directory and all its contents
fn copy_dir_all<'a>(
    src: &'a Path,
    dst: &'a Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        fs::create_dir_all(dst).await?;
        let mut entries = fs::read_dir(src).await?;

        while let Some(entry) = entries.next_entry().await? {
            let ty = entry.file_type().await?;
            let dest_path = dst.join(entry.file_name());
            if ty.is_dir() {
                copy_dir_all(&entry.path(), &dest_path).await?;
            } else if ty.is_symlink() {
                // Preserve symlinks as symlinks instead of following them
                let target = fs::read_link(entry.path()).await?;
                tokio::fs::symlink(target, dest_path).await?;
            } else {
                fs::copy(entry.path(), dest_path).await?;
            }
        }
        Ok(())
    })
}

/// Generate a git diff of uncommitted changes in the worktree
async fn generate_diff(worktree_path: &Path) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["-C", &worktree_path.to_string_lossy(), "diff", "HEAD"])
        .output()
        .await?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        anyhow::bail!(
            "Failed to generate diff: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

/// Extract package metadata using Nix evaluation of `entry_point` (the
/// worktree's copy of the eval entry point)
async fn extract_package_metadata(
    entry_point: &Path,
    attr_path: &str,
) -> anyhow::Result<serde_json::Value> {
    // Try to run nix-instantiate to get package metadata
    // This may fail if the package is in a broken state (or nix is not
    // available), which is fine
    let output = Command::new("nix-instantiate")
        .args([
            "--eval",
            "--json",
            "--expr",
            &format!(
                "let pkgs = import {} {{ }}; in builtins.tryEval (builtins.toJSON (pkgs.{}.meta or {{ }}))",
                crate::nix::normalize_entry_point(&entry_point.to_string_lossy()),
                attr_path
            ),
        ])
        .output()
        .await;
    let output = match output {
        Ok(output) => output,
        Err(e) => {
            return Ok(serde_json::json!({
                "error": "Failed to run nix-instantiate",
                "stderr": e.to_string()
            }));
        },
    };

    if output.status.success() {
        match serde_json::from_slice(&output.stdout) {
            Ok(json) => Ok(json),
            Err(_) => Ok(serde_json::json!({"error": "Failed to parse metadata"})),
        }
    } else {
        Ok(serde_json::json!({
            "error": "Failed to extract metadata",
            "stderr": String::from_utf8_lossy(&output.stderr).to_string()
        }))
    }
}

/// Clean up old preserved failures
pub async fn cleanup_old_failures(older_than_days: u32) -> anyhow::Result<usize> {
    let base_dirs = directories::ProjectDirs::from("com", "ekapusta", "ekapkgs-update")
        .context("Failed to determine project directories")?;
    let failed_dir = base_dirs.cache_dir().join("failed");

    if !failed_dir.exists() {
        return Ok(0);
    }

    let cutoff = Utc::now() - chrono::Duration::days(older_than_days as i64);
    let mut removed = 0;

    let mut session_entries = fs::read_dir(&failed_dir).await?;
    while let Some(session_entry) = session_entries.next_entry().await? {
        if !session_entry.file_type().await?.is_dir() {
            continue;
        }

        // Iterate through package directories in each session
        let mut package_entries = fs::read_dir(session_entry.path()).await?;
        while let Some(package_entry) = package_entries.next_entry().await? {
            if !package_entry.file_type().await?.is_dir() {
                continue;
            }

            // Check timestamp from manifest
            let manifest_path = package_entry.path().join("manifest.json");
            if manifest_path.exists() {
                let manifest_content = fs::read_to_string(&manifest_path).await?;
                if let Ok(artifacts) = serde_json::from_str::<FailureArtifacts>(&manifest_content) {
                    if artifacts.timestamp < cutoff {
                        fs::remove_dir_all(package_entry.path()).await?;
                        removed += 1;
                    }
                }
            }
        }

        // Remove session directory if empty
        let mut check_entries = fs::read_dir(session_entry.path()).await?;
        if check_entries.next_entry().await?.is_none() {
            fs::remove_dir(session_entry.path()).await?;
        }
    }

    Ok(removed)
}

/// List all preserved failures
pub async fn list_preserved_failures() -> anyhow::Result<Vec<FailureArtifacts>> {
    list_preserved_failures_in(&FailureArtifacts::failed_root()?).await
}

/// [`list_preserved_failures`] under `failed_dir` instead of the cache's `failed/`.
pub async fn list_preserved_failures_in(
    failed_dir: &Path,
) -> anyhow::Result<Vec<FailureArtifacts>> {
    if !failed_dir.exists() {
        return Ok(Vec::new());
    }

    let mut all_artifacts = Vec::new();

    let mut session_entries = fs::read_dir(failed_dir).await?;
    while let Some(session_entry) = session_entries.next_entry().await? {
        if !session_entry.file_type().await?.is_dir() {
            continue;
        }

        let mut package_entries = fs::read_dir(session_entry.path()).await?;
        while let Some(package_entry) = package_entries.next_entry().await? {
            if !package_entry.file_type().await?.is_dir() {
                continue;
            }

            let manifest_path = package_entry.path().join("manifest.json");
            if manifest_path.exists() {
                let manifest_content = fs::read_to_string(&manifest_path).await?;
                if let Ok(artifacts) = serde_json::from_str::<FailureArtifacts>(&manifest_content) {
                    all_artifacts.push(artifacts);
                }
            }
        }
    }

    // Sort by timestamp, most recent first
    all_artifacts.sort_by_key(|a| std::cmp::Reverse(a.timestamp));

    Ok(all_artifacts)
}

/// Label used on the command line for the unnamed (`''`) repo scope, the
/// same label the dashboard uses (reserved by `ConfigFile::validate`).
pub const DEFAULT_REPO_LABEL: &str = crate::config::RESERVED_REPO_NAME;

/// Map a `--repo` value to a `repo_name`: `default` selects `''`.
pub fn repo_from_label(label: &str) -> &str {
    if label == DEFAULT_REPO_LABEL {
        ""
    } else {
        label
    }
}

/// Display name of a repo scope (`''` → `default`).
pub fn repo_label(repo_name: &str) -> &str {
    if repo_name.is_empty() {
        DEFAULT_REPO_LABEL
    } else {
        repo_name
    }
}

/// Whether `artifacts` belongs to the `--repo` selection (`None` = any repo).
pub fn matches_repo(artifacts: &FailureArtifacts, repo: Option<&str>) -> bool {
    repo.is_none_or(|label| artifacts.repo_name == repo_from_label(label))
}

/// Pick the most recent preserved failure of `attr_path`.
///
/// `list` must be sorted most recent first (as [`list_preserved_failures`]
/// returns it). Without `repo`, the attr must have failed in only one
/// repository; otherwise the caller is asked to choose with `--repo` rather
/// than silently getting another repo's worktree.
pub fn find_preserved<'a>(
    list: &'a [FailureArtifacts],
    attr_path: &str,
    repo: Option<&str>,
) -> anyhow::Result<&'a FailureArtifacts> {
    let mut candidates = list
        .iter()
        .filter(|a| a.attr_path == attr_path && matches_repo(a, repo));
    let Some(first) = candidates.next() else {
        return Err(match repo {
            Some(label) => {
                anyhow::anyhow!("No preserved artifacts found for {attr_path} in repo '{label}'")
            },
            None => anyhow::anyhow!("No preserved artifacts found for {attr_path}"),
        });
    };
    if repo.is_none() {
        let mut repos: Vec<&str> = std::iter::once(first)
            .chain(candidates)
            .map(|a| repo_label(&a.repo_name))
            .collect();
        repos.sort_unstable();
        repos.dedup();
        if repos.len() > 1 {
            anyhow::bail!(
                "{attr_path} has preserved failures in several repos ({}); choose one with --repo",
                repos.join(", ")
            );
        }
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// The preserved worktree replaces the original (no leak) and stays a
    /// working git worktree with the failed update's changes.
    #[tokio::test]
    async fn relocate_moves_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "test"]);
        std::fs::write(repo.join("pkg.nix"), "v1").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let worktree = root.join("wt");
        git(
            &repo,
            &["worktree", "add", "-q", worktree.to_str().unwrap(), "HEAD"],
        );
        std::fs::write(worktree.join("pkg.nix"), "v2").unwrap();

        let preserved = root.join("failed/s/foo/worktree");
        std::fs::create_dir_all(preserved.parent().unwrap()).unwrap();
        relocate_worktree("foo", &repo, &worktree, &preserved)
            .await
            .unwrap();

        assert!(!worktree.exists());
        assert_eq!(
            git(&preserved, &["diff", "--name-only", "HEAD"]),
            "pkg.nix\n"
        );
    }

    /// A failed branch-mode single update is preserved in a worktree at
    /// `HEAD` holding exactly the paths it dirtied (edited, created and
    /// deleted), not the user's earlier uncommitted work, filed under its
    /// repo-scoped name with a manifest the lookup finds.
    #[tokio::test]
    async fn branch_single_failure_is_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("pkgs/foo")).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "test"]);
        std::fs::write(repo.join("default.nix"), "{ }").unwrap();
        std::fs::write(repo.join("pkgs/foo/default.nix"), "1").unwrap();
        std::fs::write(repo.join("pkgs/foo/old.patch"), "patch").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        // The user's work, then the failed update's edits.
        std::fs::write(repo.join("notes.txt"), "mine").unwrap();
        std::fs::write(repo.join("default.nix"), "{ user = 1; }").unwrap();
        let snapshot = DirtySnapshot::take(&repo).await.unwrap();
        std::fs::write(repo.join("pkgs/foo/default.nix"), "2").unwrap();
        std::fs::remove_file(repo.join("pkgs/foo/old.patch")).unwrap();
        std::fs::create_dir_all(repo.join("pkgs/bar")).unwrap();
        std::fs::write(repo.join("pkgs/bar/default.nix"), "new").unwrap();

        let error = UpdateError::BuildError {
            phase: UpdatePhase::Build,
            exit_code: 1,
            stderr: "boom".to_owned(),
            suspected_cause: None,
        };
        let entry = repo.join("default.nix").to_string_lossy().into_owned();
        let ctx = FailureContext {
            session_id: "s1",
            repo_name: "core",
            attr_path: "foo",
            eval_entry_point: &entry,
            error: &error,
            error_log: "build log",
        };
        let failed_root = root.join("failed");
        let artifacts =
            preserve_branch_failure_in(&failed_root, &ctx, &BranchChanges::Single(&snapshot))
                .await
                .unwrap();

        let preserved = &artifacts.worktree_path;
        assert_eq!(preserved, &failed_root.join("s1/core+foo/worktree"));
        assert_eq!(git(preserved, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            git(
                preserved,
                &["status", "--porcelain=v1", "--untracked-files=all"]
            ),
            " M pkgs/foo/default.nix\n D pkgs/foo/old.patch\n?? pkgs/bar/default.nix\n"
        );
        assert_eq!(
            std::fs::read_to_string(preserved.join("default.nix")).unwrap(),
            "{ }"
        );
        // The checkout itself is left to the caller's revert.
        assert_eq!(
            std::fs::read_to_string(repo.join("pkgs/foo/default.nix")).unwrap(),
            "2"
        );

        let list = list_preserved_failures_in(&failed_root).await.unwrap();
        let found = find_preserved(&list, "foo", Some("core")).unwrap();
        assert_eq!(found.worktree_path, *preserved);
        assert_eq!(found.failed_phase, UpdatePhase::Build);
        assert_eq!(
            std::fs::read_to_string(found.build_log_path.as_ref().unwrap()).unwrap(),
            "build log"
        );
        let diff = std::fs::read_to_string(&found.diff_path).unwrap();
        assert!(diff.contains("pkgs/foo/default.nix"), "{diff}");
        assert!(!diff.contains("user = 1"), "{diff}");
    }

    fn artifact(repo: &str, attr: &str, session: &str) -> FailureArtifacts {
        FailureArtifacts {
            session_id: session.to_owned(),
            repo_name: repo.to_owned(),
            attr_path: attr.to_owned(),
            failed_phase: UpdatePhase::Build,
            timestamp: Utc::now(),
            worktree_path: PathBuf::from("/w"),
            build_log_path: None,
            test_output_path: None,
            diff_path: PathBuf::from("/d"),
            metadata_path: PathBuf::from("/m"),
            error_context_path: PathBuf::from("/e"),
        }
    }

    #[test]
    fn find_preserved_by_repo() {
        // Most recent first.
        let list = vec![
            artifact("", "foo", "new"),
            artifact("", "foo", "old"),
            artifact("core", "bar", "b1"),
            artifact("", "bar", "b2"),
        ];

        // Unambiguous: most recent wins.
        assert_eq!(
            find_preserved(&list, "foo", None).unwrap().session_id,
            "new"
        );
        // `default` selects the unnamed scope.
        assert_eq!(
            find_preserved(&list, "foo", Some("default"))
                .unwrap()
                .session_id,
            "new"
        );
        assert!(find_preserved(&list, "foo", Some("core")).is_err());

        // Ambiguous without --repo.
        let err = find_preserved(&list, "bar", None).unwrap_err().to_string();
        assert!(err.contains("core, default"), "{err}");
        assert_eq!(
            find_preserved(&list, "bar", Some("core"))
                .unwrap()
                .session_id,
            "b1"
        );
        assert_eq!(
            find_preserved(&list, "bar", Some("default"))
                .unwrap()
                .session_id,
            "b2"
        );

        assert!(find_preserved(&list, "missing", None).is_err());
    }

    #[test]
    fn package_dir_is_the_per_package_directory() {
        let mut a = artifact("core", "foo", "s1");
        a.worktree_path = PathBuf::from("/cache/failed/s1/core-foo/worktree");
        assert_eq!(a.package_dir(), Path::new("/cache/failed/s1/core-foo"));

        // Two failures in one session must not share a directory.
        let mut b = artifact("core", "bar", "s1");
        b.worktree_path = PathBuf::from("/cache/failed/s1/core-bar/worktree");
        assert_ne!(a.package_dir(), b.package_dir());
    }

    #[test]
    fn package_dir_is_repo_qualified() {
        assert_eq!(
            package_dir_name("", "python3Packages.foo"),
            "python3Packages_foo"
        );
        assert_eq!(
            package_dir_name("core", "python3Packages.foo"),
            "core+python3Packages_foo"
        );
        assert_ne!(
            package_dir_name("core", "foo"),
            package_dir_name("extra", "foo")
        );
    }

    #[test]
    fn repo_scoped_names_do_not_collide_on_dashes() {
        // repo `a` + attr `b-c` and repo `a-b` + attr `c` used to both map
        // to `a-b-c`.
        assert_eq!(repo_scoped_name("a", "b-c"), "a+b-c");
        assert_eq!(repo_scoped_name("a-b", "c"), "a-b+c");
        assert_eq!(repo_scoped_name("", "b-c"), "b-c");
        assert_ne!(package_dir_name("a", "b-c"), package_dir_name("a-b", "c"));
    }

    #[test]
    fn old_manifest_defaults_to_unscoped_repo() {
        let manifest = r#"{
            "session_id": "s", "attr_path": "foo", "failed_phase": "build",
            "timestamp": "2026-01-01T00:00:00Z", "worktree_path": "/w",
            "build_log_path": null, "test_output_path": null, "diff_path": "/d",
            "metadata_path": "/m", "error_context_path": "/e"
        }"#;
        let artifacts: FailureArtifacts = serde_json::from_str(manifest).unwrap();
        assert_eq!(artifacts.repo_name, "");
    }
}
