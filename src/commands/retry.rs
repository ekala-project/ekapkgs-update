//! Retry command for resuming failed updates

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use tokio::fs;
use tokio::process::Command;
use tracing::{info, warn};

use crate::commands::run::preservation::list_preserved_failures;

/// Retry a failed update from preserved worktree
pub async fn retry(
    database_path: String,
    attr_path: String,
    from_phase: Option<String>,
    apply_patch: Option<PathBuf>,
    _version: Option<String>,
) -> anyhow::Result<()> {
    let _expanded_path = shellexpand::tilde(&database_path).to_string();

    info!("Attempting to retry update for: {}", attr_path);

    // Find the most recent preserved failure for this package
    let artifacts_list = list_preserved_failures().await?;
    let artifact = artifacts_list
        .iter()
        .find(|a| a.attr_path == attr_path)
        .with_context(|| format!("No preserved artifacts found for {}", attr_path))?;

    info!(
        "Found preserved failure from session {} (phase: {})",
        artifact.session_id,
        artifact.failed_phase.as_str()
    );

    // Verify worktree exists
    if !artifact.worktree_path.exists() {
        bail!(
            "Preserved worktree no longer exists at: {}",
            artifact.worktree_path.display()
        );
    }

    // Create a temporary working directory for the retry
    let retry_dir = std::env::temp_dir().join(format!(
        "ekapkgs-retry-{}-{}",
        attr_path.replace('.', "_").replace('/', "_"),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&retry_dir).await?;

    info!("Copying worktree to: {}", retry_dir.display());

    // Copy the preserved worktree to the retry directory
    copy_dir_all(&artifact.worktree_path, &retry_dir).await?;

    // Change to the retry directory
    std::env::set_current_dir(&retry_dir)?;

    // Apply patch if provided
    if let Some(ref patch_path) = apply_patch {
        info!("Applying patch: {}", patch_path.display());
        apply_patch_file(patch_path).await?;
    }

    // Determine what phase to resume from
    if let Some(phase) = from_phase {
        warn!("Resume-from-phase not yet implemented, starting from beginning");
        warn!("Requested phase: {}", phase);
    }

    info!("Retrying update for {}", attr_path);

    // For now, we'll just invoke the update command with the current state
    // In a full implementation, we would:
    // 1. Detect the eval entry point from the worktree
    // 2. Determine the version config
    // 3. Skip phases that already succeeded if resuming

    // Read metadata from preserved artifacts to understand the update parameters
    let metadata_path = artifact.worktree_path.parent()
        .context("No parent directory")?
        .join("metadata.json");

    if !metadata_path.exists() {
        bail!("Cannot retry: metadata not found in preserved artifacts");
    }

    // For now, provide instructions to the user
    println!("\n{}", "=".repeat(80));
    println!("Retry Environment Prepared");
    println!("{}", "=".repeat(80));
    println!();
    println!("Working directory: {}", retry_dir.display());
    println!("Package:           {}", attr_path);
    println!("Failed phase:      {}", artifact.failed_phase.as_str());
    println!();

    if apply_patch.is_some() {
        println!("Patch applied successfully.");
        println!();
    }

    println!("Next steps:");
    println!("1. Review changes with: git -C {} diff", retry_dir.display());
    println!("2. Manually run update command:");
    println!("   cd {}", retry_dir.display());
    println!("   ekapkgs-update update <file> {} [options]", attr_path);
    println!();
    println!("Or, build and test manually:");
    println!("   cd {}", retry_dir.display());
    println!("   nix-build -A {}", attr_path);
    println!();
    println!("{}", "=".repeat(80));
    println!();
    println!("Note: Automatic retry execution is not yet fully implemented.");
    println!("The worktree has been prepared for manual intervention.");
    println!();

    Ok(())
}

/// Copy directory recursively (async version)
async fn copy_dir_all(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst).await?;
    let mut entries = fs::read_dir(src).await?;

    while let Some(entry) = entries.next_entry().await? {
        let ty = entry.file_type().await?;
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            Box::pin(copy_dir_all(&entry.path(), &dst_path)).await?;
        } else {
            fs::copy(entry.path(), dst_path).await?;
        }
    }
    Ok(())
}

/// Apply a patch file to the current directory
async fn apply_patch_file(patch_path: &Path) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(["apply", "--verbose"])
        .arg(patch_path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to apply patch: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        info!("Patch output: {}", stdout);
    }

    Ok(())
}
