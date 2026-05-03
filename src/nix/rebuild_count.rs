//! Rebuild count analysis for package updates
//!
//! This module provides functionality to calculate how many packages would be rebuilt
//! as a consequence of updating a package, using the cached double evaluation approach.

use std::collections::HashMap;
use std::path::Path;

use futures::{StreamExt, pin_mut};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::eval_cache::{CachedEvaluation, DrvInfo, extract_hash_from_drv_path};
use super::nix_eval_jobs::NixEvalItem;
use super::run_eval::run_nix_eval_jobs;

/// Results of a rebuild analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildAnalysis {
    /// Total number of packages in the updated evaluation
    pub total_packages: usize,
    /// Number of packages that will be rebuilt
    pub rebuild_count: usize,
    /// Attribute paths of packages that will be rebuilt
    pub rebuilt_packages: Vec<String>,
    /// New packages not present in base evaluation
    pub new_packages: Vec<String>,
    /// Packages removed in the update
    pub removed_packages: Vec<String>,
}

impl RebuildAnalysis {
    /// Calculate rebuild percentage
    pub fn rebuild_percentage(&self) -> f64 {
        if self.total_packages == 0 {
            0.0
        } else {
            (self.rebuild_count as f64 / self.total_packages as f64) * 100.0
        }
    }

    /// Check if rebuild count exceeds a threshold
    pub fn exceeds_threshold(&self, threshold: usize) -> bool {
        self.rebuild_count > threshold
    }

    /// Get a summary string for logging or PR descriptions
    pub fn summary(&self) -> String {
        format!(
            "{} packages affected ({:.1}% of {} total)",
            self.rebuild_count,
            self.rebuild_percentage(),
            self.total_packages
        )
    }

    /// Export as JSON string
    pub fn to_json(&self) -> anyhow::Result<String> {
        serde_json::to_string_pretty(self).map_err(Into::into)
    }

    /// Export to JSON file
    pub async fn to_json_file(&self, path: impl AsRef<std::path::Path>) -> anyhow::Result<()> {
        let json = self.to_json()?;
        tokio::fs::write(path, json).await?;
        Ok(())
    }

    /// Load from JSON file
    pub async fn from_json_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let contents = tokio::fs::read_to_string(path).await?;
        serde_json::from_str(&contents).map_err(Into::into)
    }
}

/// Calculate rebuild count by comparing evaluations before and after an update
///
/// This function:
/// 1. Loads the cached base evaluation (or evaluates if not cached)
/// 2. Evaluates the updated state in the worktree
/// 3. Compares derivation hashes to determine what will rebuild
///
/// # Arguments
/// * `eval_file` - Path to the Nix file to evaluate (relative to repo root)
/// * `worktree_path` - Path to the git worktree containing the update
///
/// # Returns
/// Detailed analysis of rebuild impact
pub async fn calculate_rebuild_count(
    eval_file: &str,
    worktree_path: &Path,
) -> anyhow::Result<RebuildAnalysis> {
    // Get current commit (base)
    let current_commit = get_current_commit().await?;

    // Load or evaluate base state (cached)
    info!("Loading base evaluation for commit {}", current_commit);
    let base_eval = CachedEvaluation::load_or_create(&current_commit, eval_file).await?;

    // Evaluate updated state (in worktree)
    info!(
        "Evaluating updated state in worktree: {}",
        worktree_path.display()
    );
    let updated_eval = evaluate_in_worktree(worktree_path, eval_file).await?;

    // Compare
    let mut rebuilt = Vec::new();
    let mut new_packages = Vec::new();
    let mut removed_packages = Vec::new();

    for (attr, updated_info) in &updated_eval {
        match base_eval.get(attr) {
            Some(base_info) => {
                if base_info.drv_hash != updated_info.drv_hash {
                    rebuilt.push(attr.clone());
                    debug!(
                        "Package will rebuild: {} (hash changed from {} to {})",
                        attr, base_info.drv_hash, updated_info.drv_hash
                    );
                }
            },
            None => {
                new_packages.push(attr.clone());
                debug!("New package detected: {}", attr);
            },
        }
    }

    for attr in base_eval.keys() {
        if !updated_eval.contains_key(attr) {
            removed_packages.push(attr.clone());
            debug!("Package removed: {}", attr);
        }
    }

    // Sort for consistent output
    rebuilt.sort();
    new_packages.sort();
    removed_packages.sort();

    info!(
        "Rebuild analysis complete: {} rebuilds, {} new, {} removed",
        rebuilt.len(),
        new_packages.len(),
        removed_packages.len()
    );

    Ok(RebuildAnalysis {
        total_packages: updated_eval.len(),
        rebuild_count: rebuilt.len(),
        rebuilt_packages: rebuilt,
        new_packages,
        removed_packages,
    })
}

/// Evaluate packages in a git worktree
async fn evaluate_in_worktree(
    worktree_path: &Path,
    eval_file: &str,
) -> anyhow::Result<HashMap<String, DrvInfo>> {
    // Construct path to eval file in worktree
    let worktree_eval_file = worktree_path.join(eval_file);

    if !worktree_eval_file.exists() {
        anyhow::bail!(
            "Eval file not found in worktree: {}",
            worktree_eval_file.display()
        );
    }

    let eval_file_str = worktree_eval_file
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid UTF-8 in worktree path"))?
        .to_string();

    debug!("Evaluating in worktree: {}", eval_file_str);

    let stream = run_nix_eval_jobs(eval_file_str);
    let mut packages = HashMap::new();

    pin_mut!(stream);

    while let Some(result) = stream.next().await {
        match result {
            Ok(NixEvalItem::Drv(drv)) => {
                let drv_hash = extract_hash_from_drv_path(&drv.drv_path);
                packages.insert(
                    drv.attr.clone(),
                    DrvInfo {
                        attr_path: drv.attr.clone(),
                        drv_path: drv.drv_path.clone(),
                        drv_hash,
                    },
                );
            },
            Ok(NixEvalItem::Error(err)) => {
                debug!("Evaluation error for {}: {}", err.attr, err.error);
                // Skip errors - they won't be in the comparison set
            },
            Err(e) => {
                warn!("Error during worktree evaluation: {}", e);
            },
        }
    }

    info!("Worktree evaluation completed: {} packages", packages.len());
    Ok(packages)
}

/// Get the current git commit hash
async fn get_current_commit() -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to get current commit: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Quick estimation of rebuild impact using reverse dependency graph
///
/// This provides a lower-bound estimate much faster than full evaluation,
/// useful for quick checks before committing to a full update.
///
/// # Arguments
/// * `eval_file` - Path to the Nix file to evaluate
/// * `target_attr` - The attribute that would be updated
///
/// # Returns
/// Estimated minimum number of rebuilds
pub async fn estimate_rebuild_count(eval_file: &str, target_attr: &str) -> anyhow::Result<usize> {
    info!("Estimating rebuild count for {} (fast mode)", target_attr);

    // Get current evaluation
    let current_commit = get_current_commit().await?;
    let eval_data = CachedEvaluation::load_or_create(&current_commit, eval_file).await?;

    // Build reverse dependency graph
    let reverse_deps = build_reverse_dependency_graph(&eval_data).await?;

    // Find target package's drv_path
    let target_drv = match eval_data.get(target_attr) {
        Some(info) => &info.drv_path,
        None => {
            anyhow::bail!("Target attribute {} not found in evaluation", target_attr);
        },
    };

    // Traverse reverse dependencies
    let affected = find_all_reverse_deps(target_drv, &reverse_deps);

    info!(
        "Estimated rebuild count for {}: ≥ {} packages (lower bound)",
        target_attr,
        affected.len()
    );

    Ok(affected.len())
}

/// Build a reverse dependency graph from evaluation data
///
/// Note: This only captures direct drv-to-drv dependencies and will undercount
/// actual rebuilds. Use for estimation only.
async fn build_reverse_dependency_graph(
    _eval_data: &HashMap<String, DrvInfo>,
) -> anyhow::Result<HashMap<String, Vec<String>>> {
    // This would require input_drvs information from the full evaluation
    // For now, return an empty graph as a placeholder
    // TODO: Store input_drvs in DrvInfo for proper graph construction
    warn!("Reverse dependency graph not yet implemented - returning empty graph");
    Ok(HashMap::new())
}

/// Find all packages that transitively depend on a target
fn find_all_reverse_deps(
    target_drv: &str,
    reverse_deps: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut affected = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut queue = vec![target_drv.to_string()];

    while let Some(drv) = queue.pop() {
        if !visited.insert(drv.clone()) {
            continue;
        }

        if let Some(dependents) = reverse_deps.get(&drv) {
            for dependent in dependents {
                affected.push(dependent.clone());
                queue.push(dependent.clone());
            }
        }
    }

    affected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rebuild_analysis_percentage() {
        let analysis = RebuildAnalysis {
            total_packages: 100,
            rebuild_count: 25,
            rebuilt_packages: vec![],
            new_packages: vec![],
            removed_packages: vec![],
        };

        assert_eq!(analysis.rebuild_percentage(), 25.0);
    }

    #[test]
    fn test_rebuild_analysis_exceeds_threshold() {
        let analysis = RebuildAnalysis {
            total_packages: 100,
            rebuild_count: 150,
            rebuilt_packages: vec![],
            new_packages: vec![],
            removed_packages: vec![],
        };

        assert!(analysis.exceeds_threshold(100));
        assert!(!analysis.exceeds_threshold(200));
    }

    #[test]
    fn test_rebuild_analysis_summary() {
        let analysis = RebuildAnalysis {
            total_packages: 200,
            rebuild_count: 50,
            rebuilt_packages: vec![],
            new_packages: vec![],
            removed_packages: vec![],
        };

        let summary = analysis.summary();
        assert!(summary.contains("50 packages"));
        assert!(summary.contains("25.0%"));
        assert!(summary.contains("200 total"));
    }
}
