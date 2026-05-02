use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use super::variants::find_version_in_siblings;
use crate::nix::is_many_variants_package;
use crate::rewrite::find_and_update_attr;

/// Update version and hash in a Nix file
///
/// Returns the actual file path that was updated (may differ from input due to mkManyVariants)
pub async fn update_nix_file(
    eval_entry_point: &str,
    attr_path: &str,
    file_path: &Path,
    old_version: &str,
    new_version: &str,
    old_hash: Option<&str>,
    new_hash: Option<&str>,
) -> anyhow::Result<PathBuf> {
    debug!(
        "Updating Nix file at {} using AST manipulation",
        file_path.display()
    );
    let content = tokio::fs::read_to_string(file_path).await?;

    // Try to update the version attribute
    let (updated_content, actual_file_path) =
        match find_and_update_attr(&content, "version", new_version, Some(old_version)) {
            Ok(content) => {
                debug!(
                    "Updated version attribute: {} -> {}",
                    old_version, new_version
                );
                (content, file_path.to_path_buf())
            },
            Err(e) if e.to_string().contains("not found") => {
                // Version not found - check if this is a mkManyVariants package
                debug!(
                    "Version not found in {}, checking if mkManyVariants",
                    file_path.display()
                );

                if is_many_variants_package(eval_entry_point, attr_path).await? {
                    // This is a mkManyVariants package - search sibling files
                    let file_path_str = file_path
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("Invalid UTF-8 in path"))?;
                    match find_version_in_siblings(file_path_str, old_version, old_hash).await? {
                        Some(sibling_path) => {
                            info!("Using mkManyVariants file: {}", sibling_path);
                            let sibling_content = tokio::fs::read_to_string(&sibling_path).await?;

                            // Try simple string replacement for mkManyVariants files
                            let updated = sibling_content.replace(old_version, new_version);
                            (updated, PathBuf::from(sibling_path))
                        },
                        None => {
                            // No sibling found, return original error
                            return Err(e);
                        },
                    }
                } else {
                    // Not a mkManyVariants package, return original error
                    return Err(e);
                }
            },
            Err(e) => return Err(e),
        };

    // Update hash if provided
    let final_content = if let (Some(old_h), Some(new_h)) = (old_hash, new_hash) {
        // For mkManyVariants, use simple string replacement
        // For normal files, use AST-based replacement
        if actual_file_path.as_path() != file_path {
            // mkManyVariants file - use string replacement
            let result = updated_content.replace(old_h, new_h);
            debug!(
                "Updated hash using string replacement: {} -> {}",
                old_h, new_h
            );
            result
        } else {
            // Normal file - try AST-based replacement
            let hash_attrs = ["hash", "sha256", "outputHash", "src-hash"];

            hash_attrs
                .iter()
                .find_map(|&attr_name| {
                    find_and_update_attr(&updated_content, attr_name, new_h, Some(old_h))
                        .ok()
                        .map(|new_content| {
                            debug!("Updated {} attribute: {} -> {}", attr_name, old_h, new_h);
                            new_content
                        })
                })
                .unwrap_or_else(|| {
                    warn!("Could not find hash attribute to update in Nix file");
                    updated_content.clone()
                })
        }
    } else {
        updated_content
    };

    // Write back to file
    tokio::fs::write(&actual_file_path, final_content).await?;
    Ok(actual_file_path)
}

/// Update cargoHash attribute in Nix file
pub async fn update_cargo_hash(
    file_path: &Path,
    old_hash: &str,
    new_hash: &str,
) -> anyhow::Result<()> {
    debug!(
        "Updating cargoHash in {} using AST manipulation",
        file_path.display()
    );
    let content = tokio::fs::read_to_string(file_path).await?;

    let updated_content = find_and_update_attr(&content, "cargoHash", new_hash, Some(old_hash))?;
    debug!("Updated cargoHash attribute: {} -> {}", old_hash, new_hash);

    tokio::fs::write(file_path, updated_content).await?;
    Ok(())
}

/// Update vendorHash attribute in Nix file
pub async fn update_vendor_hash(
    file_path: &Path,
    old_hash: &str,
    new_hash: &str,
) -> anyhow::Result<()> {
    debug!(
        "Updating vendorHash in {} using AST manipulation",
        file_path.display()
    );
    let content = tokio::fs::read_to_string(file_path).await?;

    let updated_content = find_and_update_attr(&content, "vendorHash", new_hash, Some(old_hash))?;
    debug!("Updated vendorHash attribute: {} -> {}", old_hash, new_hash);

    tokio::fs::write(file_path, updated_content).await?;
    Ok(())
}

/// Update npmDepsHash attribute in Nix file
pub async fn update_npm_deps_hash(
    file_path: &Path,
    old_hash: &str,
    new_hash: &str,
) -> anyhow::Result<()> {
    debug!(
        "Updating npmDepsHash in {} using AST manipulation",
        file_path.display()
    );
    let content = tokio::fs::read_to_string(file_path).await?;

    let updated_content = find_and_update_attr(&content, "npmDepsHash", new_hash, Some(old_hash))?;
    debug!(
        "Updated npmDepsHash attribute: {} -> {}",
        old_hash, new_hash
    );

    tokio::fs::write(file_path, updated_content).await?;
    Ok(())
}

/// Update nugetDeps attribute hash in Nix file
pub async fn update_nuget_deps_hash(
    file_path: &Path,
    old_hash: &str,
    new_hash: &str,
) -> anyhow::Result<()> {
    debug!(
        "Updating nugetDeps hash in {} using AST manipulation",
        file_path.display()
    );
    let content = tokio::fs::read_to_string(file_path).await?;

    // nugetDeps typically has an outputHash attribute
    let updated_content = find_and_update_attr(&content, "nugetDeps", new_hash, Some(old_hash))?;
    debug!("Updated nugetDeps attribute: {} -> {}", old_hash, new_hash);

    tokio::fs::write(file_path, updated_content).await?;
    Ok(())
}

/// Update composerDepsHash attribute in Nix file
pub async fn update_composer_deps_hash(
    file_path: &Path,
    old_hash: &str,
    new_hash: &str,
) -> anyhow::Result<()> {
    debug!(
        "Updating composerDepsHash in {} using AST manipulation",
        file_path.display()
    );
    let content = tokio::fs::read_to_string(file_path).await?;

    let updated_content =
        find_and_update_attr(&content, "composerDepsHash", new_hash, Some(old_hash))?;
    debug!(
        "Updated composerDepsHash attribute: {} -> {}",
        old_hash, new_hash
    );

    tokio::fs::write(file_path, updated_content).await?;
    Ok(())
}
