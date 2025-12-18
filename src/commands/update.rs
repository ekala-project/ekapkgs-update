use anyhow::Context;
use regex::Regex;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::nix::{eval_nix_expr, is_many_variants_package};
use crate::package::PackageMetadata;
use crate::rewrite::{
    find_and_update_attr, is_patches_array_empty, remove_patch_from_array, remove_patches_attribute,
};
use crate::vcs_sources::{SemverStrategy, UpstreamSource};

/// Check for and run update script if it exists
///
/// Returns Ok(true) if update script was found and executed successfully,
/// Ok(false) if no update script exists, or Err if execution failed.
async fn run_update_script(file: &str, attr_path: &str) -> anyhow::Result<bool> {
    info!("Checking for update script for {}", attr_path);

    // Check if an update script is defined for this package
    let nix_expr = format!(
        "with import ./{} {{ }}; toString {}.updateScript",
        file, attr_path
    );

    let script_path_result = eval_nix_expr(&nix_expr).await;

    // If update script exists, use it
    match script_path_result {
        Ok(script_path) if !script_path.is_empty() => {
            info!("Found update script: {}", script_path);

            // Execute the update script
            debug!("Executing update script...");
            let status = Command::new(&script_path)
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status()
                .await?;

            if !status.success() {
                anyhow::bail!(
                    "Update script failed with exit code: {}",
                    status.code().unwrap_or(-1)
                );
            }

            info!("Update script completed successfully for {}", attr_path);
            Ok(true)
        },
        Ok(_) => {
            debug!("Update script path is empty");
            Ok(false)
        },
        Err(e) => {
            debug!("No update script found for {}", attr_path);
            debug!("nix-instantiate stderr: {}", e);
            Ok(false)
        },
    }
}

pub async fn update(
    file: String,
    attr_path: String,
    semver_strategy: String,
    ignore_update_script: bool,
    commit: bool,
) -> anyhow::Result<()> {
    // Parse semver strategy
    let strategy = SemverStrategy::from_str(&semver_strategy)?;
    info!("Using semver strategy: {:?}", strategy);

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
    // Try to find the package file location via meta.position
    debug!("Attempting to locate package definition...");
    let position_expr = format!("with import ./{} {{ }}; {}.meta.position", file, attr_path);

    let expr_file_path = eval_nix_expr(&position_expr).await.and_then(|position| {
        if position.is_empty() {
            anyhow::bail!("Empty position returned from meta.position");
        }
        // Parse position string (format: "file:line")
        let (file_path, _line_str) = position
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {}", position))?;
        Ok(file_path.to_string())
    })?;

    update_from_file_path(file, attr_path, expr_file_path, strategy, commit).await?;

    Ok(())
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
            let parts: Vec<&str> = line.splitn(2, ' ').collect();
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
    let commit_message = format!("{}: {} -> {}", attr_path, old_version, new_version);
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

/// Update the nix expr generically
pub async fn update_from_file_path(
    eval_entry_point: String,
    attr_path: String,
    file_location: String,
    strategy: SemverStrategy,
    commit: bool,
) -> anyhow::Result<()> {
    info!(
        "Starting generic update for {} at {}",
        attr_path, file_location
    );

    // Step 1: Extract package metadata
    let metadata = PackageMetadata::from_attr_path(&eval_entry_point, &attr_path).await?;
    info!("Current version: {}", metadata.version);

    // Step 2: Determine upstream source
    let upstream_source = if let Some(ref src_url) = metadata.src_url {
        // Try to parse URL as GitHub/GitLab/PyPI
        UpstreamSource::from_url(src_url)
            .context("Source is not from a supported VCS platform (GitHub, GitLab, PyPI)")?
    } else if let Some(ref pname) = metadata.pname {
        // If no src_url but pname exists, create PyPI source directly
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

    // Step 5: Update version in file with invalid hash
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

    // Step 6: Build source to get correct hash
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

    // Step 7: Update hash with correct value (use actual file location from step 5)
    let _ = update_nix_file(
        &eval_entry_point,
        &attr_path,
        &actual_file_location,
        &new_version, // version stays the same
        &new_version,
        Some(invalid_hash),
        Some(&correct_hash),
    )
    .await?;

    info!("Updated hash in {}", actual_file_location);

    // Step 8: Build source again to verify
    let (success, _stdout, stderr) =
        build_nix_expr(&eval_entry_point, &attr_path, Some("src")).await?;

    if !success {
        anyhow::bail!("Source build failed after hash update:\n{}", stderr);
    }

    info!("Source build successful");

    // For Rust packages, update cargoHash
    if let Some(old_cargo_hash) = &metadata.cargo_hash {
        info!("Detected Rust package, updating cargoHash");

        // Set invalid cargo hash
        let invalid_cargo_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_cargo_hash(&actual_file_location, old_cargo_hash, invalid_cargo_hash).await?;

        info!("Set invalid cargoHash in {}", actual_file_location);

        // Build full package to get correct cargo hash
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

        // Update cargoHash with correct value
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

        // Set invalid vendor hash
        let invalid_vendor_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        update_vendor_hash(&actual_file_location, old_vendor_hash, invalid_vendor_hash).await?;

        info!("Set invalid vendorHash in {}", actual_file_location);

        // Build full package to get correct vendor hash
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

        // Update vendorHash with correct value
        update_vendor_hash(
            &actual_file_location,
            invalid_vendor_hash,
            &correct_vendor_hash,
        )
        .await?;

        info!("Updated vendorHash in {}", actual_file_location);
    }

    // Step 9: Build full package to verify with reversed patch recovery
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
                        // Not a critical error, continue
                    },
                }
            }
            break;
        }

        // Build failed - check for reversed patch errors
        if let Some(patch_name) = detect_reversed_patch(&stderr) {
            debug!("Detected reversed patch: {}", patch_name);

            // Read the file
            let content = tokio::fs::read_to_string(&actual_file_location).await?;

            // Remove the patch
            match remove_patch_from_array(&content, &patch_name) {
                Ok(updated_content) => {
                    // Write the updated content back
                    tokio::fs::write(&actual_file_location, updated_content).await?;
                    debug!("Removed obsolete patch: {}", patch_name);
                    // Continue loop to retry the build
                },
                Err(e) => {
                    warn!("Failed to remove patch {}: {}", patch_name, e);
                    // Can't remove the patch, return the original error
                    anyhow::bail!(
                        "Package build failed after update. Detected reversed patch but couldn't \
                         remove it: {}\n{}",
                        e,
                        stderr
                    );
                },
            }
        } else {
            // No reversed patch detected - this is a real build failure
            warn!("Full package build failed:\n{}", stderr);
            anyhow::bail!(
                "Package build failed after update. You may need to manually fix build issues."
            );
        }
    }

    info!(
        "✓ Successfully updated {} from {} to {}",
        attr_path, metadata.version, new_version
    );

    // Create git commit if requested
    if commit {
        create_git_commit(&attr_path, &metadata.version, &new_version).await?;
    }

    Ok(())
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
