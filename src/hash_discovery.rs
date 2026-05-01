use anyhow::Context;
use regex::Regex;
use tokio::process::Command;
use tracing::{info, warn};

use crate::nix::normalize_entry_point;

/// Extract hash from Nix build error output
///
/// Nix error format: "got: sha256-<hash>"
fn extract_hash_from_error(stderr: &str) -> Option<String> {
    let hash_regex = Regex::new(r"got:\s+(sha256-[A-Za-z0-9+/=]+)").ok()?;
    let caps = hash_regex.captures(stderr)?;
    Some(caps.get(1)?.as_str().to_string())
}

/// Configuration for hash discovery
#[derive(Debug)]
pub struct HashDiscoveryConfig<'a> {
    /// Entry point for Nix evaluation (e.g., "default.nix")
    pub eval_entry_point: &'a str,
    /// Package attribute path (e.g., "pkgs.hello")
    pub attr_path: &'a str,
    /// Optional attribute suffix for building (e.g., "src" for source hash)
    pub attr_suffix: Option<&'a str>,
    /// Path to the file to update
    pub file_path: &'a str,
    /// Name of the hash attribute in the file (e.g., "sha256", "cargoHash")
    pub hash_attr_name: &'a str,
    /// Old hash value to replace
    pub old_hash: &'a str,
}

impl<'a> HashDiscoveryConfig<'a> {
    /// Create a new hash discovery configuration
    pub fn new(
        eval_entry_point: &'a str,
        attr_path: &'a str,
        attr_suffix: Option<&'a str>,
        file_path: &'a str,
        hash_attr_name: &'a str,
        old_hash: &'a str,
    ) -> Self {
        Self {
            eval_entry_point,
            attr_path,
            attr_suffix,
            file_path,
            hash_attr_name,
            old_hash,
        }
    }
}

/// Discover the correct hash by attempting to build with an invalid hash
///
/// This function:
/// 1. Updates the file with a known invalid hash
/// 2. Attempts to build (which will fail with a hash mismatch error)
/// 3. Extracts the correct hash from the error message
/// 4. Updates the file with the correct hash
/// 5. Verifies the build succeeds
///
/// # Arguments
/// * `config` - Hash discovery configuration
/// * `update_hash_fn` - Function to update the hash in the file
///
/// # Returns
/// The discovered correct hash
pub async fn discover_and_update_hash<F, Fut>(
    config: HashDiscoveryConfig<'_>,
    update_hash_fn: F,
) -> anyhow::Result<String>
where
    F: Fn(String, String, String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    // Step 1: Set invalid hash
    info!(
        "Setting invalid {} in {}",
        config.hash_attr_name, config.file_path
    );
    update_hash_fn(
        config.file_path.to_string(),
        config.old_hash.to_string(),
        invalid_hash.to_string(),
    )
    .await?;

    // Step 2: Build to discover correct hash
    let build_attr = if let Some(suffix) = config.attr_suffix {
        format!("{}.{}", config.attr_path, suffix)
    } else {
        config.attr_path.to_string()
    };

    let normalized_entry = normalize_entry_point(config.eval_entry_point);
    let build_result = Command::new("nix-build")
        .arg("-A")
        .arg(&build_attr)
        .arg(&normalized_entry)
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("Failed to run nix-build")?;

    if build_result.status.success() {
        warn!(
            "Build succeeded with invalid {} - this shouldn't happen",
            config.hash_attr_name
        );
        anyhow::bail!(
            "Expected {} mismatch error but build succeeded",
            config.hash_attr_name
        );
    }

    // Step 3: Extract correct hash from error
    let stderr = String::from_utf8_lossy(&build_result.stderr);
    let correct_hash = extract_hash_from_error(&stderr).ok_or_else(|| {
        anyhow::anyhow!(
            "Could not extract correct {} from build error:\n{}",
            config.hash_attr_name,
            stderr
        )
    })?;

    info!("Discovered correct {}: {}", config.hash_attr_name, correct_hash);

    // Step 4: Update with correct hash
    update_hash_fn(
        config.file_path.to_string(),
        invalid_hash.to_string(),
        correct_hash.clone(),
    )
    .await?;

    info!(
        "Updated {} in {}",
        config.hash_attr_name, config.file_path
    );

    // Step 5: Verify build succeeds
    let verify_result = Command::new("nix-build")
        .arg("-A")
        .arg(&build_attr)
        .arg(&normalized_entry)
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("Failed to run verification build")?;

    if !verify_result.status.success() {
        let stderr = String::from_utf8_lossy(&verify_result.stderr);
        anyhow::bail!(
            "Build failed after {} update:\n{}",
            config.hash_attr_name,
            stderr
        );
    }

    info!("{} build successful", config.hash_attr_name);

    Ok(correct_hash)
}

/// Discover hash without the update-verify cycle
///
/// This is useful when you just want to discover the hash without updating the file,
/// such as in the variant discovery case where you're working with temporary content.
///
/// # Arguments
/// * `eval_entry_point` - Entry point for Nix evaluation
/// * `attr_path` - Full attribute path to build (including any suffixes)
/// * `stderr_output` - The stderr output from a failed build attempt
///
/// # Returns
/// The extracted hash from the error, or None if not found
pub fn extract_hash(stderr_output: &str) -> Option<String> {
    extract_hash_from_error(stderr_output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_hash_from_error() {
        let stderr = r#"
error: hash mismatch in fixed-output derivation '/nix/store/...':
  specified: sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
  got:     sha256-abcdef1234567890ABCDEF1234567890ABCDEF12=
"#;

        let hash = extract_hash_from_error(stderr);
        assert_eq!(hash, Some("sha256-abcdef1234567890ABCDEF1234567890ABCDEF12=".to_string()));
    }

    #[test]
    fn test_extract_hash_no_match() {
        let stderr = "Some error without a hash";
        let hash = extract_hash_from_error(stderr);
        assert_eq!(hash, None);
    }
}
