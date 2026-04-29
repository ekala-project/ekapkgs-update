use anyhow::Context;
use tracing::{debug, info};

use crate::git::{self, PrConfig};
use crate::github;
use crate::vcs_sources::SemverStrategy;

use super::update::{UpdateOutcome, perform_update};

/// Configuration for a package update with worktree isolation and PR creation.
pub struct Updater {
    /// Nix file to evaluate (e.g. "default.nix")
    pub eval_entry_point: String,
    /// Package attribute path (e.g. "pkgs.spdlog")
    pub attr_path: String,
    /// Absolute path to the package's nix file (from meta.position)
    pub file_location: String,
    /// Version selection strategy
    pub strategy: SemverStrategy,
    /// Whether to run passthru.tests
    pub run_passthru_tests: bool,
    /// Whether to fail on test errors
    pub fail_on_test_failure: bool,
    /// PR target configuration (upstream owner/repo/base branch)
    pub pr_config: PrConfig,
    /// Git remote to push branches to
    pub fork: String,
}

impl Updater {
    /// Execute the full pipeline: create worktree, update, commit, push, create PR, clean up.
    pub async fn execute(&self) -> anyhow::Result<UpdateOutcome> {
        let repo_root = git::get_repo_root().await?;
        let worktree_path = git::create_worktree(&self.attr_path).await?;

        // Remap both eval_entry_point and file_location into worktree
        let wt_eval =
            git::remap_to_worktree(&self.eval_entry_point, &repo_root, &worktree_path);
        let wt_file =
            git::remap_to_worktree(&self.file_location, &repo_root, &worktree_path);

        debug!("Worktree paths: eval={}, file={}", wt_eval, wt_file);

        // Run the update in the worktree
        let outcome = match perform_update(
            wt_eval,
            self.attr_path.clone(),
            wt_file,
            self.strategy,
            self.run_passthru_tests,
            self.fail_on_test_failure,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(e) => {
                git::cleanup_worktree(&worktree_path).await.ok();
                return Err(e);
            },
        };

        // Commit, push, create PR from worktree
        let github_token = std::env::var("GITHUB_TOKEN").context(
            "GITHUB_TOKEN environment variable is required for PR creation. Set it with: export \
             GITHUB_TOKEN=your_token_here",
        )?;

        let branch_name = match git::create_and_push_branch(
            &worktree_path,
            &self.attr_path,
            &outcome.old_version,
            &outcome.new_version,
            &self.fork,
            outcome.tests_passed,
        )
        .await
        {
            Ok(name) => name,
            Err(e) => {
                git::cleanup_worktree(&worktree_path).await.ok();
                return Err(e);
            },
        };

        // Build PR title and body
        let pr_title = format!(
            "{}: {} -> {}",
            self.attr_path, outcome.old_version, outcome.new_version
        );
        let mut pr_body = format!(
            "## Summary\n\nThis PR updates `{}` from version {} to {}.\n\n## Changes\n\n- \
             Updated package version\n- Updated source hash",
            self.attr_path, outcome.old_version, outcome.new_version
        );

        if let Some(description) = outcome.metadata.description.as_ref() {
            pr_body.push_str(&format!(
                "\n\n## Package Information\n\n**Description:** {}",
                description
            ));
        } else {
            pr_body.push_str("\n\n## Package Information");
        }
        if let Some(homepage) = outcome.metadata.homepage.as_ref() {
            pr_body.push_str(&format!("\n\n**Homepage:** {}", homepage));
        }
        if let Some(changelog) = outcome.metadata.changelog.as_ref() {
            pr_body.push_str(&format!("\n\n**Changelog:** {}", changelog));
        }

        pr_body.push_str("\n\n🤖 Generated with ekapkgs-update");

        let pr = match github::create_pull_request(
            &self.pr_config.owner,
            &self.pr_config.repo,
            &pr_title,
            &pr_body,
            &branch_name,
            &self.pr_config.base_branch,
            &github_token,
        )
        .await
        {
            Ok(pr) => pr,
            Err(e) => {
                git::cleanup_worktree(&worktree_path).await.ok();
                return Err(e);
            },
        };

        info!("✓ Created pull request: {}", pr.html_url);
        println!("Pull request created: {}", pr.html_url);

        // Clean up worktree
        git::cleanup_worktree(&worktree_path).await.ok();

        Ok(outcome)
    }
}
