use std::path::Path;

use anyhow::Context;
use tokio::fs;
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

use crate::rewrite::{replace_maintainers_with_empty, replace_teams_with_empty};

/// Prune maintainers and teams from all .nix files in a directory
///
/// This command recursively searches for all .nix files in the given directory
/// and replaces `meta.maintainers` and `meta.teams` with empty arrays `[ ]`.
///
/// # Arguments
/// * `directory` - Path to the directory to process
/// * `check` - If true, only check if changes would be made without modifying files
///
/// # Returns
/// Ok(()) if successful, or an error if the directory cannot be processed or if
/// check mode is enabled and changes would be made
pub async fn prune_maintainers(directory: String, check: bool) -> anyhow::Result<()> {
    let dir_path = Path::new(&directory);

    if !dir_path.exists() {
        anyhow::bail!("Directory does not exist: {directory}");
    }

    if !dir_path.is_dir() {
        anyhow::bail!("Path is not a directory: {directory}");
    }

    if check {
        info!(
            "Checking for maintainers/teams to prune in .nix files in: {}",
            directory
        );
    } else {
        info!(
            "Pruning maintainers/teams from .nix files in: {}",
            directory
        );
    }

    let mut processed_count = 0;
    let mut modified_count = 0;
    let mut error_count = 0;

    // Walk the directory tree looking for .nix files
    for entry in WalkDir::new(dir_path)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let path = entry.path();

        // Skip if not a .nix file
        if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("nix") {
            continue;
        }

        debug!("Processing: {}", path.display());
        processed_count += 1;

        match process_file(path).await {
            Ok(true) => {
                if check {
                    info!("Would modify: {}", path.display());
                } else {
                    info!("Modified: {}", path.display());
                }
                modified_count += 1;
            },
            Ok(false) => {
                debug!("No changes: {}", path.display());
            },
            Err(e) => {
                warn!("Error processing {}: {}", path.display(), e);
                error_count += 1;
            },
        }
    }

    if check {
        info!(
            "Check completed: {} files processed, {} would be modified, {} errors",
            processed_count, modified_count, error_count
        );
    } else {
        info!(
            "Completed: {} files processed, {} modified, {} errors",
            processed_count, modified_count, error_count
        );
    }

    if error_count > 0 {
        warn!("{} files had errors and were not modified", error_count);
    }

    if check && modified_count > 0 {
        error!(
            "Check failed: {} files would be modified by prune-maintainers",
            modified_count
        );
        anyhow::bail!(
            "Check failed: {modified_count} files would be modified by prune-maintainers"
        );
    }

    Ok(())
}

/// Process a single .nix file
///
/// # Arguments
/// * `path` - Path to the .nix file to process
/// * `check` - If true, only check if changes would be made without modifying the file
///
/// # Returns
/// Ok(true) if the file was modified (or would be modified in check mode),
/// Ok(false) if no changes were made, or an error if the file cannot be processed
async fn process_file(path: &Path) -> anyhow::Result<bool> {
    // Read the file
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("read file {}", path.display()))?;

    // Replace maintainers
    let (content, maintainers_changed) = replace_maintainers_with_empty(&content)?;

    // Replace teams
    let (content, teams_changed) = replace_teams_with_empty(&content)?;

    let changed = maintainers_changed || teams_changed;

    if changed {
        fs::write(path, content)
            .await
            .with_context(|| format!("write file {}", path.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}
