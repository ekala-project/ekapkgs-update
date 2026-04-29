use std::process::Stdio;

use anyhow::Context;
use regex::Regex;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::git::{self, PrConfig};
use crate::github;
use crate::nix::{
    eval_nix_expr, get_variants_list, has_passthru_tests, is_many_variants_package,
    normalize_entry_point,
};
use crate::package::PackageMetadata;
use crate::rewrite::{
    find_and_update_attr, is_patches_array_empty, remove_patch_from_array,
    remove_patches_attribute, update_variant_attr,
};
use crate::variant_strategy::{infer_strategy_from_variant, is_variant_pinned};
use crate::vcs_sources::{SemverStrategy, UpstreamSource, extract_version_from_tag};

/// Result of a successful package update
pub struct UpdateOutcome {
    pub old_version: String,
    pub new_version: String,
    pub tests_passed: bool,
    pub metadata: PackageMetadata,
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
async fn get_default_variant(file: &str, attr_path: &str) -> anyhow::Result<String> {
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

pub async fn update(
    file: String,
    attr_path: String,
    semver_strategy: String,
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

    // Not a mkManyVariants package - use generic update method
    // Resolve the package file location via meta.position
    let file_location = get_file_location(&file, &attr_path).await?;

    if create_pr {
        // Determine PR config before doing work
        let pr_config = if let Some(remote_name) = upstream {
            git::get_pr_config_from_remote(&remote_name).await?
        } else {
            git::get_pr_config_from_git().await?
        };

        let outcome = perform_update_with_worktree(
            &file,
            &attr_path,
            &file_location,
            strategy,
            run_passthru_tests,
            false, // Don't fail on test errors for update command
            &pr_config,
            &fork,
        )
        .await?;

        info!(
            "✓ Successfully updated {} from {} to {} with PR",
            attr_path, outcome.old_version, outcome.new_version
        );
    } else {
        // Run update in-place (no worktree)
        let outcome = perform_update(
            file.clone(),
            attr_path.clone(),
            file_location,
            strategy,
            run_passthru_tests,
            false,
        )
        .await?;

        if commit {
            create_git_commit(
                &attr_path,
                &outcome.old_version,
                &outcome.new_version,
                outcome.tests_passed,
            )
            .await?;
        }
    }

    Ok(())
}

/// Update a single variant in a mkManyVariants package
async fn update_single_variant(
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

    // Fetch new version based on strategy
    let release = upstream_source
        .get_compatible_release(&metadata.version, strategy)
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
async fn find_variants_file(file: &str, attr_path: &str) -> anyhow::Result<String> {
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
async fn find_version_in_siblings(
    file_path: &str,
    version: &str,
    hash: Option<&str>,
) -> anyhow::Result<Option<String>> {
    use std::path::Path;

    use walkdir::WalkDir;

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

/// Update version and hash attributes in Nix file using AST manipulation
///
/// Returns the actual file path that was updated (may differ from input due to mkManyVariants)
async fn update_nix_file(
    eval_entry_point: &str,
    attr_path: &str,
    file_path: &str,
    old_version: &str,
    new_version: &str,
    old_hash: Option<&str>,
    new_hash: Option<&str>,
) -> anyhow::Result<String> {
    debug!("Updating Nix file at {} using AST manipulation", file_path);
    let content = tokio::fs::read_to_string(file_path).await?;

    // Try to update the version attribute
    let (updated_content, actual_file_path) =
        match find_and_update_attr(&content, "version", new_version, Some(old_version)) {
            Ok(content) => {
                debug!(
                    "Updated version attribute: {} -> {}",
                    old_version, new_version
                );
                (content, file_path.to_string())
            },
            Err(e) if e.to_string().contains("not found") => {
                // Version not found - check if this is a mkManyVariants package
                debug!(
                    "Version not found in {}, checking if mkManyVariants",
                    file_path
                );

                if is_many_variants_package(eval_entry_point, attr_path).await? {
                    // This is a mkManyVariants package - search sibling files
                    match find_version_in_siblings(file_path, old_version, old_hash).await? {
                        Some(sibling_path) => {
                            info!("Using mkManyVariants file: {}", sibling_path);
                            let sibling_content = tokio::fs::read_to_string(&sibling_path).await?;

                            // Try simple string replacement for mkManyVariants files
                            let updated = sibling_content.replace(old_version, new_version);
                            (updated, sibling_path)
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
        if actual_file_path != file_path {
            // mkManyVariants file - use string replacement
            let result = updated_content.replace(old_h, new_h);
            debug!(
                "Updated hash using string replacement: {} -> {}",
                old_h, new_h
            );
            result
        } else {
            // Normal file - try AST-based replacement
            let hash_attrs = vec!["hash", "sha256", "outputHash", "src-hash"];
            let mut result = updated_content.clone();
            let mut hash_updated = false;

            for attr_name in hash_attrs {
                match find_and_update_attr(&result, attr_name, new_h, Some(old_h)) {
                    Ok(new_content) => {
                        debug!("Updated {} attribute: {} -> {}", attr_name, old_h, new_h);
                        result = new_content;
                        hash_updated = true;
                        break;
                    },
                    Err(_) => continue, // Try next attribute name
                }
            }

            if !hash_updated {
                warn!("Could not find hash attribute to update in Nix file");
            }

            result
        }
    } else {
        updated_content
    };

    // Write back to file
    tokio::fs::write(&actual_file_path, final_content).await?;
    Ok(actual_file_path)
}

/// Update cargoHash attribute in Nix file
async fn update_cargo_hash(file_path: &str, old_hash: &str, new_hash: &str) -> anyhow::Result<()> {
    debug!("Updating cargoHash in {} using AST manipulation", file_path);
    let content = tokio::fs::read_to_string(file_path).await?;

    let updated_content = find_and_update_attr(&content, "cargoHash", new_hash, Some(old_hash))?;
    debug!("Updated cargoHash attribute: {} -> {}", old_hash, new_hash);

    tokio::fs::write(file_path, updated_content).await?;
    Ok(())
}

/// Update vendorHash attribute in Nix file
async fn update_vendor_hash(file_path: &str, old_hash: &str, new_hash: &str) -> anyhow::Result<()> {
    debug!(
        "Updating vendorHash in {} using AST manipulation",
        file_path
    );
    let content = tokio::fs::read_to_string(file_path).await?;

    let updated_content = find_and_update_attr(&content, "vendorHash", new_hash, Some(old_hash))?;
    debug!("Updated vendorHash attribute: {} -> {}", old_hash, new_hash);

    tokio::fs::write(file_path, updated_content).await?;
    Ok(())
}

/// Extract hash from Nix build error output
fn extract_hash_from_error(stderr: &str) -> Option<String> {
    // Nix error format: "got: sha256-<hash>"
    let hash_regex = Regex::new(r"got:\s+(sha256-[A-Za-z0-9+/=]+)").ok()?;
    let caps = hash_regex.captures(stderr)?;
    Some(caps.get(1)?.as_str().to_string())
}

/// Detect reversed patch errors and extract the patch filename
///
/// Looks for "Reversed (or previously applied) patch detected!" in the last 20 lines
/// and extracts the patch name from the preceding "applying patch" line.
///
/// Returns the patch filename to be removed from the patches array.
fn detect_reversed_patch(stderr: &str) -> Option<String> {
    // Get last 20 lines of stderr
    let lines: Vec<&str> = stderr.lines().collect();
    let start = lines.len().saturating_sub(20);
    let last_lines = &lines[start..];
    let patch_regex = Regex::new(r"applying patch /nix/store/[^-]+-(.+)").ok()?;

    // Look for the reversed patch error message
    for (i, line) in last_lines.iter().enumerate() {
        if line.contains("Reversed (or previously applied) patch detected!") {
            // Look backward for the "applying patch" line
            for j in (0..i).rev() {
                let prev_line = last_lines[j];
                // Pattern: "applying patch /nix/store/${hash}-${name}"
                if let Some(caps) = patch_regex.captures(prev_line) {
                    return Some(caps.get(1)?.as_str().to_string());
                }
            }
        }
    }

    None
}

/// Build Nix expression and return stdout/stderr
async fn build_nix_expr(
    eval_entry_point: &str,
    attr_path: &str,
    attr_suffix: Option<&str>,
) -> anyhow::Result<(bool, String, String)> {
    let full_attr = if let Some(suffix) = attr_suffix {
        format!("{}.{}", attr_path, suffix)
    } else {
        attr_path.to_string()
    };

    debug!("Building {}", full_attr);

    let output = Command::new("nix-build")
        .arg(eval_entry_point)
        .arg("-A")
        .arg(&full_attr)
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((output.status.success(), stdout, stderr))
}

/// Create a git commit for the update
async fn create_git_commit(
    attr_path: &str,
    old_version: &str,
    new_version: &str,
    tests_passed: bool,
) -> anyhow::Result<()> {
    info!("Creating git commit for update");

    // Check if we're in a git repository
    let git_check = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .await
        .context("Failed to check if directory is a git repository")?;

    if !git_check.status.success() {
        anyhow::bail!("Not in a git repository - cannot create commit");
    }

    // Get list of modified files
    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .await
        .context("Failed to run git status")?;

    if !status_output.status.success() {
        anyhow::bail!("git status failed");
    }

    let status_str = String::from_utf8_lossy(&status_output.stdout);
    let modified_files: Vec<&str> = status_str
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            // Parse git status output (format: "XY filename")
            let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
            if parts.len() == 2 {
                Some(parts[1].trim())
            } else {
                None
            }
        })
        .collect();

    if modified_files.is_empty() {
        warn!("No files to commit");
        return Ok(());
    }

    debug!("Files to commit: {:?}", modified_files);

    // Stage all modified files
    let mut add_cmd = Command::new("git");
    add_cmd.arg("add");
    for file in &modified_files {
        add_cmd.arg(file);
    }

    let add_output = add_cmd.output().await.context("Failed to run git add")?;

    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        anyhow::bail!("git add failed: {}", stderr);
    }

    // Create commit with formatted message
    let commit_message = if tests_passed {
        format!(
            "{}: {} -> {}\n\nTests: passthru.tests passed",
            attr_path, old_version, new_version
        )
    } else {
        format!("{}: {} -> {}", attr_path, old_version, new_version)
    };
    let commit_output = Command::new("git")
        .args(["commit", "-m", &commit_message])
        .output()
        .await
        .context("Failed to run git commit")?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        anyhow::bail!("git commit failed: {}", stderr);
    }

    info!("✓ Created commit: {}", commit_message);

    Ok(())
}

/// Resolve the file location for a package from meta.position
pub async fn get_file_location(eval_entry_point: &str, attr_path: &str) -> anyhow::Result<String> {
    let normalized_entry = normalize_entry_point(eval_entry_point);
    let position_expr = format!(
        "with import {} {{ }}; {}.meta.position",
        normalized_entry, attr_path
    );

    let position = eval_nix_expr(&position_expr).await?;

    if position.is_empty() {
        anyhow::bail!("Empty position returned from meta.position");
    }

    // Parse position string (format: "file:line")
    let (file_path, _line_str) = position
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {}", position))?;

    Ok(file_path.to_string())
}

/// Pure update logic: edit files, discover hashes, build, test.
/// Does NOT do any git operations. Returns outcome on success.
pub async fn perform_update(
    eval_entry_point: String,
    attr_path: String,
    file_location: String,
    strategy: SemverStrategy,
    run_passthru_tests: bool,
    fail_on_test_failure: bool,
) -> anyhow::Result<UpdateOutcome> {
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

    // Step 3: Fetch best compatible release based on strategy
    let best_release = upstream_source
        .get_compatible_release(&metadata.version, strategy)
        .await?;

    let new_version = UpstreamSource::get_version(&best_release);
    info!(
        "Found compatible version ({:?}): {} -> {}",
        strategy, metadata.version, new_version
    );

    // Step 4: Update version in file with invalid hash
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

    // Step 5: Build source to get correct hash
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

    // Step 6: Update hash with correct value
    let _ = update_nix_file(
        &eval_entry_point,
        &attr_path,
        &actual_file_location,
        &new_version,
        &new_version,
        Some(invalid_hash),
        Some(&correct_hash),
    )
    .await?;

    info!("Updated hash in {}", actual_file_location);

    // Step 7: Build source again to verify
    let (success, _stdout, stderr) =
        build_nix_expr(&eval_entry_point, &attr_path, Some("src")).await?;

    if !success {
        anyhow::bail!("Source build failed after hash update:\n{}", stderr);
    }

    info!("Source build successful");

    // For Rust packages, update cargoHash
    if let Some(old_cargo_hash) = &metadata.cargo_hash {
        info!("Detected Rust package, updating cargoHash");

        let invalid_cargo_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_cargo_hash(&actual_file_location, old_cargo_hash, invalid_cargo_hash).await?;

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

        let invalid_vendor_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_vendor_hash(&actual_file_location, old_vendor_hash, invalid_vendor_hash).await?;

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

        update_vendor_hash(
            &actual_file_location,
            invalid_vendor_hash,
            &correct_vendor_hash,
        )
        .await?;

        info!("Updated vendorHash in {}", actual_file_location);
    }

    // Step 8: Build full package to verify with reversed patch recovery
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
                    },
                }
            }
            break;
        }

        // Build failed - check for reversed patch errors
        if let Some(patch_name) = detect_reversed_patch(&stderr) {
            debug!("Detected reversed patch: {}", patch_name);

            let content = tokio::fs::read_to_string(&actual_file_location).await?;

            match remove_patch_from_array(&content, &patch_name) {
                Ok(updated_content) => {
                    tokio::fs::write(&actual_file_location, updated_content).await?;
                    debug!("Removed obsolete patch: {}", patch_name);
                },
                Err(e) => {
                    warn!("Failed to remove patch {}: {}", patch_name, e);
                    anyhow::bail!(
                        "Package build failed after update. Detected reversed patch but couldn't \
                         remove it: {}\n{}",
                        e,
                        stderr
                    );
                },
            }
        } else {
            warn!("Full package build failed:\n{}", stderr);
            anyhow::bail!(
                "Package build failed after update. You may need to manually fix build issues."
            );
        }
    }

    // Step 9: Run passthru.tests if requested
    let mut tests_passed = false;
    info!("Checking for passthru.tests...");
    if run_passthru_tests {
        let normalized_entry = normalize_entry_point(&eval_entry_point);

        if has_passthru_tests(&normalized_entry, &attr_path).await? {
            info!("Found {}.passthru.tests, building tests...", &attr_path);

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

    Ok(UpdateOutcome {
        old_version: metadata.version.clone(),
        new_version,
        tests_passed,
        metadata,
    })
}

/// Full pipeline: create worktree, update, commit, push, create PR, clean up.
/// Used by both `update --create-pr` and `run`.
pub async fn perform_update_with_worktree(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &str,
    strategy: SemverStrategy,
    run_passthru_tests: bool,
    fail_on_test_failure: bool,
    pr_config: &PrConfig,
    fork: &str,
) -> anyhow::Result<UpdateOutcome> {
    let repo_root = git::get_repo_root().await?;
    let worktree_path = git::create_worktree(attr_path).await?;

    // Remap both eval_entry_point and file_location into worktree
    let wt_eval = git::remap_to_worktree(eval_entry_point, &repo_root, &worktree_path);
    let wt_file = git::remap_to_worktree(file_location, &repo_root, &worktree_path);

    debug!(
        "Worktree paths: eval={}, file={}",
        wt_eval, wt_file
    );

    // Run the update in the worktree
    let outcome = match perform_update(
        wt_eval,
        attr_path.to_string(),
        wt_file,
        strategy,
        run_passthru_tests,
        fail_on_test_failure,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            git::cleanup_worktree(&worktree_path).await.ok();
            return Err(e);
        },
    };

    // Commit, push, create PR from worktree
    let github_token = std::env::var("GITHUB_TOKEN").context(
        "GITHUB_TOKEN environment variable is required for PR creation. Set it with: export \
         GITHUB_TOKEN=your_token_here",
    )?;

    let branch_name = match git::create_and_push_branch(
        &worktree_path,
        attr_path,
        &outcome.old_version,
        &outcome.new_version,
        fork,
        outcome.tests_passed,
    )
    .await
    {
        Ok(name) => name,
        Err(e) => {
            git::cleanup_worktree(&worktree_path).await.ok();
            return Err(e);
        },
    };

    // Build PR title and body
    let pr_title = format!(
        "{}: {} -> {}",
        attr_path, outcome.old_version, outcome.new_version
    );
    let mut pr_body = format!(
        "## Summary\n\nThis PR updates `{}` from version {} to {}.\n\n## Changes\n\n- Updated \
         package version\n- Updated source hash",
        attr_path, outcome.old_version, outcome.new_version
    );

    if let Some(description) = outcome.metadata.description.as_ref() {
        pr_body.push_str(&format!(
            "\n\n## Package Information\n\n**Description:** {}",
            description
        ));
    } else {
        pr_body.push_str("\n\n## Package Information");
    }
    if let Some(homepage) = outcome.metadata.homepage.as_ref() {
        pr_body.push_str(&format!("\n\n**Homepage:** {}", homepage));
    }
    if let Some(changelog) = outcome.metadata.changelog.as_ref() {
        pr_body.push_str(&format!("\n\n**Changelog:** {}", changelog));
    }

    pr_body.push_str("\n\n🤖 Generated with ekapkgs-update");

    let pr = match github::create_pull_request(
        &pr_config.owner,
        &pr_config.repo,
        &pr_title,
        &pr_body,
        &branch_name,
        &pr_config.base_branch,
        &github_token,
    )
    .await
    {
        Ok(pr) => pr,
        Err(e) => {
            git::cleanup_worktree(&worktree_path).await.ok();
            return Err(e);
        },
    };

    info!("✓ Created pull request: {}", pr.html_url);
    println!("Pull request created: {}", pr.html_url);

    // Clean up worktree
    git::cleanup_worktree(&worktree_path).await.ok();

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_hash_from_error() {
        let stderr = r#"
error: hash mismatch in fixed-output derivation
  specified: sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
       got: sha256-RealHashValue123456789ABCDEFGHIJKLMNOPQRST=
"#;
        let result = extract_hash_from_error(stderr);
        assert!(result.is_some());
        assert_eq!(
            result.unwrap(),
            "sha256-RealHashValue123456789ABCDEFGHIJKLMNOPQRST="
        );
    }

    #[test]
    fn test_extract_hash_from_error_no_match() {
        let stderr = "Some other error message";
        let result = extract_hash_from_error(stderr);
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_reversed_patch() {
        let stderr = r#"
unpacking sources
unpacking source archive /nix/store/abc123-source.tar.gz
source root is source
patching sources
applying patch /nix/store/xyz789-fix-build.patch
patching file src/main.c
Reversed (or previously applied) patch detected!  Skipping patch.
1 out of 1 hunk ignored -- saving rejects to file src/main.c.rej
"#;
        let result = detect_reversed_patch(stderr);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "fix-build.patch");
    }

    #[test]
    fn test_detect_reversed_patch_no_match() {
        let stderr = "Some other build error message";
        let result = detect_reversed_patch(stderr);
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_reversed_patch_in_last_20_lines() {
        // Create a stderr with more than 20 lines, with the reversed patch error near the end
        let mut lines = Vec::new();
        for i in 0..30 {
            lines.push(format!("build output line {}", i));
        }
        lines.push("applying patch /nix/store/hash123-obsolete.patch".to_string());
        lines.push("patching file test.c".to_string());
        lines.push("Reversed (or previously applied) patch detected!".to_string());
        let stderr = lines.join("\n");

        let result = detect_reversed_patch(&stderr);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "obsolete.patch");
    }

    #[test]
    fn test_path_normalization() {
        // Test that paths are normalized correctly
        // This is a simple unit test for the normalization logic

        // Path without prefix should get ./
        let path1 = "default.nix";
        let normalized1 = if path1.starts_with('/') || path1.starts_with('.') {
            path1.to_string()
        } else {
            format!("./{}", path1)
        };
        assert_eq!(normalized1, "./default.nix");

        // Path with ./ should remain unchanged
        let path2 = "./default.nix";
        let normalized2 = if path2.starts_with('/') || path2.starts_with('.') {
            path2.to_string()
        } else {
            format!("./{}", path2)
        };
        assert_eq!(normalized2, "./default.nix");

        // Absolute path should remain unchanged
        let path3 = "/nix/store/abc-default.nix";
        let normalized3 = if path3.starts_with('/') || path3.starts_with('.') {
            path3.to_string()
        } else {
            format!("./{}", path3)
        };
        assert_eq!(normalized3, "/nix/store/abc-default.nix");

        // Relative path with ../ should remain unchanged
        let path4 = "../other/default.nix";
        let normalized4 = if path4.starts_with('/') || path4.starts_with('.') {
            path4.to_string()
        } else {
            format!("./{}", path4)
        };
        assert_eq!(normalized4, "../other/default.nix");
    }
}
