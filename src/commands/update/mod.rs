mod build;
mod config;
mod file_update;
mod flake;
mod format;
mod git;
mod hash_workflows;
mod pr;
mod script;
mod variants;

use std::path::Path;

use anyhow::Context;
pub use build::{build_flake_package, build_nix_expr, detect_reversed_patch};
pub use config::{FlakeConfig, UpdateConfig, UpdateParams, VariantConfig, VersionConfig};
pub use file_update::{
    update_cargo_hash, update_composer_deps_hash, update_nix_file, update_npm_deps_hash,
    update_nuget_deps_hash, update_vendor_hash,
};
pub use flake::update_flake_package;
pub use format::format_nix_file;
pub use git::create_git_commit;
pub use hash_workflows::{
    build_with_patch_recovery, run_package_tests, update_cargo_hash_if_needed,
    update_composer_deps_hash_if_needed, update_npm_deps_hash_if_needed,
    update_nuget_deps_hash_if_needed, update_source_hash, update_vendor_hash_if_needed,
};
pub use pr::handle_commit_or_pr;
pub use script::run_update_script;
use tracing::{debug, info, warn};
pub use variants::{get_default_variant, update_single_variant};

use crate::nix::{
    eval_nix_expr, get_variants_list, is_many_variants_package, normalize_entry_point,
};
use crate::package::PackageMetadata;
use crate::variant_strategy::{infer_strategy_from_variant, is_variant_pinned};
use crate::vcs_sources::UpstreamSource;

/// Main update entry point
pub async fn update(params: UpdateParams) -> anyhow::Result<()> {
    let UpdateParams {
        file,
        attr_path,
        ignore_update_script,
        override_filename,
        system: _system,
        update_config,
        version_config,
        variant_config,
        flake_config,
    } = params;

    let strategy = version_config.strategy;
    info!("Using semver strategy: {:?}", strategy);

    // Handle flake mode
    if flake_config.enabled {
        info!("Flake mode enabled");
        return update_flake_package(
            file,
            attr_path,
            flake_config.output,
            version_config,
            update_config,
            override_filename,
        )
        .await;
    }

    // Check if this is a mkManyVariants package
    let is_many_variants = is_many_variants_package(&file, &attr_path).await?;

    if is_many_variants {
        info!("{} is a mkManyVariants package", attr_path);

        // Determine which variants to update
        let variants_to_update = if let Some(ref specific_variant) = variant_config.variant {
            // Update only the specified variant
            vec![specific_variant.clone()]
        } else if variant_config.all_variants {
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
                update_config.clone(),
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
    // Determine file location: use override if provided, otherwise get from meta.position
    let expr_file_path = if let Some(ref override_file) = override_filename {
        info!("Using override filename: {}", override_file);
        override_file.clone()
    } else {
        debug!("Attempting to locate package definition...");
        let normalized_entry = normalize_entry_point(&file);
        let position_expr = format!(
            "with import {} {{ }}; {}.meta.position",
            normalized_entry, attr_path
        );

        eval_nix_expr(&position_expr).await.and_then(|position| {
            if position.is_empty() {
                anyhow::bail!("Empty position returned from meta.position");
            }
            // Parse position string (format: "file:line")
            let (file_path, _line_str) = position
                .rsplit_once(':')
                .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {}", position))?;
            Ok(file_path.to_string())
        })?
    };

    let _removed_patches = update_from_file_path(
        file,
        attr_path,
        expr_file_path,
        version_config,
        update_config,
        false, // Don't fail on test errors for update command
    )
    .await?;

    Ok(())
}

/// Update a package from a specific file path
/// Returns a list of patches that were removed during the update
pub async fn update_from_file_path(
    eval_entry_point: String,
    attr_path: String,
    file_location: String,
    version_config: VersionConfig,
    update_config: UpdateConfig,
    fail_on_test_failure: bool,
) -> anyhow::Result<Vec<String>> {
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

    // Step 3: Fetch best compatible release
    let best_release = upstream_source
        .get_compatible_release(
            &metadata.version,
            version_config.strategy,
            None,
            version_config.explicit_version.as_deref(),
            version_config.version_regex.as_deref(),
        )
        .await?;

    let new_version = UpstreamSource::get_version(&best_release);
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
        &new_version,
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

    // Step 8: Run passthru.tests if requested
    let tests_passed = run_package_tests(
        &eval_entry_point,
        &attr_path,
        update_config.run_passthru_tests,
        fail_on_test_failure,
    )
    .await?;

    // Step 9: Format the file if requested
    if update_config.format {
        format_nix_file(&actual_file_location).await?;
    }

    info!(
        "✓ Successfully updated {} from {} to {}",
        attr_path, metadata.version, new_version
    );

    // Step 10: Handle commit and PR creation
    handle_commit_or_pr(pr::PostUpdateParams {
        attr_path: &attr_path,
        metadata: &metadata,
        new_version: &new_version,
        commit: update_config.commit,
        create_pr: update_config.create_pr,
        upstream: update_config.upstream,
        fork: &update_config.fork,
        tests_passed,
    })
    .await?;

    Ok(removed_patches)
}
