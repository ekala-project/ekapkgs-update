mod branch_git;
mod checker;
mod claude_fix;
mod config;
pub mod preservation;
mod types;
mod updater;

pub(crate) use claude_fix::warn_if_claude_unavailable;
pub use config::{RunConfig, UpdaterServiceConfig, warn_if_branch_concurrency_ignored};
pub use types::UpdateRequest;
