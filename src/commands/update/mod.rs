mod build;
mod file_update;
mod git;
mod script;
mod variants;

pub use build::*;
pub use file_update::*;
pub use git::*;
pub use script::*;
pub use variants::*;

use std::process::Stdio;

use anyhow::Context;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::git::get_pr_config_from_git;
use crate::github;
use crate::hash_discovery;
use crate::nix::{eval_nix_expr, get_variants_list, has_passthru_tests, is_many_variants_package, normalize_entry_point};
use crate::package::PackageMetadata;
use crate::rewrite::{is_patches_array_empty, remove_patch_from_array, remove_patches_attribute};
use crate::variant_strategy::{infer_strategy_from_variant, is_variant_pinned};
use crate::vcs_sources::{SemverStrategy, UpstreamSource};

/// Main update entry point
pub async fn update(
    file: String,
    attr_path: String,
    semver_strategy: String,
    ignore_update_script: bool,
    commit: bool,
    create_pr: bool,
    upstream: Option<String>,
    fork: String,
    run_passthru_tests: bool,
    variant: Option<String>,
    all_variants: bool,
) -> anyhow::Result<()> {
    // Parse semver strategy
    let strategy = SemverStrategy::from_str(&semver_strategy)?;
    info!("Using semver strategy: {:?}", strategy);

    // Check if this is a mkManyVariants package
    let is_many_variants = is_many_variants_package(&file, &attr_path).await?;

    if is_many_variants {
        info!("{} is a mkManyVariants package", attr_path);

        // Determine which variants to update
        let variants_to_update = if let Some(ref specific_variant) = variant {
            // Update only the specified variant
            vec![specific_variant.clone()]
        } else if all_variants {
            // Update all variants (when --all-variants flag is set)
            get_variants_list(&file, &attr_path).await?
        } else {
            // Update only the default variant
            let default_variant = get_default_variant(&file, &attr_path).await?;
            info!("Using default variant: {}", default_variant);
            vec![default_variant]
        };

        info!("Variants to update: {:?}", variants_to_update);

        // Update each variant
        for variant_name in variants_to_update {
            // Skip pinned variants (3+ version components)
            if is_variant_pinned(&variant_name) {
                info!(
                    "Skipping pinned variant '{}' (3+ version components)",
                    variant_name
                );
                continue;
            }

            // Infer or use explicit strategy for this variant
            let variant_strategy = match infer_strategy_from_variant(&variant_name) {
                Some(inferred) => {
                    info!(
                        "Inferred {:?} strategy for variant '{}'",
                        inferred, variant_name
                    );
                    inferred
                },
                None => {
                    info!(
                        "No strategy inferred for variant '{}', using explicit strategy: {:?}",
                        variant_name, strategy
                    );
                    strategy
                },
            };

            // Update this variant
            info!(
                "Updating variant '{}' with strategy {:?}",
                variant_name, variant_strategy
            );
            match update_single_variant(
                &file,
                &attr_path,
                &variant_name,
                variant_strategy,
                commit,
                create_pr,
                upstream.clone(),
                fork.clone(),
                run_passthru_tests,
            )
            .await
            {
                Ok(()) => info!("Successfully updated variant '{}'", variant_name),
                Err(e) => {
                    warn!("Failed to update variant '{}': {}", variant_name, e);
                    // Continue with other variants
                },
            }
        }

        return Ok(());
    }

    // Not a mkManyVariants package - use regular update flow
    // Try to run update script if not ignored
    if !ignore_update_script {
        let script_executed = run_update_script(&file, &attr_path).await?;
        if script_executed {
            return Ok(());
        }
    } else {
        info!("Ignoring update script for {}", attr_path);
    }

    // No update script or ignoring it - use generic update method
    // Try to find the package file location via meta.position
    debug!("Attempting to locate package definition...");
    let normalized_entry = normalize_entry_point(&file);
    let position_expr = format!(
        "with import {} {{ }}; {}.meta.position",
        normalized_entry, attr_path
    );

    let expr_file_path = eval_nix_expr(&position_expr).await.and_then(|position| {
        if position.is_empty() {
            anyhow::bail!("Empty position returned from meta.position");
        }
        // Parse position string (format: "file:line")
        let (file_path, _line_str) = position
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {}", position))?;
        Ok(file_path.to_string())
    })?;

    update_from_file_path(
        file,
        attr_path,
        expr_file_path,
        strategy,
        commit,
        create_pr,
        upstream,
        fork,
        run_passthru_tests,
        false, // Don't fail on test errors for update command
    )
    .await?;

    Ok(())
}

/// Extract hash from Nix build error output (local helper)
fn extract_hash_from_error(stderr: &str) -> Option<String> {
    hash_discovery::extract_hash(stderr)
}

/// Update a package from a specific file path
pub async fn update_from_file_path(
    eval_entry_point: String,
    attr_path: String,
    file_location: String,
    strategy: SemverStrategy,
    commit: bool,
    create_pr: bool,
    upstream: Option<String>,
    fork: String,
    run_passthru_tests: bool,
    fail_on_test_failure: bool,
) -> anyhow::Result<()> {
    info!(
        "Starting generic update for {} at {}",
        attr_path, file_location
    );

    // Step 1: Extract package metadata
    let metadata = PackageMetadata::from_attr_path(&eval_entry_point, &attr_path).await?;
    info!("Current version: {}", metadata.version);

    // Step 2: Determine upstream source
    let upstream_source = if let Some(ref src_url) = metadata.src_url {
        // Try to parse URL as GitHub/GitLab/PyPI
        UpstreamSource::from_url(src_url)
            .context("Source is not from a supported VCS platform (GitHub, GitLab, PyPI)")?
    } else if let Some(ref pname) = metadata.pname {
        // If no src_url but pname exists, create PyPI source directly
        UpstreamSource::PyPI {
            pname: pname.clone(),
        }
    } else {
        anyhow::bail!(
            "No source URL or pname found for package - cannot determine upstream source"
        );
    };

    info!("{}", upstream_source.description());

    // Step 3: Fetch best compatible release based on strategy
    let best_release = upstream_source
        .get_compatible_release(&metadata.version, strategy)
        .await?;

    let new_version = UpstreamSource::get_version(&best_release);
    info!(
        "Found compatible version ({:?}): {} -> {}",
        strategy, metadata.version, new_version
    );

    // Step 5: Update version in file with invalid hash
    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let actual_file_location = update_nix_file(
        &eval_entry_point,
        &attr_path,
        &file_location,
        &metadata.version,
        &new_version,
        metadata.output_hash.as_deref(),
        Some(invalid_hash),
    )
    .await?;

    info!(
        "Updated version and set invalid hash in {}",
        actual_file_location
    );

    // Step 6: Build source to get correct hash
    let (success, _stdout, stderr) =
        build_nix_expr(&eval_entry_point, &attr_path, Some("src")).await?;

    if success {
        warn!("Build succeeded with invalid hash - this shouldn't happen");
        anyhow::bail!("Expected hash mismatch error but build succeeded");
    }

    let correct_hash = extract_hash_from_error(&stderr).ok_or_else(|| {
        anyhow::anyhow!(
            "Could not extract correct hash from build error:\n{}",
            stderr
        )
    })?;

    info!("Extracted correct hash: {}", correct_hash);

    // Step 7: Update hash with correct value (use actual file location from step 5)
    let _ = update_nix_file(
        &eval_entry_point,
        &attr_path,
        &actual_file_location,
        &new_version, // version stays the same
        &new_version,
        Some(invalid_hash),
        Some(&correct_hash),
    )
    .await?;

    info!("Updated hash in {}", actual_file_location);

    // Step 8: Build source again to verify
    let (success, _stdout, stderr) =
        build_nix_expr(&eval_entry_point, &attr_path, Some("src")).await?;

    if !success {
        anyhow::bail!("Source build failed after hash update:\n{}", stderr);
    }

    info!("Source build successful");

    // For Rust packages, update cargoHash
    if let Some(old_cargo_hash) = &metadata.cargo_hash {
        info!("Detected Rust package, updating cargoHash");

        // Set invalid cargo hash
        let invalid_cargo_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_cargo_hash(&actual_file_location, old_cargo_hash, invalid_cargo_hash).await?;

        info!("Set invalid cargoHash in {}", actual_file_location);

        // Build full package to get correct cargo hash
        let (success, _stdout, stderr) =
            build_nix_expr(&eval_entry_point, &attr_path, None).await?;

        if success {
            warn!("Build succeeded with invalid cargoHash - this shouldn't happen");
            anyhow::bail!("Expected cargoHash mismatch error but build succeeded");
        }

        let correct_cargo_hash = extract_hash_from_error(&stderr).ok_or_else(|| {
            anyhow::anyhow!(
                "Could not extract correct cargoHash from build error:\n{}",
                stderr
            )
        })?;

        info!("Extracted correct cargoHash: {}", correct_cargo_hash);

        // Update cargoHash with correct value
        update_cargo_hash(
            &actual_file_location,
            invalid_cargo_hash,
            &correct_cargo_hash,
        )
        .await?;

        info!("Updated cargoHash in {}", actual_file_location);
    }

    // For Go packages, update vendorHash
    if let Some(old_vendor_hash) = &metadata.vendor_hash {
        info!("Detected Go package, updating vendorHash");

        // Set invalid vendor hash
        let invalid_vendor_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_vendor_hash(&actual_file_location, old_vendor_hash, invalid_vendor_hash).await?;

        info!("Set invalid vendorHash in {}", actual_file_location);

        // Build full package to get correct vendor hash
        let (success, _stdout, stderr) =
            build_nix_expr(&eval_entry_point, &attr_path, None).await?;

        if success {
            warn!("Build succeeded with invalid vendorHash - this shouldn't happen");
            anyhow::bail!("Expected vendorHash mismatch error but build succeeded");
        }

        let correct_vendor_hash = extract_hash_from_error(&stderr).ok_or_else(|| {
            anyhow::anyhow!(
                "Could not extract correct vendorHash from build error:\n{}",
                stderr
            )
        })?;

        info!("Extracted correct vendorHash: {}", correct_vendor_hash);

        // Update vendorHash with correct value
        update_vendor_hash(
            &actual_file_location,
            invalid_vendor_hash,
            &correct_vendor_hash,
        )
        .await?;

        info!("Updated vendorHash in {}", actual_file_location);
    }

    // Step 9: Build full package to verify with reversed patch recovery
    loop {
        let (success, _stdout, stderr) =
            build_nix_expr(&eval_entry_point, &attr_path, None).await?;

        if success {
            // Build succeeded - check if patches array is now empty
            let content = tokio::fs::read_to_string(&actual_file_location).await?;
            if is_patches_array_empty(&content) {
                match remove_patches_attribute(&content) {
                    Ok(updated_content) => {
                        tokio::fs::write(&actual_file_location, updated_content).await?;
                        debug!("Removed empty patches attribute");
                    },
                    Err(e) => {
                        debug!("Could not remove empty patches attribute: {}", e);
                        // Not a critical error, continue
                    },
                }
            }
            break;
        }

        // Build failed - check for reversed patch errors
        if let Some(patch_name) = detect_reversed_patch(&stderr) {
            debug!("Detected reversed patch: {}", patch_name);

            // Read the file
            let content = tokio::fs::read_to_string(&actual_file_location).await?;

            // Remove the patch
            match remove_patch_from_array(&content, &patch_name) {
                Ok(updated_content) => {
                    // Write the updated content back
                    tokio::fs::write(&actual_file_location, updated_content).await?;
                    debug!("Removed obsolete patch: {}", patch_name);
                    // Continue loop to retry the build
                },
                Err(e) => {
                    warn!("Failed to remove patch {}: {}", patch_name, e);
                    // Can't remove the patch, return the original error
                    anyhow::bail!(
                        "Package build failed after update. Detected reversed patch but couldn't \
                         remove it: {}\n{}",
                        e,
                        stderr
                    );
                },
            }
        } else {
            // No reversed patch detected - this is a real build failure
            warn!("Full package build failed:\n{}", stderr);
            anyhow::bail!(
                "Package build failed after update. You may need to manually fix build issues."
            );
        }
    }

    // Run passthru.tests if requested
    let mut tests_passed = false;
    info!("Checking for passthru.tests...");
    if run_passthru_tests {
        // Check if tests exist using nix eval
        let normalized_entry = normalize_entry_point(&eval_entry_point);

        if has_passthru_tests(&normalized_entry, &attr_path).await? {
            info!("Found {}.passthru.tests, building tests...", &attr_path);

            // Build tests
            let (success, _stdout, stderr) =
                build_nix_expr(&eval_entry_point, &attr_path, Some("passthru.tests")).await?;

            if !success {
                warn!("Tests failed:\n{}", stderr);
                if fail_on_test_failure {
                    anyhow::bail!("Package tests failed after update");
                } else {
                    warn!("Package tests failed after update, but continuing anyway");
                }
            } else {
                info!("✓ Tests passed");
                tests_passed = true;
            }
        } else {
            info!("No passthru.tests found for {}", attr_path);
        }
    }

    info!(
        "✓ Successfully updated {} from {} to {}",
        attr_path, metadata.version, new_version
    );

    // Handle commit and PR creation
    if create_pr {
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
        let commit_message = if tests_passed {
            format!(
                "Update {} from {} to {}\n\nTests: passthru.tests passed\n\n🤖 Generated with \
                 ekapkgs-update\n\nCo-Authored-By: ekapkgs-update <noreply@ekapkgs.org>",
                attr_path, metadata.version, new_version
            )
        } else {
            format!(
                "Update {} from {} to {}\n\n🤖 Generated with ekapkgs-update\n\nCo-Authored-By: \
                 ekapkgs-update <noreply@ekapkgs.org>",
                attr_path, metadata.version, new_version
            )
        };

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
            .args(["push", "-u", &fork, &push_target])
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
    } else if commit {
        // Just create a commit without PR
        create_git_commit(&attr_path, &metadata.version, &new_version, tests_passed).await?;
    }

    Ok(())
}
