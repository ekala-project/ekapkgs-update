use std::process::Stdio;

use regex::Regex;
use tokio::process::Command;
use tracing::{debug, info, warn};
use walkdir::WalkDir;

use crate::nix::{eval_nix_expr, get_variants_list, normalize_entry_point};
use crate::package::PackageMetadata;
use crate::rewrite::update_variant_attr;
use crate::variant_strategy::extract_version_prefix;
use crate::vcs_sources::{SemverStrategy, UpstreamSource, extract_version_from_tag};

/// Get the default variant name for a mkManyVariants package
///
/// This function determines which variant is the default by comparing the version
/// of the base package with the versions of all variants.
///
/// # Arguments
/// * `file` - Path to the Nix file to evaluate
/// * `attr_path` - The package attribute path (e.g., "pkgs.ninja")
///
/// # Returns
/// The name of the default variant (e.g., "v1_13")
///
/// # Errors
/// Returns an error if:
/// - The package is not a mkManyVariants package
/// - Unable to determine the default variant
pub async fn get_default_variant(file: &str, attr_path: &str) -> anyhow::Result<String> {
    // Get the version of the base package (which is the default variant)
    let normalized_entry = normalize_entry_point(file);
    let base_version_expr = format!(
        "with import {} {{ }}; {}.version",
        normalized_entry, attr_path
    );
    let base_version = eval_nix_expr(&base_version_expr).await?;
    debug!("Base package version: {}", base_version);

    // Normalize base version for comparison (removes "v" prefix, etc.)
    let base_version_normalized = extract_version_from_tag(&base_version);

    // Get all variants and their versions
    let variants = get_variants_list(file, attr_path).await?;

    // Find which variant has the matching version
    for variant_name in variants {
        let variant_version =
            crate::nix::get_variant_version(file, attr_path, &variant_name).await?;
        // Normalize variant version for comparison
        let variant_version_normalized = extract_version_from_tag(&variant_version);

        if variant_version_normalized == base_version_normalized {
            debug!(
                "Found default variant: {} (version: {})",
                variant_name, variant_version
            );
            return Ok(variant_name);
        }
    }

    anyhow::bail!(
        "Could not determine default variant for {} (base version: {})",
        attr_path,
        base_version
    )
}

/// Update a single variant in a mkManyVariants package
pub async fn update_single_variant(
    file: &str,
    attr_path: &str,
    variant_name: &str,
    strategy: SemverStrategy,
    _commit: bool,
    _create_pr: bool,
    _upstream: Option<String>,
    _fork: String,
    _run_passthru_tests: bool,
) -> anyhow::Result<()> {
    // Get metadata for this specific variant
    let variant_attr_path = format!("{}.variants.{}", attr_path, variant_name);
    let metadata = PackageMetadata::from_attr_path(file, &variant_attr_path).await?;

    info!(
        "Current version for variant '{}': {}",
        variant_name, metadata.version
    );

    // Find the upstream source
    let src_url = metadata
        .src_url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No src_url found for variant '{}'", variant_name))?;

    let upstream_source = UpstreamSource::from_url(src_url)
        .ok_or_else(|| anyhow::anyhow!("Could not parse upstream source from URL: {}", src_url))?;

    info!("Upstream source: {:?}", upstream_source);

    // Extract version prefix from variant name (e.g., "v1_2" -> "1.2")
    // This ensures we only consider releases matching the variant's version series
    let version_prefix = extract_version_prefix(variant_name);
    if let Some(ref prefix) = version_prefix {
        info!(
            "Filtering releases to version prefix: {} (from variant '{}')",
            prefix, variant_name
        );
    }

    // Fetch new version based on strategy
    let release = upstream_source
        .get_compatible_release(
            &metadata.version,
            strategy,
            version_prefix.as_deref(),
            None, // No explicit version for variants
            None, // No version regex for variants
        )
        .await?;

    // Normalize version from release tag (removes "v" prefix, etc.)
    let new_version = extract_version_from_tag(&release.tag_name);

    if new_version == metadata.version {
        info!(
            "Variant '{}' is already up-to-date ({})",
            variant_name, metadata.version
        );
        return Ok(());
    }

    info!(
        "New version available for variant '{}': {} -> {}",
        variant_name, metadata.version, new_version
    );

    // Find the variants.nix file
    let variants_file_path = find_variants_file(file, attr_path).await?;
    info!("Variants file: {}", variants_file_path);

    // Read the variants.nix file
    let variants_content = tokio::fs::read_to_string(&variants_file_path).await?;

    // Update the version in the variant
    let updated_content = update_variant_attr(
        &variants_content,
        variant_name,
        "version",
        new_version,
        Some(&metadata.version),
    )?;

    // Discover new hash by building with wrong hash
    let new_hash =
        discover_hash_for_variant(file, attr_path, variant_name, &updated_content).await?;

    // Update the hash in the variant
    let final_content =
        if let (Some(old_hash), Some(ref new_h)) = (&metadata.output_hash, &new_hash) {
            update_variant_attr(
                &updated_content,
                variant_name,
                "src-hash",
                new_h,
                Some(old_hash),
            )?
        } else {
            updated_content
        };

    // Write the updated file
    tokio::fs::write(&variants_file_path, &final_content).await?;
    info!(
        "Updated variant '{}' in {}",
        variant_name, variants_file_path
    );

    // Build to verify
    info!("Building variant '{}' to verify update...", variant_name);
    let build_result = Command::new("nix-build")
        .arg("-A")
        .arg(&variant_attr_path)
        .arg(file)
        .output()
        .await?;

    if !build_result.status.success() {
        let stderr = String::from_utf8_lossy(&build_result.stderr);
        anyhow::bail!("Build failed for variant '{}': {}", variant_name, stderr);
    }

    info!("Build successful for variant '{}'", variant_name);

    Ok(())
}

/// Find the variants.nix file for a mkManyVariants package
pub async fn find_variants_file(file: &str, attr_path: &str) -> anyhow::Result<String> {
    // Get the package's meta.position to find the directory
    let normalized_entry = normalize_entry_point(file);
    let position_expr = format!(
        "with import {} {{ }}; {}.meta.position",
        normalized_entry, attr_path
    );

    let position = eval_nix_expr(&position_expr).await?;
    let (file_path, _) = position
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {}", position))?;

    // The variants.nix file should be in the same directory
    let path = std::path::Path::new(file_path);
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot get parent directory of {}", file_path))?;

    let variants_path = dir.join("variants.nix");

    if variants_path.exists() {
        Ok(variants_path.to_string_lossy().to_string())
    } else {
        anyhow::bail!("variants.nix not found in {}", dir.display())
    }
}

/// Discover hash for a variant by writing temporary file and building
async fn discover_hash_for_variant(
    file: &str,
    attr_path: &str,
    variant_name: &str,
    temp_content: &str,
) -> anyhow::Result<Option<String>> {
    // Write temporary variants.nix
    let variants_file_path = find_variants_file(file, attr_path).await?;
    let backup_content = tokio::fs::read_to_string(&variants_file_path).await?;

    // Set a known invalid hash
    let temp_with_bad_hash = update_variant_attr(
        temp_content,
        variant_name,
        "src-hash",
        "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        None,
    )?;

    tokio::fs::write(&variants_file_path, &temp_with_bad_hash).await?;

    let variant_attr_path = format!("{}.variants.{}", attr_path, variant_name);

    // Try to build - it will fail but give us the correct hash
    let build_result = Command::new("nix-build")
        .arg("-A")
        .arg(&variant_attr_path)
        .arg(file)
        .stderr(Stdio::piped())
        .output()
        .await?;

    // Restore original content
    tokio::fs::write(&variants_file_path, &backup_content).await?;

    if build_result.status.success() {
        // Shouldn't happen with a wrong hash, but handle it
        return Ok(None);
    }

    let stderr = String::from_utf8_lossy(&build_result.stderr);

    // Extract hash from error message
    let hash_pattern = Regex::new(r"got:\s+(sha256-[A-Za-z0-9+/=]+)")?;
    if let Some(captures) = hash_pattern.captures(&stderr) {
        let hash = captures.get(1).unwrap().as_str().to_string();
        info!("Discovered hash for variant '{}': {}", variant_name, hash);
        Ok(Some(hash))
    } else {
        warn!("Could not extract hash from build error: {}", stderr);
        Ok(None)
    }
}

/// Find version and hash in sibling files for mkManyVariants pattern
///
/// Searches parent directory for .nix files containing both the version and hash exactly once.
/// Returns the path to the sibling file if found.
pub async fn find_version_in_siblings(
    file_path: &str,
    version: &str,
    hash: Option<&str>,
) -> anyhow::Result<Option<String>> {
    use std::path::Path;

    let path = Path::new(file_path);
    let parent = match path.parent() {
        Some(p) => p,
        None => return Ok(None),
    };

    debug!(
        "Searching for version {} in siblings of {}",
        version, file_path
    );

    // Iterate through .nix files in parent directory
    for entry in WalkDir::new(parent)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let entry_path = entry.path();

        // Skip non-nix files and the original file
        if entry_path.extension().and_then(|s| s.to_str()) != Some("nix") {
            continue;
        }
        if entry_path == path {
            continue;
        }

        // Read the file content
        let content = match tokio::fs::read_to_string(entry_path).await {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Count occurrences of version
        let version_count = content.matches(version).count();

        // Count occurrences of hash if provided
        let hash_count = if let Some(h) = hash {
            content.matches(h).count()
        } else {
            1 // If no hash provided, consider it matched
        };

        // If both appear exactly once, we found the variants file
        if version_count == 1 && hash_count == 1 {
            let sibling_path = entry_path.to_string_lossy().to_string();
            info!(
                "Found version {} and hash in sibling file: {}",
                version, sibling_path
            );
            return Ok(Some(sibling_path));
        }
    }

    Ok(None)
}
