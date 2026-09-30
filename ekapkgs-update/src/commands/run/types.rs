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
