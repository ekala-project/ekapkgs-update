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
    /// Directory structure: ~/.cache/ekapkgs-update/failed/{session_id}/{attr_path}/
    pub fn base_path(session_id: &str, attr_path: &str) -> anyhow::Result<PathBuf> {
        let base_dirs = directories::ProjectDirs::from("com", "ekapusta", "ekapkgs-update")
            .context("Failed to determine project directories")?;
        let cache_dir = base_dirs.cache_dir();

        let path = cache_dir
            .join("failed")
            .join(session_id)
            .join(attr_path.replace('.', "_").replace('/', "_"));
        Ok(path)
    }
}

/// Preserve a failed update's context for later inspection
pub async fn preserve_failure(
    session_id: &str,
    attr_path: &str,
    phase: UpdatePhase,
    worktree_path: &Path,
    error: &UpdateError,
    build_log: Option<String>,
    test_output: Option<String>,
) -> anyhow::Result<FailureArtifacts> {
    let base_path = FailureArtifacts::base_path(session_id, attr_path)?;
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
        session_id: session_id.to_string(),
        attr_path: attr_path.to_string(),
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
    all_artifacts.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    Ok(all_artifacts)
}
