//! Configuration structures for run mode operations

use crate::git::PrConfig;

/// Configuration for automated run mode
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Nix file entry point to evaluate
    pub file: String,

    /// Path to SQLite database for tracking updates
    pub database_path: String,

    /// Upstream git remote for PR creation
    pub upstream: Option<String>,

    /// Fork git remote for PR creation
    pub fork: String,

    /// Whether to run passthru.tests for packages
    pub run_passthru_tests: bool,

    /// Dry-run mode (don't actually perform updates)
    pub dry_run: bool,

    /// Number of concurrent update workers
    pub concurrent_updates: Option<usize>,

    /// Skip packages marked as unstable
    pub skip_unstable: bool,
}

/// Configuration for the updater service
#[derive(Debug, Clone)]
pub struct UpdaterServiceConfig {
    /// Nix file entry point
    pub eval_entry_point: String,

    /// Pull request configuration (if enabled)
    pub pr_config: Option<PrConfig>,

    /// Fork git remote
    pub fork: String,

    /// Whether to run passthru.tests
    pub run_passthru_tests: bool,

    /// Dry-run mode
    pub dry_run: bool,

    /// Number of concurrent update workers
    pub concurrency: usize,
}
