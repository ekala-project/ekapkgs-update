use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tracing::{debug, warn};

/// Create a git worktree for an isolated update
pub async fn create_worktree(attr_path: &str) -> anyhow::Result<PathBuf> {
    // Get XDG cache directory
    let cache_dir = directories::ProjectDirs::from("", "", "ekapkgs-update")
        .ok_or_else(|| anyhow::anyhow!("Failed to determine cache directory"))?
        .cache_dir()
        .to_path_buf();

    // Create a safe worktree directory name from attr_path
    let worktree_name = attr_path.replace('.', "-").replace('/', "-");
    let worktree_path = cache_dir
        .join("worktrees")
        .join(format!("update-{}", worktree_name));

    // Remove existing worktree if it exists
    if worktree_path.exists() {
        debug!(
            "{}: Removing existing worktree at {:?}",
            attr_path, worktree_path
        );
        cleanup_worktree(&worktree_path).await?;
    }

    // Create parent directory
    if let Some(parent) = worktree_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Create the worktree
    debug!("{}: Creating worktree at {:?}", attr_path, worktree_path);
    let output = Command::new("git")
        .args(&["worktree", "add", worktree_path.to_str().unwrap(), "HEAD"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create worktree: {}", stderr);
    }

    debug!("{}: Worktree created successfully", attr_path);
    Ok(worktree_path)
}

/// Clean up a git worktree
pub async fn cleanup_worktree(worktree_path: &Path) -> anyhow::Result<()> {
    if !worktree_path.exists() {
        return Ok(());
    }

    debug!("Cleaning up worktree at {:?}", worktree_path);

    // Remove the worktree using git worktree remove
    let output = Command::new("git")
        .args(&[
            "worktree",
            "remove",
            "--force",
            worktree_path.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Failed to remove worktree with git: {}", stderr);

        // Fall back to manual removal if git command fails
        if worktree_path.exists() {
            tokio::fs::remove_dir_all(worktree_path).await?;
        }
    }

    debug!("Worktree cleaned up successfully");
    Ok(())
}
