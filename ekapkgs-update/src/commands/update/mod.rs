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

// Externally-consumed re-exports (cli.rs, commands::pr_enhancements, integration tests).
pub use build::{build_and_get_outputs, get_extra_nix_build_args, set_extra_nix_build_args};
// Sibling-only re-exports kept narrow with `pub(super)` so child modules can
// reach them via `super::`, but no other crate module sees them.
pub(super) use build::{build_flake_package, build_nix_expr, detect_reversed_patch};
pub use config::{UpdateConfig, UpdateParams, VersionConfig};
pub(super) use file_update::{
    ReleaseTag, update_cargo_hash, update_composer_deps_hash, update_nix_file,
    update_npm_deps_hash, update_nuget_deps_hash, update_vendor_hash,
};
pub(super) use flake::update_flake_package;
pub(super) use format::format_nix_file;
pub(super) use git::create_git_commit;
pub(super) use hash_workflows::{
    TestResult, build_with_patch_recovery, run_package_tests, update_cargo_hash_if_needed,
    update_composer_deps_hash_if_needed, update_npm_deps_hash_if_needed,
    update_nuget_deps_hash_if_needed, update_platform_hashes, update_source_hash,
    update_vendor_hash_if_needed,
};
pub(super) use script::run_update_script;
use tracing::info;
pub(super) use variants::{
    add_new_variant, discover_new_variants, fill_keep_variants, get_default_variant,
    update_single_variant,
};

use crate::package::PackageMetadata;
use crate::vcs_sources::UpstreamSource;

/// Update a package from a specific file path.
/// Returns removed patches and the test result.
///
/// Each workflow step is recorded as an `update_phases` row through
/// `update_config.tracker` (a no-op unless the caller enabled it).
pub async fn update_from_file_path(
    eval_entry_point: String,
    attr_path: String,
    file_location: String,
    version_config: VersionConfig,
    update_config: UpdateConfig,
) -> anyhow::Result<(Vec<String>, TestResult)> {
    use self::types::UpdatePhase;

    info!(
        "Starting generic update for {} at {}",
        attr_path, file_location
    );
    let tracker = update_config.tracker.clone();

    // Step 1: Extract package metadata
    let metadata = tracker
        .run(
            UpdatePhase::MetadataExtraction,
            PackageMetadata::from_attr_path(&eval_entry_point, &attr_path),
        )
        .await?;
    info!("Current version: {}", metadata.version);

    // Step 2: Determine upstream source
    let upstream_source = tracker
        .run(UpdatePhase::SourceDiscovery, async {
            metadata.resolve_upstream_source()
        })
        .await?;

    info!("{}", upstream_source.description());

    // Use include-prereleases from passthru, or default to false
    let include_prereleases = metadata.include_prereleases.unwrap_or(false);

    // Prefer version-regex from passthru over CLI argument
    let version_regex = metadata
        .version_regex
        .as_deref()
        .or(version_config.version_regex.as_deref());

    // Step 3: Fetch best compatible release
    let best_release = tracker
        .run(
            UpdatePhase::VersionSelection,
            upstream_source.get_compatible_release(
                &metadata.version,
                version_config.strategy,
                None,
                version_config.explicit_version.as_deref(),
                version_regex,
                include_prereleases,
            ),
        )
        .await?;

    let new_version = best_release.version();
    info!(
        "Found compatible version ({:?}): {} -> {}",
        version_config.strategy, metadata.version, new_version
    );

    // Step 4: Update source hash (and platform-specific source hashes)
    let actual_file_location: std::path::PathBuf = tracker
        .run(UpdatePhase::SourceHashUpdate, async {
            let location = update_source_hash(
                &eval_entry_point,
                &attr_path,
                Path::new(&file_location),
                &metadata.version,
                new_version,
                metadata.output_hash.as_deref(),
                upstream_source.git_repo_name().map(|repo| ReleaseTag {
                    name: &best_release.tag_name,
                    repo,
                }),
                match &upstream_source {
                    UpstreamSource::PyPI { pname } => Some(pname.as_str()),
                    _ => None,
                },
            )
            .await?;

            if let Some(ref platforms) = metadata.platform_hashes {
                info!(
                    "Discovering source hashes for {} platforms: {:?}",
                    platforms.len(),
                    platforms
                );
                update_platform_hashes(&eval_entry_point, &attr_path, &location, platforms).await?;
            }
            Ok(location)
        })
        .await?;

    // Step 5: Update dependency hashes (unless --src-only is set)
    let has_dependency_hashes = metadata.cargo_hash.is_some()
        || metadata.vendor_hash.is_some()
        || metadata.npm_deps_hash.is_some()
        || metadata.nuget_deps_hash.is_some()
        || metadata.composer_deps_hash.is_some();
    if update_config.src_only {
        info!("Skipping dependency hash updates (--src-only flag set)");
    } else if has_dependency_hashes {
        tracker
            .run(UpdatePhase::DependencyHashUpdate, async {
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
                .await
            })
            .await?;
    }

    // Step 6: Build with patch recovery
    let removed_patches = tracker
        .run(
            UpdatePhase::Build,
            build_with_patch_recovery(&eval_entry_point, &attr_path, &actual_file_location),
        )
        .await?;

    // Step 7: Run passthru.tests if requested
    let test_result = if update_config.run_passthru_tests {
        let handle = tracker.start(UpdatePhase::Testing).await;
        match run_package_tests(&eval_entry_point, &attr_path, true).await {
            Ok(result) => {
                match &result {
                    TestResult::Passed => handle.success().await,
                    TestResult::Failed(output) => {
                        handle
                            .fail(errors::UpdateError::TestError {
                                phase: UpdatePhase::Testing,
                                test_name: Some(format!("{attr_path}.passthru.tests")),
                                output: output.clone(),
                                exit_code: 1,
                            })
                            .await;
                    },
                    TestResult::NoTests | TestResult::NotRequested => handle.skipped().await,
                }
                result
            },
            Err(e) => {
                handle
                    .fail(errors::UpdateError::classify(
                        UpdatePhase::Testing,
                        &attr_path,
                        &e,
                    ))
                    .await;
                return Err(e);
            },
        }
    } else {
        TestResult::NotRequested
    };

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
    let post_update = pr::PostUpdateParams {
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
    };
    if update_config.commit || update_config.create_pr {
        tracker
            .run(UpdatePhase::PrCreation, post_update.execute())
            .await?;
    } else {
        post_update.execute().await?;
    }

    Ok((removed_patches, test_result))
}
