//! File formatting support using nixfmt

use std::path::Path;

use tokio::process::Command;
use tracing::{debug, info, warn};

/// Format a Nix file using nixfmt
///
/// This function runs `nixfmt` on the specified file to format it according to
/// the standard Nix formatting conventions.
///
/// # Arguments
/// * `file_path` - Path to the Nix file to format
///
/// # Returns
/// Ok(()) on success, error if nixfmt fails
pub async fn format_nix_file(file_path: &Path) -> anyhow::Result<()> {
    info!("Formatting {} with nixfmt...", file_path.display());

    // Check if file exists
    if !file_path.exists() {
        anyhow::bail!("File does not exist: {}", file_path.display());
    }

    // Run nixfmt
    let output = Command::new("nixfmt")
        .arg(file_path)
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "nixfmt not found. Please install it: nix-env -iA nixpkgs.nixfmt-rfc-style"
                )
            } else {
                anyhow::anyhow!("Failed to run nixfmt: {}", e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("nixfmt failed: {}", stderr);
        anyhow::bail!("nixfmt formatting failed: {}", stderr);
    }

    debug!("Successfully formatted {}", file_path.display());
    Ok(())
}
