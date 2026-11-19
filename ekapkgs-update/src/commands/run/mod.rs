mod branch_git;
mod checker;
mod claude_fix;
mod config;
mod group_update;
mod grouping;
pub mod preservation;
mod types;
mod updater;

pub(crate) use claude_fix::warn_if_claude_unavailable;
pub use config::{
    RunConfig, UpdaterServiceConfig, UpdaterStats, warn_if_branch_concurrency_ignored,
};
pub use grouping::{group_jobs, resolve_declarations};
pub use types::{UpdateJob, UpdateRequest};
