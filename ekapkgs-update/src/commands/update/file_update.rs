use std::path::{Path, PathBuf};

use anyhow::Context;
use tracing::{debug, info, warn};

use super::variants::find_version_in_siblings;
use crate::commands::migrate::{convert_to_final_attrs_pattern_generic, fix_closing_brace};
use crate::nix::is_many_variants_package;
use crate::rewrite::{
    find_and_update_attr, needs_final_attrs_conversion, try_fixup_stale_rev, try_update_rev_attr,
};

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
    tag_name: Option<&str>,
) -> anyhow::Result<PathBuf> {
    debug!(
        "Updating Nix file at {} using AST manipulation",
        file_path.display()
    );
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("read nix file {}", file_path.display()))?;

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
            Err(e) if e.is_not_found() => {
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
                            let sibling_content = tokio::fs::read_to_string(&sibling_path)
                                .await
                                .with_context(|| format!("read sibling file {sibling_path}"))?;

                            // Try simple string replacement for mkManyVariants files
                            let updated = sibling_content.replace(old_version, new_version);
                            (updated, PathBuf::from(sibling_path))
                        },
                        None => {
                            // No sibling found, return original error
                            return Err(e.into());
                        },
                    }
                } else {
                    // Not a mkManyVariants package, return original error
                    return Err(e.into());
                }
            },
            Err(e) => return Err(e.into()),
        };

    // Try to update the rev attribute if it exists (non-blocking)
    // Only for normal files, not mkManyVariants (which use string replacement)
    let updated_content = if actual_file_path.as_path() == file_path {
        match try_update_rev_attr(&updated_content, old_version, new_version) {
            Ok(content) => {
                debug!(
                    "Updated rev attribute based on version change: {} -> {}",
                    old_version, new_version
                );
                content
            },
            Err(e) if e.is_not_found() => {
                // Check if this is a commit SHA that needs fixing via finalAttrs
                if let Some(tag) = tag_name {
                    try_fixup_commit_sha_rev(&updated_content, tag, new_version)
                } else {
                    debug!("No rev attribute to update (or skipped): {}", e);
                    updated_content
                }
            },
            Err(e) => {
                warn!("Failed to update rev attribute: {}", e);
                updated_content
            },
        }
    } else {
        // For mkManyVariants, we already did string replacement above
        updated_content
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
                        .inspect(|_| {
                            debug!("Updated {} attribute: {} -> {}", attr_name, old_h, new_h);
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
    tokio::fs::write(&actual_file_path, final_content)
        .await
        .with_context(|| format!("write nix file {}", actual_file_path.display()))?;
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
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("read nix file {}", file_path.display()))?;

    let updated_content = find_and_update_attr(&content, "cargoHash", new_hash, Some(old_hash))?;
    debug!("Updated cargoHash attribute: {} -> {}", old_hash, new_hash);

    tokio::fs::write(file_path, updated_content)
        .await
        .with_context(|| format!("write nix file {}", file_path.display()))?;
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
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("read nix file {}", file_path.display()))?;

    let updated_content = find_and_update_attr(&content, "vendorHash", new_hash, Some(old_hash))?;
    debug!("Updated vendorHash attribute: {} -> {}", old_hash, new_hash);

    tokio::fs::write(file_path, updated_content)
        .await
        .with_context(|| format!("write nix file {}", file_path.display()))?;
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
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("read nix file {}", file_path.display()))?;

    let updated_content = find_and_update_attr(&content, "npmDepsHash", new_hash, Some(old_hash))?;
    debug!(
        "Updated npmDepsHash attribute: {} -> {}",
        old_hash, new_hash
    );

    tokio::fs::write(file_path, updated_content)
        .await
        .with_context(|| format!("write nix file {}", file_path.display()))?;
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
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("read nix file {}", file_path.display()))?;

    // nugetDeps typically has an outputHash attribute
    let updated_content = find_and_update_attr(&content, "nugetDeps", new_hash, Some(old_hash))?;
    debug!("Updated nugetDeps attribute: {} -> {}", old_hash, new_hash);

    tokio::fs::write(file_path, updated_content)
        .await
        .with_context(|| format!("write nix file {}", file_path.display()))?;
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
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("read nix file {}", file_path.display()))?;

    let updated_content =
        find_and_update_attr(&content, "composerDepsHash", new_hash, Some(old_hash))?;
    debug!(
        "Updated composerDepsHash attribute: {} -> {}",
        old_hash, new_hash
    );

    tokio::fs::write(file_path, updated_content)
        .await
        .with_context(|| format!("write nix file {}", file_path.display()))?;
    Ok(())
}

/// Attempt to fix a stale commit-SHA `rev` by converting to `finalAttrs`
/// (if needed) and rewriting `rev` to reference the version via the tag format.
///
/// Returns the fixed content on success, or the original content unchanged
/// if the fixup is not applicable.
fn try_fixup_commit_sha_rev(content: &str, tag_name: &str, new_version: &str) -> String {
    use crate::rewrite::rev_update::{extract_rev_value, is_likely_commit_sha};

    // Only attempt fixup when rev is actually a commit SHA
    let is_sha = extract_rev_value(content)
        .map(|rev| is_likely_commit_sha(&rev))
        .unwrap_or(false);
    if !is_sha {
        debug!("No rev attribute to update (not a commit SHA)");
        return content.to_owned();
    }

    info!("Detected stale commit SHA in rev, attempting to fix");

    let mut fixable = content.to_owned();

    // Convert to finalAttrs if the builder doesn't use rec or finalAttrs
    if needs_final_attrs_conversion(&fixable) {
        match convert_to_final_attrs_pattern_generic(&fixable).and_then(|c| fix_closing_brace(&c)) {
            Ok(converted) => {
                info!("Converted to finalAttrs pattern for version reference");
                fixable = converted;
            },
            Err(e) => {
                warn!("Could not convert to finalAttrs: {}", e);
                return content.to_owned();
            },
        }
    }

    // Rewrite rev to reference version
    match try_fixup_stale_rev(&fixable, tag_name, new_version) {
        Ok(fixed) => {
            info!("Fixed stale rev to reference version via tag format");
            fixed
        },
        Err(e) => {
            debug!("Rev fixup not applicable: {}", e);
            content.to_owned()
        },
    }
}
