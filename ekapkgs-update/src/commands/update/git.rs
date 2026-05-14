use anyhow::Context;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Create a git commit for the update
pub async fn create_git_commit(
    attr_path: &str,
    old_version: &str,
    new_version: &str,
    tests_passed: bool,
) -> anyhow::Result<()> {
    info!("Creating git commit for update");

    // Check if we're in a git repository
    let git_check = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .await
        .context("Failed to check if directory is a git repository")?;

    if !git_check.status.success() {
        anyhow::bail!("Not in a git repository - cannot create commit");
    }

    // Get list of modified files
    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .await
        .context("Failed to run git status")?;

    if !status_output.status.success() {
        anyhow::bail!("git status failed");
    }

    let status_str = String::from_utf8_lossy(&status_output.stdout);
    let modified_files: Vec<&str> = status_str
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            // Parse git status output (format: "XY filename")
            let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
            if parts.len() == 2 {
                Some(parts[1].trim())
            } else {
                None
            }
        })
        .collect();

    if modified_files.is_empty() {
        warn!("No files to commit");
        return Ok(());
    }

    debug!("Files to commit: {:?}", modified_files);

    // Stage all modified files
    let mut add_cmd = Command::new("git");
    add_cmd.arg("add");
    for file in &modified_files {
        add_cmd.arg(file);
    }

    let add_output = add_cmd.output().await.context("Failed to run git add")?;

    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        anyhow::bail!("git add failed: {stderr}");
    }

    // Create commit with formatted message
    let commit_message = if tests_passed {
        format!("{attr_path}: {old_version} -> {new_version}\n\nTests: passthru.tests passed")
    } else {
        format!("{attr_path}: {old_version} -> {new_version}")
    };
    let commit_output = Command::new("git")
        .args(["commit", "-m", &commit_message])
        .output()
        .await
        .context("Failed to run git commit")?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        anyhow::bail!("git commit failed: {stderr}");
    }

    info!("✓ Created commit: {}", commit_message);

    Ok(())
}
