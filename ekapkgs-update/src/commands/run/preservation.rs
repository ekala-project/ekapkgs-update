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
use tracing::{debug, info};

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
    pub fn base_path(
        session_id: &str,
        repo_name: &str,
        attr_path: &str,
    ) -> anyhow::Result<PathBuf> {
        let base_dirs = directories::ProjectDirs::from("com", "ekapusta", "ekapkgs-update")
            .context("Failed to determine project directories")?;
        let cache_dir = base_dirs.cache_dir();

        let path = cache_dir
            .join("failed")
            .join(session_id)
            .join(package_dir_name(repo_name, attr_path));
        Ok(path)
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

/// Preserve a failed update's context for later inspection
pub async fn preserve_failure(
    session_id: &str,
    repo_name: &str,
    attr_path: &str,
    phase: UpdatePhase,
    worktree_path: &Path,
    error: &UpdateError,
    build_log: Option<String>,
    test_output: Option<String>,
) -> anyhow::Result<FailureArtifacts> {
    let base_path = FailureArtifacts::base_path(session_id, repo_name, attr_path)?;
    fs::create_dir_all(&base_path).await?;

    info!(
        "{}: Preserving failure artifacts to {}",
        attr_path,
        base_path.display()
    );

    // Copy worktree (entire directory)
    let preserved_worktree = base_path.join("worktree");
    copy_dir_all(worktree_path, &preserved_worktree).await?;
    debug!("{}: Worktree copied", attr_path);

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
    let metadata = extract_package_metadata(worktree_path, attr_path).await?;
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

/// Extract package metadata using Nix evaluation
async fn extract_package_metadata(
    worktree_path: &Path,
    attr_path: &str,
) -> anyhow::Result<serde_json::Value> {
    // Try to run nix-instantiate to get package metadata
    // This may fail if the package is in a broken state, which is fine
    let output = Command::new("nix-instantiate")
        .args([
            "--eval",
            "--json",
            "--expr",
            &format!(
                "with import {}/<nixpkgs> {{}}; builtins.tryEval (builtins.toJSON {}.meta or {{}})",
                worktree_path.display(),
                attr_path
            ),
        ])
        .output()
        .await?;

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
    let base_dirs = directories::ProjectDirs::from("com", "ekapusta", "ekapkgs-update")
        .context("Failed to determine project directories")?;
    let failed_dir = base_dirs.cache_dir().join("failed");

    if !failed_dir.exists() {
        return Ok(Vec::new());
    }

    let mut all_artifacts = Vec::new();

    let mut session_entries = fs::read_dir(&failed_dir).await?;
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
