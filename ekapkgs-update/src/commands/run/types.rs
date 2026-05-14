/// Message sent from release checker service to updater service
#[derive(Debug, Clone)]
pub struct UpdateRequest {
    pub attr_path: String,
    pub drv: crate::nix::nix_eval_jobs::NixEvalDrv,
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
