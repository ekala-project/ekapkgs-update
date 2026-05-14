use tokio::process::Command;
use tracing::{debug, info};

use crate::nix::{eval_nix_expr, normalize_entry_point};

/// Check for and run update script if it exists
///
/// Returns Ok(true) if update script was found and executed successfully,
/// Ok(false) if no update script exists, or Err if execution failed.
pub async fn run_update_script(file: &str, attr_path: &str) -> anyhow::Result<bool> {
    info!("Checking for update script for {}", attr_path);

    // Check if an update script is defined for this package
    let normalized_entry = normalize_entry_point(file);
    let nix_expr =
        format!("with import {normalized_entry} {{ }}; toString {attr_path}.updateScript");

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
