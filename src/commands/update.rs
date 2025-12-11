use std::env;

use anyhow::Context;
use regex::Regex;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::github::{
    extract_version_from_tag, fetch_latest_github_release, is_version_newer, parse_github_url,
};
use crate::nix::eval_nix_expr;
use crate::rewrite::{
    find_and_update_attr, is_patches_array_empty, remove_patch_from_array, remove_patches_attribute,
};

pub async fn update(file: String, attr_path: String) -> anyhow::Result<()> {
    info!("Checking for update script for {}", attr_path);

    // Check if an update script is defined for this package
    let nix_expr = format!(
        "with import ./{} {{ }}; toString {}.updateScript",
        file, attr_path
    );

    let script_path_result = eval_nix_expr(&nix_expr).await;

    // If the command failed, there's no update script
    if script_path_result.is_err() {
        debug!("No update script found for {}", attr_path);
        if let Err(e) = &script_path_result {
            debug!("nix-instantiate stderr: {}", e);
        }

        // Try to find the package file location via meta.position
        debug!("Attempting to locate package definition...");
        let position_expr = format!("with import ./{} {{ }}; {}.meta.position", file, attr_path);

        let expr_file_path = eval_nix_expr(&position_expr).await.and_then(|position| {
            if position.is_empty() {
                return Err(anyhow::anyhow!(
                    "Empty position returned from meta.position"
                ));
            }
            // Parse position string (format: "file:line")
            let (file_path, _line_str) = position
                .rsplit_once(':')
                .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {}", position))?;
            Ok(file_path.to_string())
        })?;

        update_from_file_path(file, attr_path, expr_file_path).await?;

        return Ok(());
    }

    // Parse the script path from the output
    let script_path = script_path_result?;

    if script_path.is_empty() {
        return Err(anyhow::anyhow!(
            "Empty script path returned from nix-instantiate"
        ));
    }

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
        return Err(anyhow::anyhow!(
            "Update script failed with exit code: {}",
            status.code().unwrap_or(-1)
        ));
    }

    info!("Update script completed successfully for {}", &attr_path);

    Ok(())
}

// Data structure for package metadata
#[derive(Debug)]
struct PackageMetadata {
    version: String,
    src_url: Option<String>,
    output_hash: Option<String>,
}

/// Extract package metadata from Nix evaluation
async fn extract_package_metadata(
    eval_entry_point: &str,
    attr_path: &str,
) -> anyhow::Result<PackageMetadata> {
    debug!("Extracting metadata for {}", attr_path);

    // Normalize the entry point to a valid Nix filepath
    let eval_path = if eval_entry_point.starts_with('/') || eval_entry_point.starts_with('.') {
        eval_entry_point.to_string()
    } else {
        format!("./{}", eval_entry_point)
    };

    // Try to get version directly
    let version_expr = format!(
        "with import {} {{ }}; {}.version or (builtins.parseDrvName {}.name).version",
        eval_path, attr_path, attr_path
    );

    let version = eval_nix_expr(&version_expr)
        .await
        .context("Could not determine package version")?;

    // Try to get source URL
    let url_expr = format!(
        "with import {} {{ }}; builtins.toString ({}.src.url or {}.src.urls or \"\")",
        eval_path, attr_path, attr_path
    );

    let src_url = eval_nix_expr(&url_expr)
        .await
        .ok()
        .and_then(|url| if url.is_empty() { None } else { Some(url) });

    // Try to get output hash
    let hash_expr = format!(
        "with import {} {{ }}; {}.src.outputHash or \"\"",
        eval_path, attr_path
    );

    let output_hash = eval_nix_expr(&hash_expr)
        .await
        .ok()
        .and_then(|hash| if hash.is_empty() { None } else { Some(hash) });

    Ok(PackageMetadata {
        version,
        src_url,
        output_hash,
    })
}

/// Update version and hash attributes in Nix file using AST manipulation
async fn update_nix_file(
    file_path: &str,
    old_version: &str,
    new_version: &str,
    old_hash: Option<&str>,
    new_hash: Option<&str>,
) -> anyhow::Result<()> {
    debug!("Updating Nix file at {} using AST manipulation", file_path);
    let content = tokio::fs::read_to_string(file_path).await?;

    let updated_content =
        find_and_update_attr(&content, "version", new_version, Some(old_version))?;
    debug!(
        "Updated version attribute: {} -> {}",
        old_version, new_version
    );

    // Update hash if provided
    let final_content = if let (Some(old_h), Some(new_h)) = (old_hash, new_hash) {
        // Try different hash attribute names
        let hash_attrs = vec!["hash", "sha256", "outputHash"];
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
    } else {
        updated_content
    };

    // Write back to file
    tokio::fs::write(file_path, final_content).await?;
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

    // Look for the reversed patch error message
    for (i, line) in last_lines.iter().enumerate() {
        if line.contains("Reversed (or previously applied) patch detected!") {
            // Look backward for the "applying patch" line
            for j in (0..i).rev() {
                let prev_line = last_lines[j];
                // Pattern: "applying patch /nix/store/${hash}-${name}"
                let patch_regex = Regex::new(r"applying patch /nix/store/[^-]+-(.+)").ok()?;
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

/// Update the nix expr generically
async fn update_from_file_path(
    eval_entry_point: String,
    attr_path: String,
    file_location: String,
) -> anyhow::Result<()> {
    info!(
        "Starting generic update for {} at {}",
        attr_path, file_location
    );

    // Step 1: Extract package metadata
    let metadata = extract_package_metadata(&eval_entry_point, &attr_path).await?;
    info!("Current version: {}", metadata.version);

    // Step 2: Parse source URL for GitHub
    let src_url = metadata
        .src_url
        .context("No source URL found for package")?;

    let github_repo =
        parse_github_url(&src_url).context("Source is not from GitHub, skipping generic update")?;

    info!("GitHub repo: {}/{}", github_repo.owner, github_repo.repo);

    // Step 3: Get GitHub token from environment (optional)
    let github_token = env::var("GITHUB_TOKEN").ok();

    if github_token.is_none() {
        warn!(
            "GITHUB_TOKEN not set - using unauthenticated GitHub API (60 requests/hour rate limit)"
        );
    }

    // Step 4: Fetch latest release
    let latest_release = fetch_latest_github_release(
        &github_repo.owner,
        &github_repo.repo,
        github_token.as_deref(),
    )
    .await?;

    if latest_release.prerelease {
        info!("Latest release is a prerelease, skipping");
        return Ok(());
    }

    let new_version = extract_version_from_tag(&latest_release.tag_name).to_string();
    info!("Latest release: {}", new_version);

    // Step 5: Compare versions
    if !is_version_newer(&metadata.version, &new_version)? {
        info!(
            "Package is already at latest version (current: {}, latest: {})",
            metadata.version, new_version
        );
        return Ok(());
    }

    info!(
        "Found newer version: {} -> {}",
        metadata.version, new_version
    );

    // Step 6: Update version in file with invalid hash
    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    update_nix_file(
        &file_location,
        &metadata.version,
        &new_version,
        metadata.output_hash.as_deref(),
        Some(invalid_hash),
    )
    .await?;

    info!("Updated version and set invalid hash in {}", file_location);

    // Step 7: Build source to get correct hash
    let (success, _stdout, stderr) =
        build_nix_expr(&eval_entry_point, &attr_path, Some("src")).await?;

    if success {
        warn!("Build succeeded with invalid hash - this shouldn't happen");
        return Err(anyhow::anyhow!(
            "Expected hash mismatch error but build succeeded"
        ));
    }

    let correct_hash = extract_hash_from_error(&stderr).ok_or_else(|| {
        anyhow::anyhow!(
            "Could not extract correct hash from build error:\n{}",
            stderr
        )
    })?;

    info!("Extracted correct hash: {}", correct_hash);

    // Step 8: Update hash with correct value
    update_nix_file(
        &file_location,
        &new_version, // version stays the same
        &new_version,
        Some(invalid_hash),
        Some(&correct_hash),
    )
    .await?;

    info!("Updated hash in {}", file_location);

    // Step 9: Build source again to verify
    let (success, _stdout, stderr) =
        build_nix_expr(&eval_entry_point, &attr_path, Some("src")).await?;

    if !success {
        return Err(anyhow::anyhow!(
            "Source build failed after hash update:\n{}",
            stderr
        ));
    }

    info!("Source build successful");

    // Step 10: Build full package to verify with reversed patch recovery
    loop {
        let (success, _stdout, stderr) =
            build_nix_expr(&eval_entry_point, &attr_path, None).await?;

        if success {
            // Build succeeded - check if patches array is now empty
            let content = tokio::fs::read_to_string(&file_location).await?;
            if is_patches_array_empty(&content) {
                match remove_patches_attribute(&content) {
                    Ok(updated_content) => {
                        tokio::fs::write(&file_location, updated_content).await?;
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
            let content = tokio::fs::read_to_string(&file_location).await?;

            // Remove the patch
            match remove_patch_from_array(&content, &patch_name) {
                Ok(updated_content) => {
                    // Write the updated content back
                    tokio::fs::write(&file_location, updated_content).await?;
                    debug!("Removed obsolete patch: {}", patch_name);
                    // Continue loop to retry the build
                },
                Err(e) => {
                    warn!("Failed to remove patch {}: {}", patch_name, e);
                    // Can't remove the patch, return the original error
                    return Err(anyhow::anyhow!(
                        "Package build failed after update. Detected reversed patch but couldn't \
                         remove it: {}\n{}",
                        e,
                        stderr
                    ));
                },
            }
        } else {
            // No reversed patch detected - this is a real build failure
            warn!("Full package build failed:\n{}", stderr);
            return Err(anyhow::anyhow!(
                "Package build failed after update. You may need to manually fix build issues."
            ));
        }
    }

    info!(
        "✓ Successfully updated {} from {} to {}",
        attr_path, metadata.version, new_version
    );

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
