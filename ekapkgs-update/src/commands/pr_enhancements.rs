//! Configuration for PR enhancement features
//!
//! Bundles options that influence what additional analyses and metadata are
//! attached to PRs created by the updater (CVE checks, Repology comparison,
//! directory diff, rebuild analysis). This struct is shared between the `run`
//! and `update` commands so both code paths can pass a single value around.

use std::path::Path;

use serde::Serialize;

/// Configuration for optional PR enhancements.
///
/// Naming convention: positive flags are `true` to enable a feature
/// (`directory_diff`, `analyze_rebuilds`), and `skip_*` flags are `true` to
/// disable a feature (`skip_cve_check`, `skip_repology`). CLI flags use the
/// `--no-*` form and are translated when constructing this struct.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PrEnhancementsConfig {
    /// Whether to include directory diff comparison in PR body
    pub directory_diff: bool,

    /// Whether to skip CVE vulnerability checking
    pub skip_cve_check: bool,

    /// Whether to skip Repology cross-distribution version checking
    pub skip_repology: bool,

    /// Whether to analyze and report rebuild counts for each update
    pub analyze_rebuilds: bool,

    /// Skip updates that would cause more than N rebuilds
    pub max_rebuilds: Option<usize>,

    /// Whether to skip pushing build outputs to Cachix
    pub skip_cachix: bool,

    /// Cachix cache name to push successful builds to. `None` disables
    /// Cachix push entirely. Resolved from CLI flag → `CACHIX_CACHE_NAME`
    /// env var by [`crate::cachix::resolve_cache_name`].
    pub cachix_cache: Option<String>,

    /// Whether to run package audits after successful builds
    pub run_audit: bool,
}

impl PrEnhancementsConfig {
    /// Whether any rebuild-count threshold should cause this update to be
    /// skipped.
    pub fn should_skip_for_rebuilds(&self, rebuild_count: usize) -> bool {
        self.max_rebuilds
            .is_some_and(|threshold| rebuild_count > threshold)
    }

    /// Build the package in the worktree and push all of its outputs to the
    /// configured Cachix cache. The push is a side effect — the result is
    /// intentionally not surfaced anywhere in the PR body (reviewers don't
    /// need the raw store paths). Push failures are logged inside
    /// [`crate::cachix::push_outputs`] and not propagated.
    ///
    /// Returns `Ok(())` when the feature is disabled or no cache is
    /// configured. Returns `Err(_)` only for build failures, which the
    /// caller logs but does not propagate.
    pub async fn perform_worktree_cachix_push(
        &self,
        worktree_path: &Path,
        eval_entry_point: &str,
        attr_path: &str,
    ) -> anyhow::Result<()> {
        if self.skip_cachix {
            return Ok(());
        }
        let Some(cache_name) = self.cachix_cache.as_deref() else {
            return Ok(());
        };

        use anyhow::Context;
        use tracing::{debug, info};

        use crate::commands::update::{build_and_get_outputs, cleanup_result_symlinks};

        info!(
            "{}: Building for Cachix push to '{}'",
            attr_path, cache_name
        );

        // Run the build from inside the worktree so any worktree-local Nix
        // imports resolve correctly. The Nix store deduplicates with any
        // build performed earlier (e.g. by `update_from_file_path` or the
        // directory-diff pass), so this is typically near-instant.
        std::env::set_current_dir(worktree_path)?;

        let outputs = build_and_get_outputs(eval_entry_point, attr_path, None)
            .await
            .with_context(|| format!("Failed to build {attr_path} for Cachix push"))?;

        // Best-effort symlink cleanup; not fatal if it fails.
        cleanup_result_symlinks().ok();

        debug!(
            "{}: Built {} output(s) for Cachix push",
            attr_path,
            outputs.len()
        );

        // Discard the push result: PR rendering does not use it.
        let _ = crate::cachix::push_outputs(Some(cache_name), &outputs, false).await;
        Ok(())
    }

    /// Perform a directory diff against the previous commit in the given
    /// worktree, returning the formatted markdown if the feature is enabled.
    ///
    /// This function:
    /// 1. Builds the new version (current HEAD in worktree)
    /// 2. Checks out the previous commit (HEAD~1)
    /// 3. Builds the old version
    /// 4. Restores the original HEAD
    /// 5. Compares the build outputs and formats markdown
    ///
    /// Returns `Ok(None)` when the feature is disabled.
    pub async fn perform_worktree_directory_diff(
        &self,
        worktree_path: &Path,
        eval_entry_point: &str,
        attr_path: &str,
    ) -> anyhow::Result<Option<String>> {
        if !self.directory_diff {
            return Ok(None);
        }

        use anyhow::Context;
        use tokio::process::Command;
        use tracing::{debug, info};

        use crate::commands::update::{build_and_get_outputs, cleanup_result_symlinks};
        use crate::directory_diff::{DiffConfig, compare_build_outputs, format_for_pr_body};

        info!("{}: Performing directory diff in worktree", attr_path);

        // Change to worktree directory for all git operations
        std::env::set_current_dir(worktree_path)?;

        // Step 1: Build new version (current HEAD)
        debug!("{}: Building new version", attr_path);
        let new_outputs = build_and_get_outputs(eval_entry_point, attr_path, None)
            .await
            .with_context(|| {
                format!("Failed to build new version of {attr_path} for directory diff")
            })?;

        cleanup_result_symlinks()?;

        // Step 2: Check out previous commit
        debug!("{}: Checking out previous commit (HEAD~1)", attr_path);
        let checkout_output = Command::new("git")
            .args(["checkout", "HEAD~1"])
            .current_dir(worktree_path)
            .output()
            .await?;

        if !checkout_output.status.success() {
            let stderr = String::from_utf8_lossy(&checkout_output.stderr);
            anyhow::bail!("Failed to checkout previous commit: {stderr}");
        }

        // Step 3: Build old version
        debug!("{}: Building old version", attr_path);
        let old_outputs_result = build_and_get_outputs(eval_entry_point, attr_path, None).await;

        cleanup_result_symlinks().ok();

        // Step 4: Check back to new version (return to branch HEAD)
        debug!("{}: Restoring to current commit", attr_path);
        let restore_output = Command::new("git")
            .args(["checkout", "-"])
            .current_dir(worktree_path)
            .output()
            .await?;

        if !restore_output.status.success() {
            let stderr = String::from_utf8_lossy(&restore_output.stderr);
            anyhow::bail!("Failed to restore to current commit: {stderr}");
        }

        // Check if old build succeeded
        let old_outputs = old_outputs_result.with_context(|| {
            format!("Failed to build old version of {attr_path} for directory diff")
        })?;

        // Step 5: Compare outputs
        debug!("{}: Comparing build outputs", attr_path);
        let config = DiffConfig::default();
        let diff = compare_build_outputs(&old_outputs, &new_outputs, &config)
            .with_context(|| format!("Failed to compare directory outputs for {attr_path}"))?;

        // Step 6: Format as markdown
        let markdown = format_for_pr_body(&diff);

        info!("{}: Directory diff completed successfully", attr_path);
        Ok(Some(markdown))
    }

    /// Audit the built package in the given worktree, returning formatted
    /// markdown if any findings exist.
    ///
    /// Returns `Ok(None)` when the feature is disabled or no findings were
    /// produced.
    pub async fn perform_worktree_audit(
        &self,
        worktree_path: &Path,
        eval_entry_point: &str,
        attr_path: &str,
    ) -> anyhow::Result<Option<String>> {
        if !self.run_audit {
            return Ok(None);
        }

        use anyhow::Context;
        use tracing::info;

        use crate::commands::audit::types::AuditConfig;
        use crate::commands::audit::{checks, report};
        use crate::commands::update::{build_and_get_outputs, cleanup_result_symlinks};

        info!("{}: Running package audit", attr_path);

        std::env::set_current_dir(worktree_path)?;

        let outputs = build_and_get_outputs(eval_entry_point, attr_path, None)
            .await
            .with_context(|| format!("Failed to build {attr_path} for audit"))?;

        cleanup_result_symlinks().ok();

        let config = AuditConfig::default();
        let audit_report =
            checks::run_audit(&outputs, Some(eval_entry_point), Some(attr_path), &config).await;

        if audit_report.findings.is_empty() {
            info!("{}: Audit passed with no findings", attr_path);
            return Ok(None);
        }

        info!(
            "{}: Audit found {} error(s), {} warning(s), {} info",
            attr_path,
            audit_report.error_count(),
            audit_report.warning_count(),
            audit_report.info_count()
        );

        Ok(Some(report::format_markdown(&audit_report)))
    }

    /// Perform a directory diff for the in-place update flow (uses git stash
    /// rather than checkout, since changes are still uncommitted).
    ///
    /// Returns `Ok(None)` when the feature is disabled or
    /// `eval_entry_point` is `None`.
    pub async fn perform_inplace_directory_diff(
        &self,
        eval_entry_point: Option<&str>,
        attr_path: &str,
    ) -> anyhow::Result<Option<String>> {
        if !self.directory_diff {
            return Ok(None);
        }
        let Some(eval_entry_point) = eval_entry_point else {
            return Ok(None);
        };

        use anyhow::Context;
        use tokio::process::Command;
        use tracing::{debug, info};

        use crate::commands::update::{build_and_get_outputs, cleanup_result_symlinks};
        use crate::directory_diff::{DiffConfig, compare_build_outputs, format_for_pr_body};

        info!("Performing directory diff for {}", attr_path);

        // Step 1: Build the new version (files are already updated)
        debug!("Building new version");
        let new_outputs = build_and_get_outputs(eval_entry_point, attr_path, None)
            .await
            .context("Failed to build new version for directory diff")?;

        cleanup_result_symlinks()?;

        // Step 2: Stash changes to access the old version
        debug!("Stashing changes to access old version");
        let stash_output = Command::new("git")
            .args([
                "stash",
                "push",
                "-m",
                "ekapkgs-update: temporary stash for directory diff",
            ])
            .output()
            .await
            .context("git stash push")?;

        if !stash_output.status.success() {
            let stderr = String::from_utf8_lossy(&stash_output.stderr);
            anyhow::bail!("Failed to stash changes: {stderr}");
        }

        // Step 3: Build the old version
        debug!("Building old version");
        let old_outputs_result = build_and_get_outputs(eval_entry_point, attr_path, None).await;

        let _ = cleanup_result_symlinks();

        // Step 4: Restore changes (pop stash)
        debug!("Restoring changes");
        let pop_output = Command::new("git")
            .args(["stash", "pop"])
            .output()
            .await
            .context("git stash pop")?;

        if !pop_output.status.success() {
            let stderr = String::from_utf8_lossy(&pop_output.stderr);
            anyhow::bail!("Failed to restore stashed changes: {stderr}");
        }

        let old_outputs =
            old_outputs_result.context("Failed to build old version for directory diff")?;

        // Step 5: Compare the outputs
        debug!("Comparing build outputs");
        let config = DiffConfig::default();
        let diff = compare_build_outputs(&old_outputs, &new_outputs, &config)
            .context("Failed to compare directory outputs")?;

        // Step 6: Format as markdown
        let markdown = format_for_pr_body(&diff);

        info!("Directory diff completed successfully");
        Ok(Some(markdown))
    }
}
