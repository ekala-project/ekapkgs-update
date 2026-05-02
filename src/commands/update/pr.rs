use std::process::Stdio;

use anyhow::Context;
use tokio::process::Command;
use tracing::{debug, info};

use super::create_git_commit;
use crate::git::get_pr_config_from_git;
use crate::github;
use crate::package::PackageMetadata;

/// Parameters for post-update commit/PR operations
pub struct PostUpdateParams<'a> {
    pub attr_path: &'a str,
    pub metadata: &'a PackageMetadata,
    pub new_version: &'a str,
    pub commit: bool,
    pub create_pr: bool,
    pub upstream: Option<String>,
    pub fork: &'a str,
    pub tests_passed: bool,
}

impl<'a> PostUpdateParams<'a> {
    /// Execute post-update commit/PR operations
    pub async fn execute(self) -> anyhow::Result<()> {
        let PostUpdateParams {
            attr_path,
            metadata,
            new_version,
            commit,
            create_pr,
            upstream,
            fork,
            tests_passed,
        } = self;

        if create_pr {
            create_pr_for_update(
                attr_path,
                metadata,
                new_version,
                upstream,
                fork,
                tests_passed,
            )
            .await?;
        } else if commit {
            // Just create a commit without PR
            create_git_commit(attr_path, &metadata.version, new_version, tests_passed).await?;
        }

        Ok(())
    }
}

/// Create a pull request for the package update
pub async fn create_pr_for_update(
    attr_path: &str,
    metadata: &PackageMetadata,
    new_version: &str,
    upstream: Option<String>,
    fork: &str,
    tests_passed: bool,
) -> anyhow::Result<()> {
    // Get PR configuration - use CLI override or auto-detect from git
    let pr_config = if let Some(remote_name) = upstream {
        crate::git::get_pr_config_from_remote(&remote_name).await?
    } else {
        get_pr_config_from_git().await?
    };

    // Get GitHub token from environment
    let github_token = std::env::var("GITHUB_TOKEN").context(
        "GITHUB_TOKEN environment variable is required for PR creation. Set it with: export \
         GITHUB_TOKEN=your_token_here",
    )?;

    info!("Creating pull request for {}", attr_path);

    // Create branch name
    let sanitized_attr = attr_path.replace(['.', '/'], "-");
    let branch_name = format!("update/{}/{}", sanitized_attr, new_version);

    // Create new branch
    debug!("Creating branch '{}'", branch_name);
    let output = Command::new("git")
        .args(["checkout", "-b", &branch_name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create branch '{}': {}", branch_name, stderr);
    }

    // Stage all changes
    debug!("Staging changes");
    let output = Command::new("git")
        .args(["add", "-A"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to stage changes: {}", stderr);
    }

    // Create commit with bot signature
    let commit_message =
        create_commit_message(attr_path, &metadata.version, new_version, tests_passed);

    debug!("Creating commit");
    let output = Command::new("git")
        .args(["commit", "-m", &commit_message])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to commit changes: {}", stderr);
    }

    // Push to remote
    debug!("Pushing branch to remote");
    let push_target = format!("{}:{}", branch_name, branch_name);
    let output = Command::new("git")
        .args(["push", "-u", fork, &push_target])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Failed to push branch '{}' to remote '{}': {}",
            branch_name,
            fork,
            stderr
        );
    }

    info!("Pushed branch '{}' to remote", branch_name);

    // Create pull request
    let pr_title = format!("{}: {} -> {}", attr_path, metadata.version, new_version);
    let pr_body = create_pr_body(attr_path, metadata, new_version);

    debug!("Creating pull request");
    let pr = github::create_pull_request(
        &pr_config.owner,
        &pr_config.repo,
        &pr_title,
        &pr_body,
        &branch_name,
        &pr_config.base_branch,
        &github_token,
    )
    .await?;

    info!("✓ Created pull request: {}", pr.html_url);
    println!("Pull request created: {}", pr.html_url);

    Ok(())
}


/// Create commit message with optional test status
fn create_commit_message(
    attr_path: &str,
    old_version: &str,
    new_version: &str,
    tests_passed: bool,
) -> String {
    if tests_passed {
        format!(
            "Update {} from {} to {}\n\nTests: passthru.tests passed\n\n🤖 Generated with \
             ekapkgs-update\n\nCo-Authored-By: ekapkgs-update <noreply@ekapkgs.org>",
            attr_path, old_version, new_version
        )
    } else {
        format!(
            "Update {} from {} to {}\n\n🤖 Generated with ekapkgs-update\n\nCo-Authored-By: \
             ekapkgs-update <noreply@ekapkgs.org>",
            attr_path, old_version, new_version
        )
    }
}

/// Create PR body with package metadata
fn create_pr_body(attr_path: &str, metadata: &PackageMetadata, new_version: &str) -> String {
    let mut pr_body = format!(
        "## Update {}\n\nUpdates from version {} to {}.",
        attr_path, metadata.version, new_version
    );

    // Add optional metadata fields
    if let Some(description) = metadata.description.as_ref() {
        pr_body.push_str(&format!("\n\n**Description:** {}", description));
    }
    if let Some(homepage) = metadata.homepage.as_ref() {
        pr_body.push_str(&format!("\n\n**Homepage:** {}", homepage));
    }
    if let Some(changelog) = metadata.changelog.as_ref() {
        pr_body.push_str(&format!("\n\n**Changelog:** {}", changelog));
    }

    pr_body.push_str("\n\n🤖 Generated with ekapkgs-update");

    pr_body
}
