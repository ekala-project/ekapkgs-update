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

impl UpdateRequest {
    /// Repo scope used for database rows (`''` = default / `run` command).
    pub fn repo(&self) -> &str {
        self.repo_name.as_deref().unwrap_or("")
    }
}

/// Result of an update operation
#[derive(Debug)]
pub enum UpdateResult {
    Updated {
        old_version: String,
        new_version: String,
    },
    /// Not attempted / not applicable (e.g. rebuild threshold exceeded,
    /// worktree setup failed). Not recorded as a package failure.
    Skipped(String),
    /// The update was attempted and failed; already recorded in the
    /// database (failure log, backoff). Carries a one-line summary.
    Failed(String),
    DryRun {
        current_version: String,
        new_version: String,
    },
}
