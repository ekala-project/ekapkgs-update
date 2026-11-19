mod build;
mod components;
mod config;
pub mod errors;
mod file_update;
mod flake;
mod fod_platforms;
mod fod_refresh;
mod format;
mod git;
mod hash_workflows;
pub mod instrumentation;
mod pins;
mod pr;
mod script;
pub mod types;
mod variants;

use std::collections::HashSet;
use std::path::Path;

// Externally-consumed re-exports (cli.rs, commands::pr_enhancements, integration tests).
pub use build::{build_and_get_outputs, get_extra_nix_build_args, set_extra_nix_build_args};
pub use components::ComponentUpdate;
pub use pr::components_commit_lines;
// Sibling-only re-exports kept narrow with `pub(super)` so child modules can
// reach them via `super::`, but no other crate module sees them.
pub(super) use build::{
    build_flake_package, build_nix_expr, build_nix_expr_with, detect_reversed_patch,
};
pub use config::{UpdateConfig, UpdateParams, VersionConfig};
pub(super) use file_update::{ReleaseTag, update_hash_attr, update_nix_file};
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
use tracing::{info, warn};
pub(super) use variants::{
    add_new_variant, discover_new_variants, fill_keep_variants, get_default_variant,
    update_single_variant,
};

use crate::package::{PackageMetadata, PackageQuery};
use crate::vcs_sources::UpstreamSource;

/// [`update_from_file_path`] was given an explicit target version that the
/// package already has (or exceeds), so there is nothing to update.
///
/// In branch mode, several attributes built from one file (`arro3-core`,
/// `arro3-io`, `arro3-compute`) are queued separately; the first update bumps
/// the shared file and the others find it at the target. Callers can
/// `downcast_ref` the error to tell this apart from a failure.
#[derive(Debug, Clone, thiserror::Error)]
#[error("already at version {current} (requested {target})")]
pub struct AlreadyAtVersion {
    /// Version found in the tree
    pub current: String,
    /// Requested explicit version
    pub target: String,
}

/// What a successful [`update_from_file_path`] did besides the version bump.
#[derive(Debug)]
pub struct UpdateOutcome {
    /// Patches removed because they no longer applied
    pub removed_patches: Vec<String>,
    /// Result of `passthru.tests`
    pub test_result: TestResult,
    /// Components (`passthru.ekapkgs-update.components`) bumped along
    pub components: Vec<ComponentUpdate>,
}

/// Update a package from a specific file path.
///
/// Each workflow step is recorded as an `update_phases` row through
/// `update_config.tracker` (a no-op unless the caller enabled it).
pub async fn update_from_file_path(
    eval_entry_point: String,
    attr_path: String,
    file_location: String,
    version_config: VersionConfig,
    update_config: UpdateConfig,
) -> anyhow::Result<UpdateOutcome> {
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

    // An explicit target the tree has already reached is nothing to do, e.g.
    // because another attribute built from the same file was updated first.
    if let Some(target) = version_config.explicit_version.as_deref() {
        if crate::database::version_at_least(&metadata.version, target) {
            return Err(AlreadyAtVersion {
                current: metadata.version.clone(),
                target: target.to_owned(),
            }
            .into());
        }
    }

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

    // With a `version-regex` the release was selected by the version it
    // captures (`v(.*)-release`), so that is the version written, not the tag
    // with its prefix stripped. A tag matched by name alone falls back.
    let new_version = best_release
        .resolved_version(version_regex)
        .unwrap_or_else(|_| best_release.version().to_owned());
    let new_version = new_version.as_str();
    info!(
        "Found compatible version ({:?}): {} -> {}",
        version_config.strategy, metadata.version, new_version
    );

    // Snapshot the `.drv` closure before any file is edited: FODs whose `.drv`
    // the bump changes (`moby-src`, vendored dependencies under custom names)
    // are found by comparing against it, and their hashes refreshed.
    let closure_before =
        match fod_refresh::ClosureSnapshot::capture(&eval_entry_point, &attr_path).await {
            Ok(snapshot) => Some(snapshot),
            Err(e) => {
                warn!(
                    "{attr_path}: could not snapshot the derivation closure; hashes of other \
                     fixed-output derivations will not be refreshed: {e:#}"
                );
                None
            },
        };
    // Hashes this update discovered; FODs carrying them are up to date.
    let mut confirmed_hashes = HashSet::new();

    // Step 4: Update source hash (and platform-specific source hashes), bump
    // components, and refresh other changed sources
    let (actual_file_location, component_updates): (std::path::PathBuf, Vec<ComponentUpdate>) =
        tracker
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
                    update_platform_hashes(&eval_entry_point, &attr_path, &location, platforms)
                        .await?;
                }
                if let Some(hash) = PackageQuery::new(&eval_entry_point, &attr_path)
                    .get_attr("src.outputHash")
                    .await
                {
                    // Per-system source hashes without platform-hashes
                    // (mistral-vibe's wheelHashes)
                    if metadata.platform_hashes.is_none() {
                        let systems = fod_platforms::refresh_other_systems(
                            &eval_entry_point,
                            &attr_path,
                            &location,
                            &hash,
                        )
                        .await?;
                        if !systems.is_empty() {
                            info!(
                                "{attr_path}: also refreshed the source hash for {}",
                                systems.join(", ")
                            );
                        }
                    }
                    confirmed_hashes.insert(hash);
                }

                // Other sources first: a component may be pinned by a file in
                // one of them (docker's runc by moby's Dockerfile).
                if let Some(before) = &closure_before {
                    refresh_fods(
                        &eval_entry_point,
                        &attr_path,
                        &location,
                        before,
                        &mut confirmed_hashes,
                        fod_refresh::FodScope::Sources,
                    )
                    .await?;
                }

                let components =
                    components::update_components(&eval_entry_point, &attr_path, &location)
                        .await
                        .unwrap_or_else(|e| {
                            warn!("{attr_path}: components not updated: {e:#}");
                            Vec::new()
                        });

                if let (Some(before), false) = (&closure_before, components.is_empty()) {
                    refresh_fods(
                        &eval_entry_point,
                        &attr_path,
                        &location,
                        before,
                        &mut confirmed_hashes,
                        fod_refresh::FodScope::Sources,
                    )
                    .await?;
                }
                Ok((location, components))
            })
            .await?;

    // Step 5: Update dependency hashes (unless --src-only is set): other
    // changed FODs, the known attributes, then FODs built from those
    let has_dependency_hashes = metadata.cargo_hash.is_some()
        || metadata.vendor_hash.is_some()
        || metadata.npm_deps_hash.is_some()
        || metadata.nuget_deps_hash.is_some()
        || metadata.composer_deps_hash.is_some();
    if update_config.src_only {
        info!("Skipping dependency hash updates (--src-only flag set)");
    } else if has_dependency_hashes || closure_before.is_some() {
        tracker
            .run(UpdatePhase::DependencyHashUpdate, async {
                let file = actual_file_location.as_path();
                // Other changed FODs first: a dependency FOD may be built
                // from one (kapacitor's go-modules need libflux, whose cargo
                // vendor FOD changed with the flux component). The FODs of the
                // known attributes are left to their own discovery below.
                if let Some(before) = &closure_before {
                    let known = [
                        &metadata.cargo_hash,
                        &metadata.vendor_hash,
                        &metadata.npm_deps_hash,
                        &metadata.nuget_deps_hash,
                        &metadata.composer_deps_hash,
                    ];
                    let mut deferred = confirmed_hashes.clone();
                    deferred.extend(known.into_iter().flatten().cloned());
                    refresh_fods(
                        &eval_entry_point,
                        &attr_path,
                        file,
                        before,
                        &mut deferred,
                        fod_refresh::FodScope::All,
                    )
                    .await?;
                    confirmed_hashes.extend(
                        deferred
                            .into_iter()
                            .filter(|hash| !known.iter().any(|k| k.as_ref() == Some(hash))),
                    );
                }

                let discovered = [
                    // Rust
                    update_cargo_hash_if_needed(
                        &eval_entry_point,
                        &attr_path,
                        file,
                        metadata.cargo_hash.as_deref(),
                    )
                    .await?,
                    // Go
                    update_vendor_hash_if_needed(
                        &eval_entry_point,
                        &attr_path,
                        file,
                        metadata.vendor_hash.as_deref(),
                    )
                    .await?,
                    // Node.js
                    update_npm_deps_hash_if_needed(
                        &eval_entry_point,
                        &attr_path,
                        file,
                        metadata.npm_deps_hash.as_deref(),
                    )
                    .await?,
                    // .NET
                    update_nuget_deps_hash_if_needed(
                        &eval_entry_point,
                        &attr_path,
                        file,
                        metadata.nuget_deps_hash.as_deref(),
                    )
                    .await?,
                    // PHP
                    update_composer_deps_hash_if_needed(
                        &eval_entry_point,
                        &attr_path,
                        file,
                        metadata.composer_deps_hash.as_deref(),
                    )
                    .await?,
                ];
                confirmed_hashes.extend(discovered.into_iter().flatten());

                if let Some(before) = &closure_before {
                    refresh_fods(
                        &eval_entry_point,
                        &attr_path,
                        file,
                        before,
                        &mut confirmed_hashes,
                        fod_refresh::FodScope::All,
                    )
                    .await?;
                }
                Ok(())
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
        components: &component_updates,
    };
    if update_config.commit || update_config.create_pr {
        tracker
            .run(UpdatePhase::PrCreation, post_update.execute())
            .await?;
    } else {
        post_update.execute().await?;
    }

    Ok(UpdateOutcome {
        removed_patches,
        test_result,
        components: component_updates,
    })
}

/// [`fod_refresh::refresh_changed_fods`] over the directory of `file`.
async fn refresh_fods(
    eval_entry_point: &str,
    attr_path: &str,
    file: &Path,
    before: &fod_refresh::ClosureSnapshot,
    confirmed: &mut HashSet<String>,
    scope: fod_refresh::FodScope,
) -> anyhow::Result<()> {
    let package_dir = file
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", file.display()))?;
    let refreshed = fod_refresh::refresh_changed_fods(
        eval_entry_point,
        attr_path,
        package_dir,
        before,
        confirmed,
        scope,
    )
    .await?;
    if !refreshed.is_empty() {
        info!("{attr_path}: refreshed hashes of {}", refreshed.join(", "));
    }
    Ok(())
}
