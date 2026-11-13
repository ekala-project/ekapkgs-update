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
        });

        // Renames and copies are followed by the original path.
        if matches!(index, 'R' | 'C') {
            if let Some(orig) = tokens.next() {
                if index == 'R' {
                    entries.push(DirtyEntry {
                        path: orig.to_owned(),
                        new_file: false,
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
    pub(super) async fn stage(&self, attr_path: &str, pkg_dir: &str) {
        let introduced = match self.introduced().await {
            Ok(entries) => entries,
            Err(e) => {
                warn!("{}: {:#}; staging only {}", attr_path, e, pkg_dir);
                Vec::new()
            },
        };

        let extra: Vec<&str> = introduced.iter().map(|e| e.path.as_str()).collect();
        if !extra.is_empty() {
            debug!("{}: Staging changed paths: {}", attr_path, extra.join(", "));
        }

        let mut args = vec!["add", "-A", "--", pkg_dir];
        args.extend(extra);
        match git(&self.root, &args).await {
            Ok(output) if !output.status.success() => {
                warn!(
                    "{}: git add failed: {}",
                    attr_path,
                    String::from_utf8_lossy(&output.stderr)
                );
            },
            Err(e) => warn!("{}: Failed to stage changes: {:#}", attr_path, e),
            Ok(_) => {},
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, new_file: bool) -> DirtyEntry {
        DirtyEntry {
            path: path.to_owned(),
            new_file,
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
            (b"MM both.nix\0", vec![entry("both.nix", false)]),
            (
                b"R  new name.nix\0old name.nix\0",
                vec![entry("new name.nix", true), entry("old name.nix", false)],
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

        snapshot.stage("foo", &pkg_dir.to_string_lossy()).await;

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
}
