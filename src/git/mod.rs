use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tracing::{debug, warn};

use crate::github::parse_github_url;

/// Create a git worktree for an isolated update
pub async fn create_worktree(attr_path: &str) -> anyhow::Result<PathBuf> {
    // Get XDG cache directory
    let cache_dir = directories::ProjectDirs::from("", "", "ekapkgs-update")
        .ok_or_else(|| anyhow::anyhow!("Failed to determine cache directory"))?
        .cache_dir()
        .to_path_buf();

    // Create a safe worktree directory name from attr_path
    let worktree_name = attr_path.replace(['.', '/'], "-");
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
        .args(["worktree", "add", worktree_path.to_str().unwrap(), "HEAD"])
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
        .args([
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

/// Create a git branch, commit changes, and push to remote
/// Returns the branch name
pub async fn create_and_push_branch(
    worktree_path: &Path,
    attr_path: &str,
    old_version: &str,
    new_version: &str,
    remote_repo: &str,
) -> anyhow::Result<String> {
    // Create a safe branch name from attr_path and version
    let sanitized_attr = attr_path.replace(['.', '/'], "-");
    let branch_name = format!("update/{}/{}", sanitized_attr, new_version);

    debug!(
        "{}: Creating branch '{}' in worktree {:?}",
        attr_path, branch_name, worktree_path
    );

    // Create new branch
    let output = Command::new("git")
        .current_dir(worktree_path)
        .args(["checkout", "-b", &branch_name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create branch '{}': {}", branch_name, stderr);
    }

    // Add all changes
    let output = Command::new("git")
        .current_dir(worktree_path)
        .args(["add", "-A"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to stage changes: {}", stderr);
    }

    // Create commit message
    let commit_message = format!(
        "Update {} from {} to {}\n\n🤖 Generated with ekapkgs-update\n\nCo-Authored-By: \
         ekapkgs-update <noreply@ekapkgs.org>",
        attr_path, old_version, new_version
    );

    // Commit changes
    let output = Command::new("git")
        .current_dir(worktree_path)
        .args(["commit", "-m", &commit_message])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to commit changes: {}", stderr);
    }

    debug!(
        "{}: Committed changes to branch '{}'",
        attr_path, branch_name
    );

    // Push to remote
    let push_target = format!("{}:{}", branch_name, branch_name);
    let output = Command::new("git")
        .current_dir(worktree_path)
        .args(["push", "-u", remote_repo, &push_target])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Failed to push branch '{}' to remote '{}': {}",
            branch_name,
            remote_repo,
            stderr
        );
    }

    debug!(
        "{}: Pushed branch '{}' to remote '{}'",
        attr_path, branch_name, remote_repo
    );

    Ok(branch_name)
}

/// PR configuration for creating pull requests
#[derive(Debug, Clone)]
pub struct PrConfig {
    pub owner: String,
    pub repo: String,
    pub base_branch: String,
}

/// Get PR configuration from a specific remote
pub async fn get_pr_config_from_remote(remote: &str) -> anyhow::Result<PrConfig> {
    debug!("Getting PR configuration from remote: {}", remote);

    // Get remote URL
    let remote_url = get_remote_url(remote).await?;
    debug!("Remote URL: {}", remote_url);

    // Parse GitHub owner/repo from URL
    let github_repo = parse_github_url(&remote_url)
        .ok_or_else(|| anyhow::anyhow!("Remote URL is not a GitHub repository: {}", remote_url))?;

    // Get default/base branch
    let base_branch = get_default_branch(remote).await?;
    debug!("Base branch: {}", base_branch);

    Ok(PrConfig {
        owner: github_repo.owner,
        repo: github_repo.repo,
        base_branch,
    })
}

/// Automatically detect PR configuration from git upstream
pub async fn get_pr_config_from_git() -> anyhow::Result<PrConfig> {
    debug!("Auto-detecting PR configuration from git upstream");

    // Get current branch
    let current_branch = get_current_branch().await?;
    debug!("Current branch: {}", current_branch);

    // Get upstream remote name
    let remote = get_upstream_remote(&current_branch).await?;
    debug!("Upstream remote: {}", remote);

    // Use the helper function
    get_pr_config_from_remote(&remote).await
}

/// Get the current git branch name
async fn get_current_branch() -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to get current branch: {}", stderr);
    }

    let branch = String::from_utf8(output.stdout)?.trim().to_string();

    if branch == "HEAD" {
        anyhow::bail!("Currently in detached HEAD state");
    }

    Ok(branch)
}

/// Get the upstream remote name for a branch
async fn get_upstream_remote(branch: &str) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["config", &format!("branch.{}.remote", branch)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        let remote = String::from_utf8(output.stdout)?.trim().to_string();
        if !remote.is_empty() {
            return Ok(remote);
        }
    }

    // Fallback to "origin"
    debug!(
        "No upstream remote configured for branch '{}', using 'origin'",
        branch
    );
    Ok("origin".to_string())
}

/// Get the URL for a git remote
async fn get_remote_url(remote: &str) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", remote])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to get URL for remote '{}': {}", remote, stderr);
    }

    let url = String::from_utf8(output.stdout)?.trim().to_string();

    Ok(url)
}

/// Get the default branch for a remote
async fn get_default_branch(remote: &str) -> anyhow::Result<String> {
    // First try: local cached symbolic ref (fast, no network)
    let symref_path = format!("refs/remotes/{}/HEAD", remote);
    let output = Command::new("git")
        .args(["symbolic-ref", &symref_path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        let full_ref = String::from_utf8(output.stdout)?.trim().to_string();

        // Extract branch name from "refs/remotes/origin/master"
        let prefix = format!("refs/remotes/{}/", remote);
        if let Some(branch) = full_ref.strip_prefix(&prefix) {
            debug!("Found default branch from local symbolic ref: {}", branch);
            return Ok(branch.to_string());
        }
    }

    debug!("Local symbolic ref not found, querying remote");

    // Second try: query remote directly (requires network)
    let output = Command::new("git")
        .args(["ls-remote", "--symref", remote, "HEAD"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        let stdout = String::from_utf8(output.stdout)?;

        // Parse output like: "ref: refs/heads/master	HEAD"
        for line in stdout.lines() {
            if line.starts_with("ref:") {
                if let Some(ref_part) = line.split_whitespace().nth(1) {
                    if let Some(branch) = ref_part.strip_prefix("refs/heads/") {
                        debug!("Found default branch from remote: {}", branch);
                        return Ok(branch.to_string());
                    }
                }
            }
        }
    }

    // Final fallback: try common branch names
    debug!("Could not determine default branch from remote, trying fallbacks");

    for candidate in &["master", "main"] {
        let output = Command::new("git")
            .args(["ls-remote", "--heads", remote, candidate])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if output.status.success() && !output.stdout.is_empty() {
            debug!("Using fallback branch: {}", candidate);
            return Ok(candidate.to_string());
        }
    }

    anyhow::bail!("Could not determine default branch for remote '{}'", remote)
}
