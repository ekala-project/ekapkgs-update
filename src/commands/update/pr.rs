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
    pub eval_entry_point: Option<&'a str>,
    pub directory_diff: bool,
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
            eval_entry_point,
            directory_diff,
        } = self;

        if create_pr {
            create_pr_for_update(
                attr_path,
                metadata,
                new_version,
                upstream,
                fork,
                tests_passed,
                eval_entry_point,
                directory_diff,
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
    eval_entry_point: Option<&str>,
    directory_diff: bool,
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

    // Perform directory diff if requested and eval_entry_point is available
    let diff_markdown = if directory_diff && eval_entry_point.is_some() {
        perform_directory_diff(eval_entry_point.unwrap(), attr_path).await.ok()
    } else {
        None
    };

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
    let pr_body = create_pr_body(attr_path, metadata, new_version, diff_markdown.as_deref());

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
fn create_pr_body(
    attr_path: &str,
    metadata: &PackageMetadata,
    new_version: &str,
    diff_markdown: Option<&str>,
) -> String {
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

    // Add directory diff if available
    if let Some(diff) = diff_markdown {
        pr_body.push_str("\n\n");
        pr_body.push_str(diff);
    }

    pr_body.push_str("\n\n🤖 Generated with ekapkgs-update");

    pr_body
}

/// Perform directory diff comparison between old and new package versions
///
/// This function:
/// 1. Builds the new version (current state, files already updated)
/// 2. Temporarily stashes changes to access the old version
/// 3. Builds the old version
/// 4. Restores the changes
/// 5. Compares the build outputs and returns formatted markdown
async fn perform_directory_diff(eval_entry_point: &str, attr_path: &str) -> anyhow::Result<String> {
    use crate::commands::update::{build_and_get_outputs, cleanup_result_symlinks};
    use crate::directory_diff::{compare_build_outputs, format_for_pr_body, DiffConfig};

    info!("Performing directory diff for {}", attr_path);

    // Step 1: Build the new version (files are already updated)
    debug!("Building new version");
    let new_outputs = build_and_get_outputs(eval_entry_point, attr_path, None)
        .await
        .context("Failed to build new version for directory diff")?;

    // Clean up result symlinks after capturing paths
    cleanup_result_symlinks()?;

    // Step 2: Stash changes to access the old version
    debug!("Stashing changes to access old version");
    let stash_output = Command::new("git")
        .args(["stash", "push", "-m", "ekapkgs-update: temporary stash for directory diff"])
        .output()
        .await?;

    if !stash_output.status.success() {
        let stderr = String::from_utf8_lossy(&stash_output.stderr);
        anyhow::bail!("Failed to stash changes: {}", stderr);
    }

    // Step 3: Build the old version
    debug!("Building old version");
    let old_outputs_result = build_and_get_outputs(eval_entry_point, attr_path, None).await;

    // Clean up result symlinks
    let _ = cleanup_result_symlinks();

    // Step 4: Restore changes (pop stash)
    debug!("Restoring changes");
    let pop_output = Command::new("git")
        .args(["stash", "pop"])
        .output()
        .await?;

    if !pop_output.status.success() {
        let stderr = String::from_utf8_lossy(&pop_output.stderr);
        anyhow::bail!("Failed to restore stashed changes: {}", stderr);
    }

    // Check if old build succeeded
    let old_outputs = old_outputs_result
        .context("Failed to build old version for directory diff")?;

    // Step 5: Compare the outputs
    debug!("Comparing build outputs");
    let config = DiffConfig::default();
    let diff = compare_build_outputs(&old_outputs, &new_outputs, &config)
        .context("Failed to compare directory outputs")?;

    // Step 6: Format as markdown
    let markdown = format_for_pr_body(&diff);

    info!("Directory diff completed successfully");
    Ok(markdown)
}
