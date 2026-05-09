//! Flake-specific Nix evaluation functionality
//!
//! This module provides functions for evaluating and interacting with Nix flakes,
//! including querying package metadata and building flake outputs.

use anyhow::Context;
use tokio::process::Command;
use tracing::debug;

/// Detect the current system (e.g., "x86_64-linux", "aarch64-darwin")
///
/// Uses `nix eval --impure --expr 'builtins.currentSystem' --raw` to determine
/// the current system.
///
/// # Returns
/// The system string (e.g., "x86_64-linux")
///
/// # Errors
/// Returns an error if the nix eval command fails
pub async fn detect_system() -> anyhow::Result<String> {
    let output = Command::new("nix")
        .arg("eval")
        .arg("--impure")
        .arg("--expr")
        .arg("builtins.currentSystem")
        .arg("--raw")
        .output()
        .await
        .context("Failed to execute nix eval for system detection")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to detect system: {}", stderr.trim());
    }

    let system = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    debug!("Detected system: {}", system);
    Ok(system)
}

/// Build a flake installable path from components
///
/// # Arguments
/// * `flake_ref` - The flake reference (e.g., ".", "github:owner/repo")
/// * `output_path` - Optional output path (e.g., "packages.x86_64-linux")
/// * `attr` - The attribute name (e.g., "hello")
///
/// # Returns
/// A complete installable path (e.g., ".#packages.x86_64-linux.hello" or ".#hello")
///
/// # Examples
/// ```
/// # use ekapkgs_update::nix::flake::build_installable;
/// assert_eq!(
///     build_installable(".", Some("packages.x86_64-linux"), "hello"),
///     ".#packages.x86_64-linux.hello"
/// );
/// assert_eq!(build_installable(".", None, "hello"), ".#hello");
/// ```
pub fn build_installable(flake_ref: &str, output_path: Option<&str>, attr: &str) -> String {
    match output_path {
        Some(path) => format!("{flake_ref}#{path}.{attr}"),
        None => format!("{flake_ref}#{attr}"),
    }
}

/// Evaluate a flake attribute and return the result as a string
///
/// Uses `nix eval <installable>.<field> --raw` to query specific fields
/// from a flake package.
///
/// # Arguments
/// * `installable` - The flake installable (e.g., ".#packages.x86_64-linux.hello")
/// * `field` - The field to query (e.g., "version", "pname")
///
/// # Returns
/// The field value as a string
///
/// # Errors
/// Returns an error if the nix eval command fails
///
/// # Example
/// ```no_run
/// # use ekapkgs_update::nix::flake::eval_flake_attr;
/// # async fn example() -> anyhow::Result<()> {
/// let version = eval_flake_attr(".#hello", "version").await?;
/// println!("Version: {}", version);
/// # Ok(())
/// # }
/// ```
pub async fn eval_flake_attr(installable: &str, field: &str) -> anyhow::Result<String> {
    let attr_path = format!("{installable}.{field}");

    debug!("Evaluating flake attribute: {}", attr_path);

    let output = Command::new("nix")
        .arg("eval")
        .arg(&attr_path)
        .arg("--raw")
        .output()
        .await
        .with_context(|| format!("Failed to execute nix eval for '{}'", attr_path))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Failed to evaluate flake attribute '{}': {}",
            attr_path,
            stderr.trim()
        );
    }

    let result = String::from_utf8_lossy(&output.stdout).trim().to_owned();

    debug!("Flake attribute '{}' = '{}'", attr_path, result);
    Ok(result)
}

/// Try to evaluate a flake attribute, returning None if it fails
///
/// This is useful for optional attributes that may not exist.
///
/// # Arguments
/// * `installable` - The flake installable
/// * `field` - The field to query
///
/// # Returns
/// * `Some(value)` if the attribute exists and evaluation succeeds
/// * `None` if the attribute doesn't exist or evaluation fails
pub async fn try_eval_flake_attr(installable: &str, field: &str) -> Option<String> {
    match eval_flake_attr(installable, field).await {
        Ok(value) => Some(value),
        Err(e) => {
            debug!(
                "Failed to evaluate optional flake attribute '{}.{}': {}",
                installable, field, e
            );
            None
        },
    }
}

/// Get package metadata from a flake
///
/// Queries various attributes from a flake package to build a PackageMetadata struct.
///
/// # Arguments
/// * `installable` - The flake installable (e.g., ".#hello" or ".#packages.x86_64-linux.hello")
///
/// # Returns
/// A PackageMetadata struct with version, pname, and other metadata
///
/// # Errors
/// Returns an error if required attributes (version) cannot be evaluated
pub async fn get_flake_package_metadata(
    installable: &str,
) -> anyhow::Result<crate::package::PackageMetadata> {
    use crate::package::PackageMetadata;

    debug!("Fetching metadata for flake package: {}", installable);

    // Required fields
    let version = eval_flake_attr(installable, "version").await?;

    // Optional fields - try to get them but don't fail if they're missing
    let pname = try_eval_flake_attr(installable, "pname").await;
    let description = try_eval_flake_attr(installable, "meta.description").await;
    let homepage = try_eval_flake_attr(installable, "meta.homepage").await;
    let changelog = try_eval_flake_attr(installable, "meta.changelog").await;

    // For src_url, we need to handle the src.url pattern
    let src_url = try_eval_flake_attr(installable, "src.url")
        .await
        .or_else(|| try_eval_flake_attr_sync(installable, "src.urls.0"));

    // Hash is typically in src.outputHash
    let output_hash = try_eval_flake_attr(installable, "src.outputHash").await;

    // Dependency hashes
    let cargo_hash = try_eval_flake_attr(installable, "cargoDeps.outputHash").await;
    let vendor_hash = try_eval_flake_attr(installable, "vendorHash")
        .await
        .or_else(|| try_eval_flake_attr_sync(installable, "vendorSha256"));
    let npm_deps_hash = try_eval_flake_attr(installable, "npmDepsHash").await;
    let nuget_deps_hash = try_eval_flake_attr(installable, "nugetDeps").await;
    let composer_deps_hash = try_eval_flake_attr(installable, "composerDepsHash").await;

    Ok(PackageMetadata {
        version,
        src_url,
        output_hash,
        cargo_hash,
        vendor_hash,
        npm_deps_hash,
        nuget_deps_hash,
        composer_deps_hash,
        pname,
        description,
        homepage,
        changelog,
        skip: None, // Flake packages don't support passthru.ekapkgs-update yet
        semver_strategy: None,
        include_prereleases: None,
    })
}

// Helper to avoid async recursion in or_else
fn try_eval_flake_attr_sync(installable: &str, field: &str) -> Option<String> {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(async { try_eval_flake_attr(installable, field).await })
    })
}

/// Get the file location where a flake package is defined
///
/// Queries `meta.position` to find the source file location.
///
/// # Arguments
/// * `installable` - The flake installable
///
/// # Returns
/// The file path where the package is defined
///
/// # Errors
/// Returns an error if meta.position cannot be evaluated or parsed
pub async fn get_flake_package_position(installable: &str) -> anyhow::Result<String> {
    let position = eval_flake_attr(installable, "meta.position").await?;

    // Position format is typically "file:line"
    // Extract just the file path
    if let Some(colon_pos) = position.find(':') {
        let file_path = &position[..colon_pos];
        debug!("Flake package defined in: {}", file_path);
        Ok(file_path.to_owned())
    } else {
        // If no colon, assume it's just a file path
        Ok(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_installable_with_output_path() {
        assert_eq!(
            build_installable(".", Some("packages.x86_64-linux"), "hello"),
            ".#packages.x86_64-linux.hello"
        );
    }

    #[test]
    fn test_build_installable_without_output_path() {
        assert_eq!(build_installable(".", None, "hello"), ".#hello");
    }

    #[test]
    fn test_build_installable_github_ref() {
        assert_eq!(
            build_installable(
                "github:NixOS/nixpkgs",
                Some("legacyPackages.x86_64-linux"),
                "hello"
            ),
            "github:NixOS/nixpkgs#legacyPackages.x86_64-linux.hello"
        );
    }
}
