use regex::Regex;
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
