/// Message sent from release checker service to updater service
#[derive(Debug, Clone)]
pub struct UpdateRequest {
    pub attr_path: String,
    /// Derivation store path for failure recording. `None` when the request
    /// originates from a source that doesn't have nix-eval-jobs output (e.g.
    /// the `watch` command).
    pub drv_path: Option<String>,
    pub current_version: String,
    pub new_version: String,

    // ── Multi-repo routing (all optional for backwards compatibility) ──
    /// Name of the source repo (from config). Empty/None for single-repo mode.
    pub repo_name: Option<String>,
    /// Eval entry point for this repo (absolute path). When set, overrides the
    /// global `eval_entry_point` in `UpdaterServiceConfig`.
    pub eval_entry_point: Option<String>,
    /// PR config for the target repo. When set, overrides the global config.
    pub pr_config: Option<crate::git::PrConfig>,
    /// Fork remote for this repo. When set, overrides the global fork.
    pub fork: Option<String>,
}

/// Result of an update operation
#[derive(Debug)]
pub enum UpdateResult {
    Updated {
        old_version: String,
        new_version: String,
    },
    Skipped(String),
    DryRun {
        current_version: String,
        new_version: String,
    },
}
