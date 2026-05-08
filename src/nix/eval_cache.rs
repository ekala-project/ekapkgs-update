//! Evaluation result caching to avoid redundant nix-eval-jobs runs
//!
//! This module provides caching for nix-eval-jobs evaluation results, keyed by git commit hash.
//! This is particularly useful for rebuild count analysis where we need to compare evaluations
//! before and after updates.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use futures::{StreamExt, pin_mut};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::{debug, info};

use super::nix_eval_jobs::{NixEvalDrv, NixEvalItem};
use super::run_eval::run_nix_eval_jobs;

/// Cached evaluation result for a specific commit and eval file
#[derive(Serialize, Deserialize)]
pub struct CachedEvaluation {
    /// Git commit hash this evaluation was performed at
    pub commit_hash: String,
    /// Timestamp when this evaluation was cached
    pub timestamp: String,
    /// The eval file path that was evaluated
    pub file_path: String,
    /// Map of attribute path to derivation info
    pub packages: HashMap<String, DrvInfo>,
}

/// Essential derivation information for rebuild analysis
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DrvInfo {
    /// Full attribute path (e.g., "python.pkgs.setuptools")
    pub attr_path: String,
    /// Full derivation path (e.g., "/nix/store/abc123...-name.drv")
    pub drv_path: String,
    /// Hash extracted from drv_path for quick comparison
    pub drv_hash: String,
}

impl CachedEvaluation {
    /// Load cached evaluation or create a new one by evaluating
    ///
    /// # Arguments
    /// * `commit` - Git commit hash to use as cache key
    /// * `eval_file` - Path to the Nix file to evaluate
    ///
    /// # Returns
    /// HashMap mapping attribute paths to derivation info
    pub async fn load_or_create(
        commit: &str,
        eval_file: &str,
    ) -> anyhow::Result<HashMap<String, DrvInfo>> {
        let cache_path = Self::cache_file_path(commit, eval_file)?;

        if cache_path.exists() {
            info!(
                "Loading cached evaluation for commit {} from {}",
                commit,
                cache_path.display()
            );
            let contents = fs::read_to_string(&cache_path)
                .await
                .with_context(|| format!("read cache file {}", cache_path.display()))?;
            let cached: CachedEvaluation = serde_json::from_str(&contents)
                .with_context(|| format!("parse cache file {}", cache_path.display()))?;
            debug!("Loaded {} packages from cache", cached.packages.len());
            Ok(cached.packages)
        } else {
            info!(
                "No cache found for commit {}, performing fresh evaluation",
                commit
            );
            let packages = Self::evaluate_fresh(eval_file).await?;

            // Save to cache. We move `packages` into `cached` for serialization,
            // then return ownership by destructuring `cached.packages` afterwards
            // to avoid cloning the entire map.
            let cached = CachedEvaluation {
                commit_hash: commit.to_owned(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                file_path: eval_file.to_owned(),
                packages,
            };

            let Some(cache_parent) = cache_path.parent() else {
                anyhow::bail!(
                    "cache path has no parent directory: {}",
                    cache_path.display()
                );
            };
            fs::create_dir_all(cache_parent)
                .await
                .with_context(|| format!("create cache directory {}", cache_parent.display()))?;
            let json = serde_json::to_string_pretty(&cached)?;
            fs::write(&cache_path, json)
                .await
                .with_context(|| format!("write cache file {}", cache_path.display()))?;

            info!(
                "Cached {} packages to {}",
                cached.packages.len(),
                cache_path.display()
            );

            Ok(cached.packages)
        }
    }

    /// Perform a fresh evaluation using nix-eval-jobs
    async fn evaluate_fresh(eval_file: &str) -> anyhow::Result<HashMap<String, DrvInfo>> {
        let stream = run_nix_eval_jobs(eval_file.to_owned());
        let mut packages = HashMap::new();

        pin_mut!(stream);

        while let Some(result) = stream.next().await {
            match result {
                Ok(NixEvalItem::Drv(drv)) => {
                    // Unbox and destructure to move `attr` and `drv_path` out
                    // without cloning. The remaining clone is unavoidable: the
                    // map key requires its own owned `String` distinct from the
                    // `attr_path` field stored in `DrvInfo`.
                    let NixEvalDrv { attr, drv_path, .. } = *drv;
                    let drv_hash = extract_hash_from_drv_path(&drv_path);
                    packages.insert(
                        attr.clone(),
                        DrvInfo {
                            attr_path: attr,
                            drv_path,
                            drv_hash,
                        },
                    );
                },
                Ok(NixEvalItem::Error(err)) => {
                    debug!("Evaluation error for {}: {}", err.attr, err.error);
                    // Skip errors - they won't be in the comparison set
                },
                Err(e) => {
                    // Log but continue - partial evaluation is better than none
                    tracing::warn!("Error during evaluation: {}", e);
                },
            }
        }

        info!("Fresh evaluation completed: {} packages", packages.len());
        Ok(packages)
    }

    /// Get the cache file path for a given commit and eval file
    fn cache_file_path(commit: &str, eval_file: &str) -> anyhow::Result<PathBuf> {
        // Use a hash of the eval file path to create a stable filename
        let file_hash = hash_string(eval_file);

        Ok(crate::paths::eval_cache_dir()?
            .join(format!("{commit}-{file_hash}.json"))
            .into_std_path_buf())
    }
}

/// Extract the hash portion from a derivation path
///
/// # Example
/// ```
/// # use ekapkgs_update::nix::eval_cache::extract_hash_from_drv_path;
/// let drv_path = "/nix/store/abc123xyz-package-1.0.drv";
/// let hash = extract_hash_from_drv_path(drv_path);
/// assert_eq!(hash, "abc123xyz");
/// ```
pub fn extract_hash_from_drv_path(drv_path: &str) -> String {
    // "/nix/store/abc123...-name.drv" -> "abc123..."
    drv_path
        .strip_prefix("/nix/store/")
        .and_then(|s| s.split('-').next())
        .unwrap_or("")
        .to_owned()
}

/// Hash a string into a stable hex token used for cache-file naming.
///
/// Uses [`std::hash::DefaultHasher`] which is deterministic within a single
/// Rust version. The output is **not** stable across Rust toolchain upgrades;
/// the cache directory may accumulate orphan entries after a toolchain bump.
/// The cache itself is only an optimization, so a miss falls through to a
/// fresh evaluation, and orphan entries are inert until manually pruned.
///
/// Not cryptographically secure.
fn hash_string(s: &str) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_hash_from_drv_path() {
        let drv_path = "/nix/store/abc123xyz-package-1.0.drv";
        let hash = extract_hash_from_drv_path(drv_path);
        assert_eq!(hash, "abc123xyz");
    }

    #[test]
    fn test_extract_hash_long() {
        let drv_path = "/nix/store/3fr8b3xlygv2a64ff7fq7564j4sxv4lc-cmake-3.29.6.drv";
        let hash = extract_hash_from_drv_path(drv_path);
        assert_eq!(hash, "3fr8b3xlygv2a64ff7fq7564j4sxv4lc");
    }

    #[test]
    fn test_hash_string_deterministic() {
        let s = "default.nix";
        let hash1 = hash_string(s);
        let hash2 = hash_string(s);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_string_different() {
        let hash1 = hash_string("default.nix");
        let hash2 = hash_string("other.nix");
        assert_ne!(hash1, hash2);
    }
}
