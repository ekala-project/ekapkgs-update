use std::collections::{HashMap, HashSet};

use anyhow::Context;
use regex::Regex;
use tracing::{debug, info, warn};
use walkdir::WalkDir;

use crate::nix::{
    eval_nix_expr, eval_nix_expr_for_system, get_variants_list, normalize_entry_point,
};
use crate::package::PackageMetadata;
use crate::rewrite::{add_variant_entry, update_variant_attr};
use crate::variant_strategy::{
    extract_version_prefix, infer_variant_component_count, variant_name_from_series_key,
    version_series_key,
};
use crate::vcs_sources::{SemverStrategy, UpstreamSource, extract_version_from_tag};

/// Hash attribute names to try, in order of preference.
const HASH_ATTR_CANDIDATES: &[&str] = &["src-hash", "hash", "sha256"];

/// Detect which hash attribute name a variants.nix file uses by checking
/// if any of the candidate names appear in the content.
fn detect_hash_attr_name(content: &str) -> &'static str {
    for &attr in HASH_ATTR_CANDIDATES {
        if content.contains(attr) {
            return attr;
        }
    }
    HASH_ATTR_CANDIDATES[0] // default
}

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
    let base_version_expr = format!("with import {normalized_entry} {{ }}; {attr_path}.version");
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
        "Could not determine default variant for {attr_path} (base version: {base_version})"
    )
}

/// Update a single variant in a mkManyVariants package
pub async fn update_single_variant(
    file: &str,
    attr_path: &str,
    variant_name: &str,
    strategy: SemverStrategy,
    _update_config: super::UpdateConfig,
) -> anyhow::Result<()> {
    // Get metadata for this specific variant
    let variant_attr_path = format!("{attr_path}.variants.{variant_name}");
    let metadata = PackageMetadata::from_attr_path(file, &variant_attr_path).await?;

    info!(
        "Current version for variant '{}': {}",
        variant_name, metadata.version
    );

    // Find the upstream source (explicit github-repo > src.url > error)
    let upstream_source = if let Some(ref gh_repo) = metadata.github_repo {
        let parts: Vec<&str> = gh_repo.splitn(2, '/').collect();
        if parts.len() == 2 {
            UpstreamSource::GitHub {
                owner: parts[0].to_owned(),
                repo: parts[1].to_owned(),
            }
        } else {
            anyhow::bail!("Invalid github-repo format '{gh_repo}': expected 'owner/repo'");
        }
    } else if let Some(ref src_url) = metadata.src_url {
        UpstreamSource::from_url(src_url)
            .ok_or_else(|| anyhow::anyhow!("Could not parse upstream source from URL: {src_url}"))?
    } else {
        anyhow::bail!("No src_url or github-repo found for variant '{variant_name}'");
    };

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

    // Use include-prereleases from passthru, or default to false
    let include_prereleases = metadata.include_prereleases.unwrap_or(false);

    // Use version-regex from passthru if available
    let version_regex = metadata.version_regex.as_deref();

    // Fetch new version based on strategy
    let release = upstream_source
        .get_compatible_release(
            &metadata.version,
            strategy,
            version_prefix.as_deref(),
            None, // No explicit version for variants
            version_regex,
            include_prereleases,
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
    let variants_content = tokio::fs::read_to_string(&variants_file_path)
        .await
        .with_context(|| format!("read variants file {variants_file_path}"))?;

    // Update the version in the variant
    let updated_content = update_variant_attr(
        &variants_content,
        variant_name,
        "version",
        new_version,
        Some(&metadata.version),
    )?;

    // Check if this package uses platform-specific hashes
    if let Some(ref platforms) = metadata.platform_hashes {
        // Platform-hash flow: update version, then for each platform evaluate
        // src.url with --system and prefetch it to discover the correct hash.
        tokio::fs::write(&variants_file_path, &updated_content)
            .await
            .with_context(|| format!("write variants file {variants_file_path}"))?;

        let normalized_entry = normalize_entry_point(file);

        for platform in platforms {
            info!(
                "Discovering hash for variant '{}' on {}",
                variant_name, platform
            );

            // Evaluate the stale hash for this platform
            let hash_expr = format!(
                "with import {} {{ }}; {}.src.outputHash",
                normalized_entry, variant_attr_path
            );
            let stale_hash = match eval_nix_expr_for_system(&hash_expr, platform).await {
                Ok(h) => h,
                Err(e) => {
                    warn!(
                        "Could not evaluate src.outputHash for {} on {}: {}",
                        variant_name, platform, e
                    );
                    continue;
                },
            };

            // Evaluate the source URL for this platform
            let url_expr = format!(
                "with import {} {{ }}; {}.src.url or (builtins.head {}.src.urls)",
                normalized_entry, variant_attr_path, variant_attr_path
            );
            let src_url = match eval_nix_expr_for_system(&url_expr, platform).await {
                Ok(u) => u,
                Err(e) => {
                    warn!(
                        "Could not evaluate src.url for {} on {}: {}",
                        variant_name, platform, e
                    );
                    continue;
                },
            };

            debug!("Prefetching {} for {}", src_url, platform);

            // Prefetch the URL to get the correct hash
            let correct_hash = match prefetch_url_hash(&src_url).await {
                Ok(h) => h,
                Err(e) => {
                    warn!("Could not prefetch {} for {}: {}", src_url, platform, e);
                    continue;
                },
            };

            // Replace the stale hash in the file
            let content = tokio::fs::read_to_string(&variants_file_path).await?;
            if !content.contains(&stale_hash) {
                warn!(
                    "Stale hash for {} not found in {}, skipping",
                    platform, variants_file_path
                );
                continue;
            }
            let fixed = content.replacen(&stale_hash, &correct_hash, 1);
            tokio::fs::write(&variants_file_path, &fixed).await?;
            info!(
                "Updated hash for {} on {}: {}",
                variant_name, platform, correct_hash
            );
        }
    } else {
        // Normal single-hash flow
        let new_hash =
            discover_hash_for_variant(file, attr_path, variant_name, &updated_content).await?;

        let final_content =
            if let (Some(old_hash), Some(ref new_h)) = (&metadata.output_hash, &new_hash) {
                // Try multiple hash attribute names
                let hash_attrs = ["src-hash", "hash", "sha256"];
                let mut result = None;
                for attr_name in &hash_attrs {
                    if let Ok(content) = update_variant_attr(
                        &updated_content,
                        variant_name,
                        attr_name,
                        new_h,
                        Some(old_hash),
                    ) {
                        result = Some(content);
                        break;
                    }
                }
                result.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Could not find hash attribute in variant '{}' (tried: {:?})",
                        variant_name,
                        hash_attrs
                    )
                })?
            } else {
                updated_content
            };

        tokio::fs::write(&variants_file_path, &final_content)
            .await
            .with_context(|| format!("write variants file {variants_file_path}"))?;
    }

    info!(
        "Updated variant '{}' in {}",
        variant_name, variants_file_path
    );

    // Build to verify
    info!("Building variant '{}' to verify update...", variant_name);
    let (success, _stdout, stderr) = super::build_nix_expr(file, &variant_attr_path, None).await?;

    if !success {
        anyhow::bail!("Build failed for variant '{variant_name}': {stderr}");
    }

    info!("Build successful for variant '{}'", variant_name);

    Ok(())
}

/// Find the variants.nix file for a mkManyVariants package
pub async fn find_variants_file(file: &str, attr_path: &str) -> anyhow::Result<String> {
    // Get the package's meta.position to find the directory
    let normalized_entry = normalize_entry_point(file);
    let position_expr = format!("with import {normalized_entry} {{ }}; {attr_path}.meta.position");

    let position = eval_nix_expr(&position_expr).await?;
    let (file_path, _) = position
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {position}"))?;

    // The variants.nix file should be in the same directory
    let path = std::path::Path::new(file_path);
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot get parent directory of {file_path}"))?;

    let variants_path = dir.join("variants.nix");

    if variants_path.exists() {
        Ok(variants_path.to_string_lossy().to_string())
    } else {
        anyhow::bail!("variants.nix not found in {}", dir.display())
    }
}

/// Discover new upstream version series not covered by existing variants
///
/// Fetches all upstream releases, groups them by version series (matching the
/// component count of existing variant names), and identifies series that are
/// newer than the highest existing variant but don't have a corresponding
/// variant entry yet.
///
/// # Returns
/// A vector of `(variant_name, best_version)` pairs for each new series to add.
pub async fn discover_new_variants(
    file: &str,
    attr_path: &str,
    existing_variants: &[String],
) -> anyhow::Result<Vec<(String, String)>> {
    // Step 1: Determine the component count from existing variant names
    let component_count = match infer_variant_component_count(existing_variants) {
        Some(count) => count,
        None => {
            debug!(
                "{}: Cannot infer variant component count (mixed or no versioned variants), \
                 skipping new variant discovery",
                attr_path
            );
            return Ok(vec![]);
        },
    };
    debug!(
        "{}: Variant component count: {} (e.g., {} → {})",
        attr_path,
        component_count,
        existing_variants.first().map_or("?", String::as_str),
        if component_count == 1 {
            "Minor"
        } else {
            "Patch"
        }
    );

    // Step 2: Get metadata from any existing variant to find upstream source
    let reference_variant = existing_variants
        .iter()
        .find(|v| extract_version_prefix(v).is_some())
        .ok_or_else(|| anyhow::anyhow!("No parseable variant found for {attr_path}"))?;

    let variant_attr_path = format!("{attr_path}.variants.{reference_variant}");
    let metadata = PackageMetadata::from_attr_path(file, &variant_attr_path).await?;

    let upstream_source = if let Some(ref gh_repo) = metadata.github_repo {
        let parts: Vec<&str> = gh_repo.splitn(2, '/').collect();
        if parts.len() == 2 {
            UpstreamSource::GitHub {
                owner: parts[0].to_owned(),
                repo: parts[1].to_owned(),
            }
        } else {
            anyhow::bail!("Invalid github-repo format '{gh_repo}': expected 'owner/repo'");
        }
    } else {
        let src_url = metadata.src_url.as_ref().ok_or_else(|| {
            anyhow::anyhow!("No src_url or github-repo found for variant '{reference_variant}'")
        })?;
        UpstreamSource::from_url(src_url)
            .ok_or_else(|| anyhow::anyhow!("Could not parse upstream source from URL: {src_url}"))?
    };

    let include_prereleases = metadata.include_prereleases.unwrap_or(false);

    // Step 3: Fetch all upstream releases
    let all_releases = upstream_source.fetch_all_releases().await?;
    debug!(
        "{}: Fetched {} upstream releases",
        attr_path,
        all_releases.len()
    );

    // Step 4: Group releases by series key, keeping only the best version per series
    let mut series_best: HashMap<String, String> = HashMap::new();
    for release in &all_releases {
        if release.is_prerelease && !include_prereleases {
            continue;
        }

        let version = extract_version_from_tag(&release.tag_name);

        // Skip prerelease-looking versions
        if !include_prereleases && crate::vcs_sources::version_looks_prerelease(version) {
            continue;
        }

        // Skip versions that don't parse well (no dots)
        if !version.contains('.') {
            continue;
        }

        let Some(key) = version_series_key(version, component_count) else {
            continue;
        };

        series_best
            .entry(key)
            .and_modify(|existing| {
                // Keep the higher version
                let existing_version = extract_version_from_tag(existing);
                if crate::vcs_sources::compare_version_components(version, existing_version)
                    == std::cmp::Ordering::Greater
                {
                    *existing = version.to_owned();
                }
            })
            .or_insert_with(|| version.to_owned());
    }

    // Step 5: Compute existing series keys
    let existing_keys: HashSet<String> = existing_variants
        .iter()
        .filter_map(|v| extract_version_prefix(v))
        .collect();

    // Find the highest existing series key (to only add newer ones)
    let highest_existing = existing_keys
        .iter()
        .max_by(|a, b| crate::vcs_sources::compare_version_components(a, b))
        .cloned();

    let Some(ref highest) = highest_existing else {
        debug!("{}: No existing version series found, skipping", attr_path);
        return Ok(vec![]);
    };
    debug!(
        "{}: Highest existing series: {} (from {} variants)",
        attr_path,
        highest,
        existing_keys.len()
    );

    // Step 6: Find new series that are newer than the highest existing
    let mut new_variants: Vec<(String, String)> = series_best
        .into_iter()
        .filter(|(key, _)| {
            !existing_keys.contains(key)
                && crate::vcs_sources::compare_version_components(key, highest)
                    == std::cmp::Ordering::Greater
        })
        .map(|(key, version)| (variant_name_from_series_key(&key), version))
        .collect();

    // Sort by variant name for deterministic ordering
    new_variants.sort_by(|(a, _), (b, _)| a.cmp(b));

    if new_variants.is_empty() {
        info!(
            "{}: No new variant series found upstream (highest existing: {})",
            attr_path, highest
        );
    } else {
        info!(
            "{}: Found {} new variant series: {:?}",
            attr_path,
            new_variants.len(),
            new_variants
                .iter()
                .map(|(name, ver)| format!("{name} ({ver})"))
                .collect::<Vec<_>>()
        );
    }

    Ok(new_variants)
}

/// Add a new variant entry to a mkManyVariants package
///
/// Creates a new variant in `variants.nix` with the specified version,
/// discovers the correct source hash (handling both single-hash and
/// platform-hash packages), and verifies the build.
pub async fn add_new_variant(
    file: &str,
    attr_path: &str,
    variant_name: &str,
    version: &str,
    update_config: super::UpdateConfig,
) -> anyhow::Result<()> {
    let variants_file_path = find_variants_file(file, attr_path).await?;
    info!(
        "Adding variant '{}' (version {}) to {}",
        variant_name, version, variants_file_path
    );

    // Read the current variants file
    let content = tokio::fs::read_to_string(&variants_file_path)
        .await
        .with_context(|| format!("read variants file {variants_file_path}"))?;

    // Detect which hash attribute name the existing variants use
    let hash_attr = detect_hash_attr_name(&content);
    debug!("Using hash attribute name: {}", hash_attr);

    // Check if this package uses platform-specific hashes by examining an
    // existing variant's metadata
    let existing_variants = get_variants_list(file, attr_path).await?;
    let reference_variant = existing_variants
        .iter()
        .find(|v| extract_version_prefix(v).is_some())
        .ok_or_else(|| anyhow::anyhow!("No existing variant to use as reference"))?;

    let ref_attr_path = format!("{attr_path}.variants.{reference_variant}");
    let ref_metadata = PackageMetadata::from_attr_path(file, &ref_attr_path).await?;

    let placeholder_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let variant_attr_path = format!("{attr_path}.variants.{variant_name}");

    if let Some(ref platforms) = ref_metadata.platform_hashes {
        // Platform-hash flow: insert variant with placeholder, then prefetch each platform
        let content_with_variant = add_variant_entry(
            &content,
            variant_name,
            &[("version", version), (hash_attr, placeholder_hash)],
        )?;

        tokio::fs::write(&variants_file_path, &content_with_variant)
            .await
            .with_context(|| format!("write variants file {variants_file_path}"))?;

        let normalized_entry = normalize_entry_point(file);

        for platform in platforms {
            info!(
                "Discovering hash for new variant '{}' on {}",
                variant_name, platform
            );

            let url_expr = format!(
                "with import {} {{ }}; {}.src.url or (builtins.head {}.src.urls)",
                normalized_entry, variant_attr_path, variant_attr_path
            );
            let src_url = match eval_nix_expr_for_system(&url_expr, platform).await {
                Ok(u) => u,
                Err(e) => {
                    warn!(
                        "Could not evaluate src.url for {} on {}: {}",
                        variant_name, platform, e
                    );
                    continue;
                },
            };

            debug!("Prefetching {} for {}", src_url, platform);

            let correct_hash = match prefetch_url_hash(&src_url).await {
                Ok(h) => h,
                Err(e) => {
                    warn!("Could not prefetch {} for {}: {}", src_url, platform, e);
                    continue;
                },
            };

            // Replace the placeholder hash (first occurrence still remaining)
            let current = tokio::fs::read_to_string(&variants_file_path).await?;
            if !current.contains(placeholder_hash) {
                // All placeholders consumed — update the stale hash for this platform
                let hash_expr = format!(
                    "with import {} {{ }}; {}.src.outputHash",
                    normalized_entry, variant_attr_path
                );
                let stale_hash = match eval_nix_expr_for_system(&hash_expr, platform).await {
                    Ok(h) => h,
                    Err(e) => {
                        warn!(
                            "Could not evaluate stale hash for {} on {}: {}",
                            variant_name, platform, e
                        );
                        continue;
                    },
                };
                let fixed = current.replacen(&stale_hash, &correct_hash, 1);
                tokio::fs::write(&variants_file_path, &fixed).await?;
            } else {
                let fixed = current.replacen(placeholder_hash, &correct_hash, 1);
                tokio::fs::write(&variants_file_path, &fixed).await?;
            }
            info!(
                "Updated hash for {} on {}: {}",
                variant_name, platform, correct_hash
            );
        }
    } else {
        // Normal single-hash flow
        let content_with_variant = add_variant_entry(
            &content,
            variant_name,
            &[("version", version), (hash_attr, placeholder_hash)],
        )?;

        // Write the variant with placeholder hash, then discover the correct hash
        let new_hash =
            discover_hash_for_variant(file, attr_path, variant_name, &content_with_variant).await?;

        // discover_hash_for_variant restores the backup (which is the content WITH
        // the new variant and placeholder hash, since we wrote it to disk first).
        // We need to re-read and update with the correct hash.
        let final_content = if let Some(ref hash) = new_hash {
            update_variant_attr(
                &content_with_variant,
                variant_name,
                hash_attr,
                hash,
                Some(placeholder_hash),
            )?
        } else {
            content_with_variant
        };

        tokio::fs::write(&variants_file_path, &final_content)
            .await
            .with_context(|| format!("write variants file {variants_file_path}"))?;
    }

    // Build to verify
    info!("Building new variant '{}' to verify...", variant_name);
    let (success, _stdout, stderr) = super::build_nix_expr(file, &variant_attr_path, None).await?;

    if !success {
        anyhow::bail!("Build failed for new variant '{variant_name}': {stderr}");
    }

    info!("Build successful for new variant '{}'", variant_name);

    // Commit the new variant if --commit is set
    if update_config.commit {
        commit_new_variant(&variants_file_path, attr_path, variant_name, version).await?;
    }

    Ok(())
}

/// Create a git commit for a newly added variant
async fn commit_new_variant(
    variants_file_path: &str,
    attr_path: &str,
    variant_name: &str,
    version: &str,
) -> anyhow::Result<()> {
    let add_output = tokio::process::Command::new("git")
        .args(["add", variants_file_path])
        .output()
        .await
        .context("Failed to run git add")?;

    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        anyhow::bail!("git add failed: {stderr}");
    }

    let commit_msg = format!("{attr_path}.{variant_name}: init at {version}");
    let commit_output = tokio::process::Command::new("git")
        .args(["commit", "-m", &commit_msg])
        .output()
        .await
        .context("Failed to run git commit")?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        anyhow::bail!("git commit failed: {stderr}");
    }

    info!("Committed: {}", commit_msg);
    Ok(())
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
    let backup_content = tokio::fs::read_to_string(&variants_file_path)
        .await
        .with_context(|| format!("read variants file {variants_file_path}"))?;

    // Set a known invalid hash — try multiple attribute names
    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let hash_attrs = ["src-hash", "hash", "sha256"];
    let mut temp_with_bad_hash = None;
    for attr_name in &hash_attrs {
        if let Ok(content) =
            update_variant_attr(temp_content, variant_name, attr_name, invalid_hash, None)
        {
            temp_with_bad_hash = Some(content);
            break;
        }
    }
    let temp_with_bad_hash = temp_with_bad_hash.ok_or_else(|| {
        anyhow::anyhow!(
            "Could not find hash attribute in variant '{}' (tried: {:?})",
            variant_name,
            hash_attrs
        )
    })?;

    tokio::fs::write(&variants_file_path, &temp_with_bad_hash)
        .await
        .with_context(|| format!("write variants file {variants_file_path}"))?;

    let variant_attr_path = format!("{attr_path}.variants.{variant_name}");

    // Try to build - it will fail but give us the correct hash
    let (success, _stdout, stderr) =
        super::build_nix_expr(file, &variant_attr_path, Some("src")).await?;

    // Restore original content
    tokio::fs::write(&variants_file_path, &backup_content)
        .await
        .with_context(|| format!("restore variants file {variants_file_path}"))?;

    if success {
        // Shouldn't happen with a wrong hash, but handle it
        return Ok(None);
    }

    // Extract hash from error message
    let hash_pattern = Regex::new(r"got:\s+(sha256-[A-Za-z0-9+/=]+)")?;
    if let Some(captures) = hash_pattern.captures(&stderr) {
        let hash = captures.get(1).unwrap().as_str().to_owned();
        info!("Discovered hash for variant '{}': {}", variant_name, hash);
        Ok(Some(hash))
    } else {
        warn!("Could not extract hash from build error: {}", stderr);
        Ok(None)
    }
}

/// Prefetch a URL and return its SRI hash
///
/// Uses `nix store prefetch-file --json <url>` to download the file and
/// compute its hash without requiring a full build.
async fn prefetch_url_hash(url: &str) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("nix")
        .args(["store", "prefetch-file", "--json", url])
        .output()
        .await
        .context("Failed to execute nix store prefetch-file")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("nix store prefetch-file failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).context("Failed to parse prefetch-file JSON output")?;

    json["hash"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("No hash field in prefetch-file output"))
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
    let Some(parent) = path.parent() else {
        return Ok(None);
    };

    debug!(
        "Searching for version {} in siblings of {}",
        version, file_path
    );

    // Iterate through .nix files in parent directory
    for entry in WalkDir::new(parent)
        .max_depth(1)
        .into_iter()
        .filter_map(std::result::Result::ok)
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
        let Ok(content) = tokio::fs::read_to_string(entry_path).await else {
            continue;
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
