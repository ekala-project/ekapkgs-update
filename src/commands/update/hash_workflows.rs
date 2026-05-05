use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use super::{
    build_nix_expr, update_cargo_hash, update_composer_deps_hash, update_nix_file,
    update_npm_deps_hash, update_nuget_deps_hash, update_vendor_hash,
};
use crate::hash_discovery;

/// Update source hash using the invalid hash discovery pattern
pub async fn update_source_hash(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_version: &str,
    new_version: &str,
    old_hash: Option<&str>,
) -> anyhow::Result<PathBuf> {
    // Step 1: Update version in file with invalid hash
    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let actual_file_location = update_nix_file(
        eval_entry_point,
        attr_path,
        file_location,
        old_version,
        new_version,
        old_hash,
        Some(invalid_hash),
    )
    .await?;

    info!(
        "Updated version and set invalid hash in {}",
        actual_file_location.display()
    );

    // Step 2: Build source to get correct hash
    let (success, _stdout, stderr) =
        build_nix_expr(eval_entry_point, attr_path, Some("src")).await?;

    if success {
        warn!("Build succeeded with invalid hash - this shouldn't happen");
        anyhow::bail!("Expected hash mismatch error but build succeeded");
    }

    let correct_hash = hash_discovery::extract_hash(&stderr).ok_or_else(|| {
        anyhow::anyhow!("Could not extract correct hash from build error:\n{stderr}")
    })?;

    info!("Extracted correct hash: {}", correct_hash);

    // Step 3: Update hash with correct value
    update_nix_file(
        eval_entry_point,
        attr_path,
        &actual_file_location,
        new_version,
        new_version,
        Some(invalid_hash),
        Some(&correct_hash),
    )
    .await?;

    info!("Updated hash in {}", actual_file_location.display());

    // Step 4: Build source again to verify
    let (success, _stdout, stderr) =
        build_nix_expr(eval_entry_point, attr_path, Some("src")).await?;

    if !success {
        anyhow::bail!("Source build failed after hash update:\n{stderr}");
    }

    info!("Source build successful");

    Ok(actual_file_location)
}

/// Update cargoHash for Rust packages using the invalid hash discovery pattern
pub async fn update_cargo_hash_if_needed(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_cargo_hash: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(old_hash) = old_cargo_hash {
        info!("Detected Rust package, updating cargoHash");

        // Set invalid cargo hash
        let invalid_cargo_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_cargo_hash(file_location, old_hash, invalid_cargo_hash).await?;

        info!("Set invalid cargoHash in {}", file_location.display());

        // Build full package to get correct cargo hash
        let (success, _stdout, stderr) = build_nix_expr(eval_entry_point, attr_path, None).await?;

        if success {
            warn!("Build succeeded with invalid cargoHash - this shouldn't happen");
            anyhow::bail!("Expected cargoHash mismatch error but build succeeded");
        }

        let correct_cargo_hash = hash_discovery::extract_hash(&stderr).ok_or_else(|| {
            anyhow::anyhow!("Could not extract correct cargoHash from build error:\n{stderr}")
        })?;

        info!("Extracted correct cargoHash: {}", correct_cargo_hash);

        // Update cargoHash with correct value
        update_cargo_hash(file_location, invalid_cargo_hash, &correct_cargo_hash).await?;

        info!("Updated cargoHash in {}", file_location.display());
    }

    Ok(())
}

/// Update vendorHash for Go packages using the invalid hash discovery pattern
pub async fn update_vendor_hash_if_needed(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_vendor_hash: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(old_hash) = old_vendor_hash {
        info!("Detected Go package, updating vendorHash");

        // Set invalid vendor hash
        let invalid_vendor_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_vendor_hash(file_location, old_hash, invalid_vendor_hash).await?;

        info!("Set invalid vendorHash in {}", file_location.display());

        // Build full package to get correct vendor hash
        let (success, _stdout, stderr) = build_nix_expr(eval_entry_point, attr_path, None).await?;

        if success {
            warn!("Build succeeded with invalid vendorHash - this shouldn't happen");
            anyhow::bail!("Expected vendorHash mismatch error but build succeeded");
        }

        let correct_vendor_hash = hash_discovery::extract_hash(&stderr).ok_or_else(|| {
            anyhow::anyhow!("Could not extract correct vendorHash from build error:\n{stderr}")
        })?;

        info!("Extracted correct vendorHash: {}", correct_vendor_hash);

        // Update vendorHash with correct value
        update_vendor_hash(file_location, invalid_vendor_hash, &correct_vendor_hash).await?;

        info!("Updated vendorHash in {}", file_location.display());
    }

    Ok(())
}

/// Build package with automatic recovery from reversed patches
/// Returns a list of patch names that were removed during the build process
pub async fn build_with_patch_recovery(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
) -> anyhow::Result<Vec<String>> {
    use super::{build_nix_expr, detect_reversed_patch};
    use crate::rewrite::{
        is_patches_array_empty, remove_patch_from_array, remove_patches_attribute,
    };

    let mut removed_patches = Vec::new();

    loop {
        let (success, _stdout, stderr) = build_nix_expr(eval_entry_point, attr_path, None).await?;

        if success {
            // Build succeeded - check if patches array is now empty
            let content = tokio::fs::read_to_string(file_location).await?;
            if is_patches_array_empty(&content) {
                match remove_patches_attribute(&content) {
                    Ok(updated_content) => {
                        tokio::fs::write(file_location, updated_content).await?;
                        info!("Removed empty patches attribute");
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
            info!("Detected reversed/already-applied patch: {}", patch_name);

            // Read the file
            let content = tokio::fs::read_to_string(file_location).await?;

            // Remove the patch
            match remove_patch_from_array(&content, &patch_name) {
                Ok(updated_content) => {
                    // Write the updated content back
                    tokio::fs::write(file_location, updated_content).await?;
                    info!("Removed obsolete patch: {}", patch_name);
                    removed_patches.push(patch_name.clone());
                    // Continue loop to retry the build
                },
                Err(e) => {
                    warn!("Failed to remove patch {}: {}", patch_name, e);
                    // Can't remove the patch, return the original error
                    anyhow::bail!(
                        "Package build failed after update. Detected reversed/already-applied \
                         patch '{patch_name}' but couldn't remove it from the patches array: \
                         {e}\n\nBuild error:\n{stderr}"
                    );
                },
            }
        } else {
            // No reversed patch detected - this is a real build failure
            warn!("Full package build failed:\n{}", stderr);
            anyhow::bail!(
                "Package build failed after update with no reversed patches detected. This \
                 indicates a real build issue that needs manual intervention.\n\nBuild \
                 error:\n{stderr}"
            );
        }
    }

    Ok(removed_patches)
}

/// Run passthru.tests if they exist and return whether tests passed
pub async fn run_package_tests(
    eval_entry_point: &str,
    attr_path: &str,
    run_passthru_tests: bool,
    fail_on_test_failure: bool,
) -> anyhow::Result<bool> {
    use crate::nix::{has_passthru_tests, normalize_entry_point};

    let mut tests_passed = false;
    info!("Checking for passthru.tests...");

    if run_passthru_tests {
        let normalized_entry = normalize_entry_point(eval_entry_point);

        if has_passthru_tests(&normalized_entry, attr_path).await? {
            info!("Found {}.passthru.tests, building tests...", attr_path);

            // Build tests
            let (success, _stdout, stderr) =
                build_nix_expr(eval_entry_point, attr_path, Some("passthru.tests")).await?;

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

    Ok(tests_passed)
}

/// Update npmDepsHash for Node.js packages using the invalid hash discovery pattern
pub async fn update_npm_deps_hash_if_needed(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_npm_deps_hash: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(old_hash) = old_npm_deps_hash {
        info!("Detected Node.js package, updating npmDepsHash");

        // Set invalid npm deps hash
        let invalid_npm_deps_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_npm_deps_hash(file_location, old_hash, invalid_npm_deps_hash).await?;

        info!("Set invalid npmDepsHash in {}", file_location.display());

        // Build full package to get correct npm deps hash
        let (success, _stdout, stderr) = build_nix_expr(eval_entry_point, attr_path, None).await?;

        if success {
            warn!("Build succeeded with invalid npmDepsHash - this shouldn't happen");
            anyhow::bail!("Expected npmDepsHash mismatch error but build succeeded");
        }

        let correct_npm_deps_hash = hash_discovery::extract_hash(&stderr).ok_or_else(|| {
            anyhow::anyhow!("Could not extract correct npmDepsHash from build error:\n{stderr}")
        })?;

        info!("Extracted correct npmDepsHash: {}", correct_npm_deps_hash);

        // Update npmDepsHash with correct value
        update_npm_deps_hash(file_location, invalid_npm_deps_hash, &correct_npm_deps_hash).await?;

        info!("Updated npmDepsHash in {}", file_location.display());
    }

    Ok(())
}

/// Update nugetDeps hash for .NET packages using the invalid hash discovery pattern
pub async fn update_nuget_deps_hash_if_needed(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_nuget_deps_hash: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(old_hash) = old_nuget_deps_hash {
        info!("Detected .NET package, updating nugetDeps hash");

        // Set invalid nuget deps hash
        let invalid_nuget_deps_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_nuget_deps_hash(file_location, old_hash, invalid_nuget_deps_hash).await?;

        info!("Set invalid nugetDeps hash in {}", file_location.display());

        // Build full package to get correct nuget deps hash
        let (success, _stdout, stderr) = build_nix_expr(eval_entry_point, attr_path, None).await?;

        if success {
            warn!("Build succeeded with invalid nugetDeps hash - this shouldn't happen");
            anyhow::bail!("Expected nugetDeps hash mismatch error but build succeeded");
        }

        let correct_nuget_deps_hash = hash_discovery::extract_hash(&stderr).ok_or_else(|| {
            anyhow::anyhow!("Could not extract correct nugetDeps hash from build error:\n{stderr}")
        })?;

        info!(
            "Extracted correct nugetDeps hash: {}",
            correct_nuget_deps_hash
        );

        // Update nugetDeps hash with correct value
        update_nuget_deps_hash(
            file_location,
            invalid_nuget_deps_hash,
            &correct_nuget_deps_hash,
        )
        .await?;

        info!("Updated nugetDeps hash in {}", file_location.display());
    }

    Ok(())
}

/// Update composerDepsHash for PHP packages using the invalid hash discovery pattern
pub async fn update_composer_deps_hash_if_needed(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_composer_deps_hash: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(old_hash) = old_composer_deps_hash {
        info!("Detected PHP package, updating composerDepsHash");

        // Set invalid composer deps hash
        let invalid_composer_deps_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_composer_deps_hash(file_location, old_hash, invalid_composer_deps_hash).await?;

        info!(
            "Set invalid composerDepsHash in {}",
            file_location.display()
        );

        // Build full package to get correct composer deps hash
        let (success, _stdout, stderr) = build_nix_expr(eval_entry_point, attr_path, None).await?;

        if success {
            warn!("Build succeeded with invalid composerDepsHash - this shouldn't happen");
            anyhow::bail!("Expected composerDepsHash mismatch error but build succeeded");
        }

        let correct_composer_deps_hash =
            hash_discovery::extract_hash(&stderr).ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not extract correct composerDepsHash from build error:\n{stderr}"
                )
            })?;

        info!(
            "Extracted correct composerDepsHash: {}",
            correct_composer_deps_hash
        );

        // Update composerDepsHash with correct value
        update_composer_deps_hash(
            file_location,
            invalid_composer_deps_hash,
            &correct_composer_deps_hash,
        )
        .await?;

        info!("Updated composerDepsHash in {}", file_location.display());
    }

    Ok(())
}
