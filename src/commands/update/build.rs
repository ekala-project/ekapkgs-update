use regex::Regex;
use std::path::PathBuf;
use tokio::process::Command;
use tracing::debug;

/// Build Nix expression and return stdout/stderr
///
/// Returns a tuple of (success, stdout, stderr)
pub async fn build_nix_expr(
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

/// Build a flake package and return stdout/stderr
///
/// Uses `nix build <installable>` to build flake packages.
///
/// # Arguments
/// * `installable` - The flake installable path (e.g., ".#hello" or
///   ".#packages.x86_64-linux.hello")
/// * `attr_suffix` - Optional suffix to append (e.g., "passthru.tests")
///
/// # Returns
/// A tuple of (success, stdout, stderr)
pub async fn build_flake_package(
    installable: &str,
    attr_suffix: Option<&str>,
) -> anyhow::Result<(bool, String, String)> {
    let full_installable = if let Some(suffix) = attr_suffix {
        format!("{}.{}", installable, suffix)
    } else {
        installable.to_string()
    };

    debug!("Building flake package: {}", full_installable);

    let output = Command::new("nix")
        .arg("build")
        .arg(&full_installable)
        .arg("--no-link") // Don't create result symlink
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((output.status.success(), stdout, stderr))
}

/// Detect reversed patch errors and extract the patch filename
///
/// Looks for "Reversed (or previously applied) patch detected!" in the last 20 lines
/// and extracts the patch name from the preceding "applying patch" line.
///
/// Returns the patch filename to be removed from the patches array.
pub fn detect_reversed_patch(stderr: &str) -> Option<String> {
    let patch_regex = Regex::new(r"applying patch /nix/store/[^-]+-(.+)").ok()?;

    // Take last 20 lines in reverse order
    let last_lines: Vec<_> = stderr.lines().rev().take(20).collect();

    // Find position of error message
    last_lines
        .iter()
        .position(|line| line.contains("Reversed (or previously applied) patch detected!"))
        .and_then(|error_pos| {
            // From error position onward, find first patch name
            last_lines[error_pos..].iter().find_map(|line| {
                patch_regex
                    .captures(line)
                    .and_then(|caps| caps.get(1))
                    .map(|m| m.as_str().to_string())
            })
        })
}

/// Build a Nix expression and return all output paths
///
/// Builds the package and returns a list of (output_name, store_path) tuples
/// for all outputs (e.g., "out", "dev", "lib", etc.)
///
/// # Arguments
/// * `eval_entry_point` - Path to the Nix file to evaluate
/// * `attr_path` - Attribute path to build
/// * `attr_suffix` - Optional suffix to append
///
/// # Returns
/// A vector of (output_name, store_path) tuples, or an error if the build failed
pub async fn build_and_get_outputs(
    eval_entry_point: &str,
    attr_path: &str,
    attr_suffix: Option<&str>,
) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let full_attr = if let Some(suffix) = attr_suffix {
        format!("{}.{}", attr_path, suffix)
    } else {
        attr_path.to_string()
    };

    debug!("Building {} and extracting outputs", full_attr);

    // Build with nix-build, creating result symlinks
    let output = Command::new("nix-build")
        .arg(eval_entry_point)
        .arg("-A")
        .arg(&full_attr)
        .arg("-o")
        .arg("result")
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Build failed for {}: {}", full_attr, stderr);
    }

    // Get all output paths by reading result symlinks
    let mut outputs = Vec::new();

    // Check for the main "result" symlink
    if std::path::Path::new("result").exists() {
        let store_path = std::fs::canonicalize("result")?;
        outputs.push(("out".to_string(), store_path));
    }

    // Check for additional outputs (result-dev, result-lib, etc.)
    let output_suffixes = ["dev", "lib", "debug", "doc", "man", "info", "bin"];
    for suffix in &output_suffixes {
        let link_name = format!("result-{}", suffix);
        if std::path::Path::new(&link_name).exists() {
            let store_path = std::fs::canonicalize(&link_name)?;
            outputs.push((suffix.to_string(), store_path));
        }
    }

    Ok(outputs)
}

/// Clean up result symlinks created by nix-build
///
/// Removes result, result-dev, result-lib, etc. symlinks from the current directory
pub fn cleanup_result_symlinks() -> anyhow::Result<()> {
    let symlinks = [
        "result",
        "result-dev",
        "result-lib",
        "result-debug",
        "result-doc",
        "result-man",
        "result-info",
        "result-bin",
    ];

    for link in &symlinks {
        if std::path::Path::new(link).exists() {
            std::fs::remove_file(link)?;
        }
    }

    Ok(())
}
