//! Git bookkeeping for branch-mode (`--commit-strategy branch`) updates.
//!
//! An update — and especially a `--claude-fix` attempt — may touch files
//! outside the package directory: a new dependency package, an override in a
//! shared file, a fix to a sibling package. Scoping `git add` / `git checkout`
//! to the package directory alone leaves those edits uncommitted on success
//! (so the commit does not evaluate on its own) and un-reverted on failure (so
//! they leak into every later commit's working tree).
//!
//! Instead, the dirty paths are snapshotted before the update starts. Anything
//! that becomes dirty afterwards is attributed to the update: staged with its
//! commit on success, restored or deleted on failure. Paths that were already
//! dirty beforehand (e.g. the user's own uncommitted work) are left alone.
//!
//! Staging always includes the package directory, preserving the previous
//! behaviour for files there that were dirty before the update. Reverting
//! never touches a path that was dirty before the update, inside the package
//! directory or not: its index and worktree state are left exactly as the
//! update left them. If the update also modified such a path, its changes
//! cannot be separated from the user's, so the path is left as is and a
//! warning is logged.
//!
//! Attribution is exact only because branch mode never runs two updates at
//! once (`config::updater_concurrency` clamps it to 1). Otherwise a concurrent
//! update's paths would be committed or reverted by whichever update finished
//! first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use tokio::process::Command;
use tracing::{debug, warn};

/// A path reported by `git status --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct DirtyEntry {
    /// Path relative to the repository root.
    pub path: String,
    /// The path does not exist in `HEAD` (untracked, or newly added to the index).
    pub new_file: bool,
    /// The deletion is already staged (`git rm`, e.g. by a Claude fix): the path
    /// is in neither the index nor the worktree, so `git add` must not be given
    /// it (an unmatched pathspec aborts the whole `git add`).
    pub staged_deletion: bool,
}

/// Snapshot of the working tree taken before an update.
#[derive(Debug, Clone)]
pub(super) struct DirtySnapshot {
    root: PathBuf,
    /// Paths dirty at snapshot time, with a cheap fingerprint of their
    /// worktree state used to detect whether the update also touched them.
    baseline: HashMap<String, Option<Fingerprint>>,
}

/// Size and modification time of a worktree path (`None` if it is absent).
type Fingerprint = (u64, Option<std::time::SystemTime>);

fn fingerprint(path: &Path) -> Option<Fingerprint> {
    std::fs::symlink_metadata(path)
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}

/// Parse `git status --porcelain=v1 -z --untracked-files=all` output.
pub(super) fn parse_porcelain_z(output: &[u8]) -> Vec<DirtyEntry> {
    let text = String::from_utf8_lossy(output);
    let mut tokens = text.split('\0').filter(|t| !t.is_empty());
    let mut entries = Vec::new();

    while let Some(token) = tokens.next() {
        if token.len() < 4 {
            continue;
        }
        let (status, path) = token.split_at(3);
        let mut chars = status.chars();
        let index = chars.next().unwrap_or(' ');
        let worktree = chars.next().unwrap_or(' ');

        let new_file = (index == '?' && worktree == '?') || matches!(index, 'A' | 'R' | 'C');
        entries.push(DirtyEntry {
            path: path.to_owned(),
            new_file,
            staged_deletion: index == 'D',
        });

        // Renames and copies are followed by the original path.
        if matches!(index, 'R' | 'C') {
            if let Some(orig) = tokens.next() {
                if index == 'R' {
                    // The rename already stages the original path's removal.
                    entries.push(DirtyEntry {
                        path: orig.to_owned(),
                        new_file: false,
                        staged_deletion: true,
                    });
                }
            }
        }
    }

    entries
}

async fn git(root: &Path, args: &[&str]) -> anyhow::Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .await
        .with_context(|| format!("Failed to run git {}", args.join(" ")))
}

async fn dirty_entries(root: &Path) -> anyhow::Result<Vec<DirtyEntry>> {
    let output = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(parse_porcelain_z(&output.stdout))
}

impl DirtySnapshot {
    /// Record the currently dirty paths of the repository containing `dir`.
    pub(super) async fn take(dir: &Path) -> anyhow::Result<Self> {
        let output = git(dir, &["rev-parse", "--show-toplevel"]).await?;
        if !output.status.success() {
            anyhow::bail!(
                "Not a git repository ({}): {}",
                dir.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let baseline = dirty_entries(&root)
            .await?
            .into_iter()
            .map(|e| {
                let fp = fingerprint(&root.join(&e.path));
                (e.path, fp)
            })
            .collect();
        Ok(Self { root, baseline })
    }

    /// Paths that became dirty since the snapshot was taken.
    async fn introduced(&self) -> anyhow::Result<Vec<DirtyEntry>> {
        Ok(dirty_entries(&self.root)
            .await?
            .into_iter()
            .filter(|e| !self.baseline.contains_key(&e.path))
            .collect())
    }

    /// Stage the package directory plus every path dirtied since the snapshot.
    ///
    /// Deletions that are already staged are left out of `git add`: the path
    /// no longer matches anything, and git would reject the whole command.
    ///
    /// # Errors
    /// If `git add` fails. The index may then hold only part of the update
    /// (whatever the update staged itself), so the caller must not commit it.
    pub(super) async fn stage(&self, attr_path: &str, pkg_dir: &str) -> anyhow::Result<()> {
        self.stage_paths(attr_path, Some(pkg_dir)).await
    }

    /// [`Self::stage`], with the package directory optional.
    async fn stage_paths(&self, attr_path: &str, pkg_dir: Option<&str>) -> anyhow::Result<()> {
        let introduced = match self.introduced().await {
            Ok(entries) => entries,
            Err(e) => {
                warn!(
                    "{}: {:#}; staging only {}",
                    attr_path,
                    e,
                    pkg_dir.unwrap_or("the package directory")
                );
                Vec::new()
            },
        };

        let extra: Vec<&str> = introduced
            .iter()
            .filter(|e| !e.staged_deletion)
            .map(|e| e.path.as_str())
            .collect();
        if !extra.is_empty() {
            debug!("{}: Staging changed paths: {}", attr_path, extra.join(", "));
        }

        let mut args = vec!["add", "-A", "--"];
        // A package directory the update removed entirely has nothing left
        // to add; its removal is staged or listed in `extra`.
        if let Some(pkg_dir) = pkg_dir.filter(|d| Path::new(d).exists()) {
            args.push(pkg_dir);
        }
        args.extend(extra);
        if args.len() == 3 {
            return Ok(());
        }
        let output = git(&self.root, &args).await?;
        if !output.status.success() {
            anyhow::bail!(
                "git add failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// Restore every path dirtied since the snapshot (in `pkg_dir` or
    /// elsewhere), deleting files that did not exist before. Paths that were
    /// already dirty at snapshot time are never touched; if the update changed
    /// one of them too, a warning is logged and it is left as is.
    pub(super) async fn revert(&self, attr_path: &str, pkg_dir: &str) {
        let introduced = match self.introduced().await {
            Ok(entries) => entries,
            Err(e) => {
                warn!(
                    "{}: {:#}; not reverting, {} may hold partial changes",
                    attr_path, e, pkg_dir
                );
                return;
            },
        };

        for (path, before) in &self.baseline {
            if fingerprint(&self.root.join(path)) != *before {
                warn!(
                    "{}: {} had uncommitted changes before the update and was \
                     modified again; leaving it as is",
                    attr_path, path
                );
            }
        }

        let (new_files, modified): (Vec<_>, Vec<_>) =
            introduced.into_iter().partition(|e| e.new_file);

        // Restore modified/deleted tracked files to HEAD (index and worktree).
        // They were clean at snapshot time, so HEAD is their prior state.
        if !modified.is_empty() {
            let mut args = vec!["checkout", "HEAD", "--"];
            args.extend(modified.iter().map(|e| e.path.as_str()));
            match git(&self.root, &args).await {
                Ok(output) if !output.status.success() => {
                    warn!(
                        "{}: Failed to revert changes: {}",
                        attr_path,
                        String::from_utf8_lossy(&output.stderr)
                    );
                },
                Err(e) => warn!("{}: Failed to run git checkout: {:#}", attr_path, e),
                Ok(_) => {},
            }
        }

        // Delete files the update created, then prune directories it left empty.
        for entry in &new_files {
            let _ = git(
                &self.root,
                &[
                    "rm",
                    "-q",
                    "--cached",
                    "--ignore-unmatch",
                    "--",
                    &entry.path,
                ],
            )
            .await;
            let full = self.root.join(&entry.path);
            if let Err(e) = tokio::fs::remove_file(&full).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!("{}: Failed to remove {}: {}", attr_path, full.display(), e);
                }
            }
            let mut dir = full.parent();
            while let Some(d) = dir {
                if d == self.root || tokio::fs::remove_dir(d).await.is_err() {
                    break;
                }
                dir = d.parent();
            }
        }

        if !new_files.is_empty() || !modified.is_empty() {
            debug!(
                "{}: Reverted {} modified and removed {} new path(s)",
                attr_path,
                modified.len(),
                new_files.len()
            );
        }
    }
}

/// Where a branch-mode update group started: the commit, branch and dirty
/// paths before its first member, plus the commits the group created.
///
/// Members are committed one by one on the current branch. If the group
/// fails, [`GroupCheckpoint::undo`] removes its commits by resetting the
/// branch to the start commit and reverts every path the group dirtied.
/// That is only safe because branch mode runs one update at a time and every
/// git operation holds the git mutex: no other commit can land in between.
/// It is still checked ([`plan_undo`]) before anything is reset.
#[derive(Debug)]
pub(super) struct GroupCheckpoint {
    snapshot: DirtySnapshot,
    /// `HEAD` before the group.
    start: String,
    /// Branch checked out before the group (`None`: detached `HEAD`).
    branch: Option<String>,
    /// Commits made by [`GroupCheckpoint::commit`], oldest first.
    created: std::sync::Mutex<Vec<String>>,
}

/// What undoing a failed group may do, given the repository's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UndoPlan {
    /// Nothing was committed: revert the paths the group dirtied.
    Revert,
    /// Reset the branch to the start commit (dropping the group's commits),
    /// then revert the paths the group dirtied.
    ResetAndRevert,
    /// The branch moved in a way the group does not account for; nothing is
    /// touched.
    Refuse(String),
}

/// Decide how to undo a failed group.
///
/// `since_start` is `git rev-list <start>..HEAD` (newest first), `None` when
/// `start` is no longer an ancestor of `HEAD`. The branch is only reset when
/// it is still the one the group started on and the commits on top of
/// `start` are exactly the ones the group created (`created`, oldest
/// first), so a reset can only ever drop the group's own commits.
pub(super) fn plan_undo(
    start: &str,
    created: &[String],
    branch_then: Option<&str>,
    branch_now: Option<&str>,
    since_start: Option<&[String]>,
) -> UndoPlan {
    if branch_then != branch_now {
        return UndoPlan::Refuse(format!(
            "the checked out branch changed from {} to {}",
            branch_then.unwrap_or("(detached HEAD)"),
            branch_now.unwrap_or("(detached HEAD)")
        ));
    }
    let Some(since_start) = since_start else {
        return UndoPlan::Refuse(format!("HEAD no longer descends from {start}"));
    };
    if !since_start.iter().rev().eq(created.iter()) {
        return UndoPlan::Refuse(format!(
            "HEAD has {} commit(s) on top of {start}, but the group created {}: {}",
            since_start.len(),
            created.len(),
            if created.is_empty() {
                "none".to_owned()
            } else {
                created.join(", ")
            }
        ));
    }
    if created.is_empty() {
        UndoPlan::Revert
    } else {
        UndoPlan::ResetAndRevert
    }
}

/// Run git in `root` and return its trimmed stdout, failing on a non-zero exit.
async fn git_stdout(root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = git(root, args).await?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The checked out branch, `None` for a detached `HEAD`.
async fn current_branch(root: &Path) -> anyhow::Result<Option<String>> {
    let output = git(root, &["symbolic-ref", "--quiet", "HEAD"]).await?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

impl GroupCheckpoint {
    /// Record the state of the repository containing `dir`. Must be called
    /// with the git mutex held.
    pub(super) async fn take(dir: &Path) -> anyhow::Result<Self> {
        let snapshot = DirtySnapshot::take(dir).await?;
        let start = git_stdout(&snapshot.root, &["rev-parse", "HEAD"]).await?;
        let branch = current_branch(&snapshot.root).await?;
        Ok(Self {
            snapshot,
            start,
            branch,
            created: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Commits the group created so far, oldest first.
    pub(super) fn created(&self) -> Vec<String> {
        self.created
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Commit every path dirtied since the checkpoint (and `pkg_dir`) that
    /// the group has not committed yet. Must be called with the git mutex
    /// held. Returns `false` (without committing) when nothing changed.
    ///
    /// # Errors
    /// If staging or the commit fails; the group must then be undone.
    pub(super) async fn commit(
        &self,
        label: &str,
        pkg_dir: Option<&str>,
        message: &str,
    ) -> anyhow::Result<bool> {
        let root = &self.snapshot.root;
        self.snapshot.stage_paths(label, pkg_dir).await?;
        let staged = git(root, &["diff", "--cached", "--quiet"]).await?;
        if staged.status.success() {
            return Ok(false);
        }
        git_stdout(root, &["commit", "-q", "-m", message]).await?;
        let head = git_stdout(root, &["rev-parse", "HEAD"]).await?;
        self.created
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(head);
        Ok(true)
    }

    /// Undo a failed group: drop its commits and revert every path it
    /// dirtied, keeping paths that were dirty before it started. Must be
    /// called with the git mutex held.
    ///
    /// The branch is reset with `git reset --soft`, so the index and working
    /// tree keep their content (including user changes that a member's
    /// commit swept in from its package directory); the paths the group
    /// introduced are then restored like a failed branch-mode update's.
    ///
    /// # Errors
    /// If the branch moved in a way the group does not account for
    /// ([`plan_undo`]) or git fails; nothing is reset then.
    pub(super) async fn undo(&self, label: &str) -> anyhow::Result<()> {
        let root = &self.snapshot.root;
        let created = self.created();
        let branch_now = current_branch(root).await?;
        let is_ancestor = git(root, &["merge-base", "--is-ancestor", &self.start, "HEAD"])
            .await?
            .status
            .success();
        let since_start: Option<Vec<String>> = if is_ancestor {
            let range = format!("{}..HEAD", self.start);
            let listed = git_stdout(root, &["rev-list", &range]).await?;
            Some(listed.lines().map(str::to_owned).collect())
        } else {
            None
        };
        match plan_undo(
            &self.start,
            &created,
            self.branch.as_deref(),
            branch_now.as_deref(),
            since_start.as_deref(),
        ) {
            UndoPlan::Refuse(reason) => {
                anyhow::bail!("refusing to undo the group's changes: {reason}")
            },
            UndoPlan::ResetAndRevert => {
                git_stdout(root, &["reset", "-q", "--soft", &self.start]).await?;
                debug!(
                    "{}: reset to {} (dropped {} commit(s))",
                    label,
                    self.start,
                    created.len()
                );
                self.created
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clear();
            },
            UndoPlan::Revert => {},
        }
        self.snapshot.revert(label, &root.to_string_lossy()).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, new_file: bool) -> DirtyEntry {
        DirtyEntry {
            path: path.to_owned(),
            new_file,
            staged_deletion: false,
        }
    }

    fn staged_deletion(path: &str) -> DirtyEntry {
        DirtyEntry {
            path: path.to_owned(),
            new_file: false,
            staged_deletion: true,
        }
    }

    #[test]
    fn test_parse_porcelain_z() {
        let cases: &[(&[u8], Vec<DirtyEntry>)] = &[
            (b"", vec![]),
            (b" M flake.lock\0", vec![entry("flake.lock", false)]),
            (
                b"?? python/pkgs/jinxed/default.nix\0",
                vec![entry("python/pkgs/jinxed/default.nix", true)],
            ),
            (b"A  new.nix\0", vec![entry("new.nix", true)]),
            (b" D gone.nix\0", vec![entry("gone.nix", false)]),
            (b"D  removed.nix\0", vec![staged_deletion("removed.nix")]),
            (b"MM both.nix\0", vec![entry("both.nix", false)]),
            (
                b"R  new name.nix\0old name.nix\0",
                vec![entry("new name.nix", true), staged_deletion("old name.nix")],
            ),
            (b"C  copy.nix\0orig.nix\0", vec![entry("copy.nix", true)]),
            (
                b" M a.nix\0?? b/c.nix\0",
                vec![entry("a.nix", false), entry("b/c.nix", true)],
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(
                &parse_porcelain_z(input),
                expected,
                "input: {:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    async fn sh(root: &Path, args: &[&str]) {
        let out = git(root, args).await.expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    async fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        sh(root, &["init", "-q"]).await;
        sh(root, &["config", "user.email", "test@example.com"]).await;
        sh(root, &["config", "user.name", "test"]).await;
        std::fs::create_dir_all(root.join("pkgs/foo")).unwrap();
        std::fs::write(root.join("pkgs/foo/default.nix"), "v1").unwrap();
        std::fs::write(root.join("shared.nix"), "orig").unwrap();
        std::fs::write(root.join("user.lock"), "orig").unwrap();
        sh(root, &["add", "-A"]).await;
        sh(root, &["commit", "-q", "-m", "init"]).await;
        // Pre-existing user work that must never be touched.
        std::fs::write(root.join("user.lock"), "user edit").unwrap();
        std::fs::write(root.join("user-notes.txt"), "mine").unwrap();
        dir
    }

    /// Simulate an update plus a Claude fix that edits outside the package dir.
    fn simulate_update(root: &Path) {
        std::fs::write(root.join("pkgs/foo/default.nix"), "v2").unwrap();
        std::fs::write(root.join("shared.nix"), "fixed").unwrap();
        std::fs::create_dir_all(root.join("pkgs/newdep")).unwrap();
        std::fs::write(root.join("pkgs/newdep/default.nix"), "new").unwrap();
    }

    #[tokio::test]
    async fn test_revert_restores_and_removes_only_introduced_paths() {
        let dir = init_repo().await;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let pkg_dir = root.join("pkgs/foo");
        let snapshot = DirtySnapshot::take(&pkg_dir).await.unwrap();
        simulate_update(&root);

        snapshot.revert("foo", &pkg_dir.to_string_lossy()).await;

        let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
        assert_eq!(read("pkgs/foo/default.nix"), "v1");
        assert_eq!(read("shared.nix"), "orig");
        assert!(!root.join("pkgs/newdep").exists());
        assert_eq!(read("user.lock"), "user edit");
        assert_eq!(read("user-notes.txt"), "mine");
    }

    async fn status_of(root: &Path, path: &str) -> String {
        let out = git(root, &["status", "--porcelain=v1", "--", path])
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
    }

    #[tokio::test]
    async fn test_revert_keeps_preexisting_changes_in_pkg_dir() {
        let dir = init_repo().await;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let pkg_dir = root.join("pkgs/foo");
        let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();

        // Committed files in the package dir, then the user's own work there.
        std::fs::write(pkg_dir.join("staged.nix"), "orig").unwrap();
        std::fs::write(pkg_dir.join("unstaged.nix"), "orig").unwrap();
        std::fs::write(pkg_dir.join("clean.nix"), "orig").unwrap();
        sh(&root, &["add", "-A", "pkgs/foo"]).await;
        sh(&root, &["commit", "-q", "-m", "more"]).await;
        std::fs::write(pkg_dir.join("staged.nix"), "user staged").unwrap();
        sh(&root, &["add", "pkgs/foo/staged.nix"]).await;
        std::fs::write(pkg_dir.join("unstaged.nix"), "user unstaged").unwrap();
        std::fs::write(pkg_dir.join("user-untracked.txt"), "mine").unwrap();

        let snapshot = DirtySnapshot::take(&pkg_dir).await.unwrap();
        // The update modifies a clean file and creates a new one.
        std::fs::write(pkg_dir.join("clean.nix"), "updated").unwrap();
        std::fs::write(pkg_dir.join("created.nix"), "new").unwrap();

        snapshot.revert("foo", &pkg_dir.to_string_lossy()).await;

        assert_eq!(read("pkgs/foo/staged.nix"), "user staged");
        assert_eq!(
            status_of(&root, "pkgs/foo/staged.nix").await,
            "M  pkgs/foo/staged.nix"
        );
        assert_eq!(read("pkgs/foo/unstaged.nix"), "user unstaged");
        assert_eq!(
            status_of(&root, "pkgs/foo/unstaged.nix").await,
            " M pkgs/foo/unstaged.nix"
        );
        assert_eq!(read("pkgs/foo/user-untracked.txt"), "mine");
        assert_eq!(read("pkgs/foo/clean.nix"), "orig");
        assert_eq!(status_of(&root, "pkgs/foo/clean.nix").await, "");
        assert!(!pkg_dir.join("created.nix").exists());
    }

    #[tokio::test]
    async fn test_stage_includes_introduced_paths_only() {
        let dir = init_repo().await;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let pkg_dir = root.join("pkgs/foo");
        let snapshot = DirtySnapshot::take(&pkg_dir).await.unwrap();
        simulate_update(&root);

        snapshot
            .stage("foo", &pkg_dir.to_string_lossy())
            .await
            .unwrap();

        let out = git(&root, &["diff", "--cached", "--name-only"])
            .await
            .unwrap();
        let mut staged: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        staged.sort();
        assert_eq!(
            staged,
            [
                "pkgs/foo/default.nix",
                "pkgs/newdep/default.nix",
                "shared.nix"
            ]
        );
    }

    /// A Claude fix that `git rm`s a file (blake3's Cargo.lock) next to the
    /// version bump: the whole update is staged, not just the deletion.
    #[tokio::test]
    async fn test_stage_with_deletion_already_staged() {
        let dir = init_repo().await;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let pkg_dir = root.join("pkgs/foo");
        std::fs::write(pkg_dir.join("Cargo.lock"), "lock").unwrap();
        sh(&root, &["add", "pkgs/foo/Cargo.lock"]).await;
        sh(&root, &["commit", "-q", "-m", "lock"]).await;

        let snapshot = DirtySnapshot::take(&pkg_dir).await.unwrap();
        std::fs::write(pkg_dir.join("default.nix"), "v2").unwrap();
        std::fs::write(root.join("shared.nix"), "fixed").unwrap();
        sh(&root, &["rm", "-q", "pkgs/foo/Cargo.lock"]).await;

        snapshot
            .stage("foo", &pkg_dir.to_string_lossy())
            .await
            .unwrap();

        let out = git(&root, &["diff", "--cached", "--name-status"])
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "D\tpkgs/foo/Cargo.lock\nM\tpkgs/foo/default.nix\nM\tshared.nix\n"
        );
    }

    #[tokio::test]
    async fn test_stage_reports_git_add_failure() {
        let dir = init_repo().await;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let pkg_dir = root.join("pkgs/foo");
        let snapshot = DirtySnapshot::take(&pkg_dir).await.unwrap();
        simulate_update(&root);
        // Another git process holds the index.
        std::fs::write(root.join(".git/index.lock"), "").unwrap();

        let err = snapshot
            .stage("foo", &pkg_dir.to_string_lossy())
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("git add failed"), "{err:#}");
    }

    #[tokio::test]
    async fn test_revert_restores_staged_deletion() {
        let dir = init_repo().await;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let pkg_dir = root.join("pkgs/foo");
        std::fs::write(pkg_dir.join("Cargo.lock"), "lock").unwrap();
        sh(&root, &["add", "pkgs/foo/Cargo.lock"]).await;
        sh(&root, &["commit", "-q", "-m", "lock"]).await;

        let snapshot = DirtySnapshot::take(&pkg_dir).await.unwrap();
        std::fs::write(pkg_dir.join("default.nix"), "v2").unwrap();
        sh(&root, &["rm", "-q", "pkgs/foo/Cargo.lock"]).await;

        snapshot.revert("foo", &pkg_dir.to_string_lossy()).await;

        let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
        assert_eq!(read("pkgs/foo/Cargo.lock"), "lock");
        assert_eq!(read("pkgs/foo/default.nix"), "v1");
        assert_eq!(status_of(&root, "pkgs/foo").await, "");
    }

    #[test]
    fn test_plan_undo() {
        let ids = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let main = Some("refs/heads/main");
        let created = ids(&["c1", "c2"]);
        // `since_start` is newest first, `created` oldest first.
        let ours = ids(&["c2", "c1"]);
        let foreign = ids(&["x", "c2", "c1"]);
        type Case<'a> = (&'a [String], Option<&'a str>, Option<&'a [String]>, &'a str);
        let cases: Vec<Case<'_>> = vec![
            (&created, main, Some(&ours), "reset"),
            (&[], main, Some(&[]), "revert"),
            (&created, main, Some(&foreign), "refuse"),
            (&created, main, Some(&ours[..1]), "refuse"),
            (&[], main, Some(&ours), "refuse"),
            (&created, main, None, "refuse"),
            (&created, Some("refs/heads/other"), Some(&ours), "refuse"),
            (&created, None, Some(&ours), "refuse"),
        ];
        for (created, branch_now, since_start, expected) in cases {
            let plan = plan_undo("s", created, main, branch_now, since_start);
            let got = match plan {
                UndoPlan::ResetAndRevert => "reset",
                UndoPlan::Revert => "revert",
                UndoPlan::Refuse(_) => "refuse",
            };
            assert_eq!(got, expected, "{created:?} {branch_now:?} {since_start:?}");
        }
        // Detached HEAD that stayed detached.
        assert_eq!(
            plan_undo("s", &created, None, None, Some(&ours)),
            UndoPlan::ResetAndRevert
        );
    }

    async fn head(root: &Path) -> String {
        git_stdout(root, &["rev-parse", "HEAD"]).await.unwrap()
    }

    /// Two members committed, then the group fails: both commits are
    /// dropped and every path the group touched is restored, while the
    /// user's own uncommitted work stays.
    #[tokio::test]
    async fn test_group_undo_drops_commits_and_reverts() {
        let dir = init_repo().await;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let start = head(&root).await;
        let checkpoint = GroupCheckpoint::take(&root).await.unwrap();
        let pkg = |p: &str| root.join(p).to_string_lossy().into_owned();
        let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();

        std::fs::write(root.join("pkgs/foo/default.nix"), "v2").unwrap();
        assert!(
            checkpoint
                .commit("foo", Some(&pkg("pkgs/foo")), "foo: 1 -> 2")
                .await
                .unwrap()
        );
        std::fs::create_dir_all(root.join("pkgs/bar")).unwrap();
        std::fs::write(root.join("pkgs/bar/default.nix"), "new").unwrap();
        std::fs::write(root.join("shared.nix"), "fixed").unwrap();
        assert!(
            checkpoint
                .commit("bar", Some(&pkg("pkgs/bar")), "bar: 1 -> 2")
                .await
                .unwrap()
        );
        // Nothing left: no empty commit.
        assert!(!checkpoint.commit("baz", None, "baz").await.unwrap());
        assert_eq!(checkpoint.created().len(), 2);
        // The user's work was never committed.
        let committed = git_stdout(
            &root,
            &["log", "--name-only", "--format=", &format!("{start}..")],
        )
        .await
        .unwrap();
        assert!(!committed.contains("user"), "{committed}");
        // A failing member's uncommitted edit.
        std::fs::write(root.join("pkgs/foo/default.nix"), "v3").unwrap();

        checkpoint.undo("group").await.unwrap();

        assert_eq!(head(&root).await, start);
        assert_eq!(read("pkgs/foo/default.nix"), "v1");
        assert_eq!(read("shared.nix"), "orig");
        assert!(!root.join("pkgs/bar").exists());
        assert_eq!(read("user.lock"), "user edit");
        assert_eq!(read("user-notes.txt"), "mine");
        assert_eq!(
            git_stdout(&root, &["status", "--porcelain=v1"])
                .await
                .unwrap(),
            "M user.lock\n?? user-notes.txt"
        );
        assert!(checkpoint.created().is_empty());
    }

    /// A commit the group did not make on top of its own: nothing is reset.
    #[tokio::test]
    async fn test_group_undo_refuses_foreign_commits() {
        let dir = init_repo().await;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let checkpoint = GroupCheckpoint::take(&root).await.unwrap();

        std::fs::write(root.join("pkgs/foo/default.nix"), "v2").unwrap();
        checkpoint.commit("foo", None, "foo: 1 -> 2").await.unwrap();
        std::fs::write(root.join("shared.nix"), "someone else").unwrap();
        sh(&root, &["commit", "-q", "-m", "foreign", "shared.nix"]).await;
        let foreign = head(&root).await;

        let err = checkpoint.undo("group").await.unwrap_err();
        assert!(format!("{err:#}").contains("refusing"), "{err:#}");
        assert_eq!(head(&root).await, foreign);
        assert_eq!(
            std::fs::read_to_string(root.join("pkgs/foo/default.nix")).unwrap(),
            "v2"
        );
    }

    /// Another branch checked out: refused even though the group committed.
    #[tokio::test]
    async fn test_group_undo_refuses_other_branch() {
        let dir = init_repo().await;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let checkpoint = GroupCheckpoint::take(&root).await.unwrap();
        std::fs::write(root.join("pkgs/foo/default.nix"), "v2").unwrap();
        checkpoint.commit("foo", None, "foo").await.unwrap();
        let ours = head(&root).await;
        sh(&root, &["checkout", "-q", "-b", "other"]).await;

        let err = checkpoint.undo("group").await.unwrap_err();
        assert!(format!("{err:#}").contains("branch changed"), "{err:#}");
        assert_eq!(head(&root).await, ours);
    }

    /// A member failed before anything was committed: only its edits go.
    #[tokio::test]
    async fn test_group_undo_without_commits_reverts() {
        let dir = init_repo().await;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let start = head(&root).await;
        let checkpoint = GroupCheckpoint::take(&root).await.unwrap();
        simulate_update(&root);

        checkpoint.undo("group").await.unwrap();

        assert_eq!(head(&root).await, start);
        assert_eq!(
            std::fs::read_to_string(root.join("pkgs/foo/default.nix")).unwrap(),
            "v1"
        );
        assert!(!root.join("pkgs/newdep").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("user.lock")).unwrap(),
            "user edit"
        );
    }
}
