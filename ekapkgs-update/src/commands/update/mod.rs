mod build;
mod config;
pub mod errors;
mod file_update;
mod flake;
mod format;
mod git;
mod hash_workflows;
pub mod instrumentation;
mod pr;
mod script;
pub mod types;
mod variants;

use std::path::Path;

use anyhow::Context;
// Externally-consumed re-exports (cli.rs, commands::pr_enhancements, integration tests).
pub use build::{build_and_get_outputs, cleanup_result_symlinks, set_extra_nix_build_args};
// Sibling-only re-exports kept narrow with `pub(super)` so child modules can
// reach them via `super::`, but no other crate module sees them.
pub(super) use build::{build_flake_package, build_nix_expr, detect_reversed_patch};
pub use config::{UpdateConfig, UpdateParams, VersionConfig};
pub(super) use file_update::{
    update_cargo_hash, update_composer_deps_hash, update_nix_file, update_npm_deps_hash,
    update_nuget_deps_hash, update_vendor_hash,
};
pub(super) use flake::update_flake_package;
pub(super) use format::format_nix_file;
pub(super) use git::create_git_commit;
pub(super) use hash_workflows::{
    TestResult, build_with_patch_recovery, run_package_tests, update_cargo_hash_if_needed,
    update_composer_deps_hash_if_needed, update_npm_deps_hash_if_needed,
    update_nuget_deps_hash_if_needed, update_source_hash, update_vendor_hash_if_needed,
};
pub(super) use script::run_update_script;
use tracing::info;
pub(super) use variants::{get_default_variant, update_single_variant};

use crate::package::PackageMetadata;
use crate::vcs_sources::UpstreamSource;

/// Update a package from a specific file path.
/// Returns removed patches and the test result.
pub async fn update_from_file_path(
    eval_entry_point: String,
    attr_path: String,
    file_location: String,
    version_config: VersionConfig,
    update_config: UpdateConfig,
) -> anyhow::Result<(Vec<String>, TestResult)> {
    info!(
        "Starting generic update for {} at {}",
        attr_path, file_location
    );

    // Step 1: Extract package metadata
    let metadata = PackageMetadata::from_attr_path(&eval_entry_point, &attr_path).await?;
    info!("Current version: {}", metadata.version);

    // Step 2: Determine upstream source
    let upstream_source = if let Some(ref src_url) = metadata.src_url {
        UpstreamSource::from_url(src_url)
            .context("Source is not from a supported VCS platform (GitHub, GitLab, PyPI)")?
    } else if let Some(ref pname) = metadata.pname {
        UpstreamSource::PyPI {
            pname: pname.clone(),
        }
    } else {
        anyhow::bail!(
            "No source URL or pname found for package - cannot determine upstream source"
        );
    };

    info!("{}", upstream_source.description());

    // Use include-prereleases from passthru, or default to false
    let include_prereleases = metadata.include_prereleases.unwrap_or(false);

    // Prefer version-regex from passthru over CLI argument
    let version_regex = metadata
        .version_regex
        .as_deref()
        .or(version_config.version_regex.as_deref());

    // Step 3: Fetch best compatible release
    let best_release = upstream_source
        .get_compatible_release(
            &metadata.version,
            version_config.strategy,
            None,
            version_config.explicit_version.as_deref(),
            version_regex,
            include_prereleases,
        )
        .await?;

    let new_version = best_release.version();
    info!(
        "Found compatible version ({:?}): {} -> {}",
        version_config.strategy, metadata.version, new_version
    );

    // Step 4: Update source hash
    let actual_file_location: std::path::PathBuf = update_source_hash(
        &eval_entry_point,
        &attr_path,
        Path::new(&file_location),
        &metadata.version,
        new_version,
        metadata.output_hash.as_deref(),
    )
    .await?;

    // Step 5: Update dependency hashes (unless --src-only is set)
    if !update_config.src_only {
        // Update cargoHash for Rust packages
        update_cargo_hash_if_needed(
            &eval_entry_point,
            &attr_path,
            &actual_file_location,
            metadata.cargo_hash.as_deref(),
        )
        .await?;

        // Update vendorHash for Go packages
        update_vendor_hash_if_needed(
            &eval_entry_point,
            &attr_path,
            &actual_file_location,
            metadata.vendor_hash.as_deref(),
        )
        .await?;

        // Update npmDepsHash for Node.js packages
        update_npm_deps_hash_if_needed(
            &eval_entry_point,
            &attr_path,
            &actual_file_location,
            metadata.npm_deps_hash.as_deref(),
        )
        .await?;

        // Update nugetDeps for .NET packages
        update_nuget_deps_hash_if_needed(
            &eval_entry_point,
            &attr_path,
            &actual_file_location,
            metadata.nuget_deps_hash.as_deref(),
        )
        .await?;

        // Update composerDepsHash for PHP packages
        update_composer_deps_hash_if_needed(
            &eval_entry_point,
            &attr_path,
            &actual_file_location,
            metadata.composer_deps_hash.as_deref(),
        )
        .await?;
    } else {
        info!("Skipping dependency hash updates (--src-only flag set)");
    }

    // Step 6: Build with patch recovery
    let removed_patches =
        build_with_patch_recovery(&eval_entry_point, &attr_path, &actual_file_location).await?;

    // Step 7: Run passthru.tests if requested
    let test_result = run_package_tests(
        &eval_entry_point,
        &attr_path,
        update_config.run_passthru_tests,
    )
    .await?;

    // Step 8: Format the file if requested
    if update_config.format {
        format_nix_file(&actual_file_location).await?;
    }

    if test_result.failed() {
        info!(
            "⚠ Updated {} from {} to {} (passthru.tests failed)",
            attr_path, metadata.version, new_version
        );
    } else {
        info!(
            "✓ Successfully updated {} from {} to {}",
            attr_path, metadata.version, new_version
        );
    }

    // Step 9: Handle commit and PR creation
    pr::PostUpdateParams {
        attr_path: &attr_path,
        metadata: &metadata,
        new_version,
        commit: update_config.commit,
        create_pr: update_config.create_pr,
        upstream: update_config.upstream.clone(),
        fork: &update_config.fork,
        tests_passed: test_result.passed(),
        eval_entry_point: Some(&eval_entry_point),
        pr_enhancements: &update_config.pr_enhancements,
    }
    .execute()
    .await?;

    Ok((removed_patches, test_result))
}
