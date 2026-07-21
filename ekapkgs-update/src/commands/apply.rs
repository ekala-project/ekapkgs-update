//! Apply command for LLM-generated fixes

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use tokio::fs;
use tokio::process::Command;
use tracing::{info, warn};

use crate::commands::run::preservation::list_preserved_failures;

/// Apply LLM-generated fix to preserved worktree
pub async fn apply(
    database_path: String,
    attr_path: String,
    patch: Option<PathBuf>,
    changes_json: Option<PathBuf>,
    validate: bool,
    resume: bool,
) -> anyhow::Result<()> {
    let _expanded_path = shellexpand::tilde(&database_path).to_string();

    info!("Applying fix for: {}", attr_path);

    // Find the most recent preserved failure
    let artifacts_list = list_preserved_failures().await?;
    let artifact = artifacts_list
        .iter()
        .find(|a| a.attr_path == attr_path)
        .with_context(|| format!("No preserved artifacts found for {}", attr_path))?;

    info!(
        "Found preserved failure from session {} (phase: {})",
        artifact.session_id,
        artifact.failed_phase.as_str()
    );

    // Verify worktree exists
    if !artifact.worktree_path.exists() {
        bail!(
            "Preserved worktree no longer exists at: {}",
            artifact.worktree_path.display()
        );
    }

    // Change to the worktree directory
    std::env::set_current_dir(&artifact.worktree_path)?;

    info!("Working in: {}", artifact.worktree_path.display());

    // Apply patch if provided
    if let Some(patch_path) = patch {
        info!("Applying patch: {}", patch_path.display());
        apply_patch_file(&patch_path).await?;
        println!("✓ Patch applied successfully");
    }

    // Apply JSON-based changes if provided
    if let Some(json_path) = changes_json {
        info!("Applying changes from JSON: {}", json_path.display());
        apply_json_changes(&json_path).await?;
        println!("✓ Changes applied successfully");
    }

    // Show what changed
    info!("Checking git status...");
    let status_output = Command::new("git")
        .args(["status", "--short"])
        .output()
        .await?;

    if status_output.status.success() {
        let status_str = String::from_utf8_lossy(&status_output.stdout);
        if !status_str.trim().is_empty() {
            println!("\nModified files:");
            println!("{}", status_str);
        } else {
            println!("\nNo files were modified.");
        }
    }

    // Validate if requested
    if validate {
        println!("\nValidating changes...");

        info!("Running nix-instantiate to check syntax...");
        let instantiate_result = Command::new("nix-instantiate")
            .args(["--eval", "--strict", "-A", &attr_path])
            .output()
            .await;

        match instantiate_result {
            Ok(output) if output.status.success() => {
                println!("✓ Nix evaluation succeeded");
            },
            Ok(output) => {
                warn!("Nix evaluation failed");
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("✗ Validation failed:\n{}", stderr);
                bail!("Validation failed - see errors above");
            },
            Err(e) => {
                warn!("Failed to run nix-instantiate: {}", e);
                println!("⚠ Could not validate (nix-instantiate not available)");
            },
        }

        // Try to build
        info!("Attempting to build package...");
        println!("\nAttempting to build {}...", attr_path);
        let build_result = Command::new("nix-build")
            .args(["-A", &attr_path])
            .output()
            .await;

        match build_result {
            Ok(output) if output.status.success() => {
                println!("✓ Build succeeded");
            },
            Ok(output) => {
                warn!("Build failed");
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("✗ Build failed:\n{}", stderr);
                bail!("Build validation failed");
            },
            Err(e) => {
                warn!("Failed to run nix-build: {}", e);
                println!("⚠ Could not build (nix-build not available)");
            },
        }
    }

    // Resume update if requested
    if resume {
        warn!("Resume functionality not yet fully implemented");
        println!("\n⚠ Note: Automatic resume is not yet implemented.");
        println!("To continue manually:");
        println!("  1. Commit the changes: git add . && git commit -m 'Fix: ...'");
        println!("  2. Run: ekapkgs-update retry {}", attr_path);
    } else {
        println!("\n{}", "=".repeat(80));
        println!("Fix Applied Successfully");
        println!("{}", "=".repeat(80));
        println!();
        println!("Working directory: {}", artifact.worktree_path.display());
        println!("Package:           {}", attr_path);
        println!();
        println!("Next steps:");
        println!("  1. Review changes: git diff");
        println!("  2. Test manually:  nix-build -A {}", attr_path);
        println!("  3. Commit:         git add . && git commit");
        println!("  4. Continue:       ekapkgs-update retry {}", attr_path);
        println!();
    }

    Ok(())
}

/// Apply a patch file using git apply
async fn apply_patch_file(patch_path: &Path) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(["apply", "--verbose", "--check"])
        .arg(patch_path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Patch dry-run failed: {}", stderr);
    }

    // Actually apply the patch
    let output = Command::new("git")
        .args(["apply", "--verbose"])
        .arg(patch_path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to apply patch: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        info!("Patch output: {}", stdout);
    }

    Ok(())
}

/// Apply structured JSON operations to files.
///
/// This is the shared implementation used by both the `apply` CLI command and
/// the autofix pipeline. The `changes` value must follow this format:
///
/// ```json
/// {
///   "files": [
///     {
///       "path": "path/to/file.nix",
///       "operations": [
///         {"type": "replace", "old": "...", "new": "..."},
///         {"type": "insert_after", "marker": "...", "content": "..."}
///       ]
///     }
///   ]
/// }
/// ```
///
/// File paths in the JSON are resolved relative to `base_dir`.
pub async fn apply_json_operations(
    changes: &serde_json::Value,
    base_dir: &Path,
) -> anyhow::Result<()> {
    let files = changes["files"]
        .as_array()
        .context("JSON must have 'files' array")?;

    for file_change in files {
        let file_path_str = file_change["path"]
            .as_str()
            .context("Each file must have 'path'")?;
        let operations = file_change["operations"]
            .as_array()
            .context("Each file must have 'operations' array")?;

        let file_path = base_dir.join(file_path_str);
        info!("Applying changes to: {}", file_path.display());

        let mut content = fs::read_to_string(&file_path).await?;

        for operation in operations {
            let op_type = operation["type"]
                .as_str()
                .context("Each operation must have 'type'")?;

            match op_type {
                "replace" => {
                    let old = operation["old"]
                        .as_str()
                        .context("'replace' operation needs 'old'")?;
                    let new = operation["new"]
                        .as_str()
                        .context("'replace' operation needs 'new'")?;

                    if !content.contains(old) {
                        warn!(
                            "Could not find text to replace in {}: {}",
                            file_path.display(),
                            old
                        );
                        bail!("Replace operation failed - text not found");
                    }

                    content = content.replace(old, new);
                },
                "insert_after" => {
                    let marker = operation["marker"]
                        .as_str()
                        .context("'insert_after' operation needs 'marker'")?;
                    let insert_content = operation["content"]
                        .as_str()
                        .context("'insert_after' operation needs 'content'")?;

                    if let Some(pos) = content.find(marker) {
                        let insert_pos = pos + marker.len();
                        content.insert_str(insert_pos, insert_content);
                    } else {
                        warn!(
                            "Could not find marker in {}: {}",
                            file_path.display(),
                            marker
                        );
                        bail!("Insert operation failed - marker not found");
                    }
                },
                _ => {
                    warn!("Unknown operation type: {}", op_type);
                    bail!("Unsupported operation type: {}", op_type);
                },
            }
        }

        fs::write(&file_path, content).await?;
        info!("Applied changes to: {}", file_path.display());
    }

    Ok(())
}

/// Apply changes from a JSON file (wrapper around [`apply_json_operations`]).
async fn apply_json_changes(json_path: &Path) -> anyhow::Result<()> {
    let content = fs::read_to_string(json_path).await?;
    let changes: serde_json::Value =
        serde_json::from_str(&content).context("Failed to parse changes JSON")?;

    let base_dir = std::env::current_dir().context("Failed to get current directory")?;
    apply_json_operations(&changes, &base_dir).await
}
