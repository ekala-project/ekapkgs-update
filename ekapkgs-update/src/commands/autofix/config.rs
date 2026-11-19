//! Configuration for the autofix pipeline.

use std::collections::HashMap;

use crate::config::LlmConfig;

/// Configuration for the `autofix run` command.
pub struct AutofixConfig {
    /// Path to the SQLite database.
    pub database_path: String,
    /// Nix evaluation entry point of the unnamed (`''`) repo scope (`run` /
    /// single-repo watch), e.g. `default.nix`. Mapped into each preserved
    /// worktree relative to its repository root.
    pub eval_entry_point: String,
    /// Evaluation entry point of each `[[repos]]` entry, relative to its
    /// repository root (`eval_entrypoint`, else `entrypoint`).
    pub repo_entry_points: HashMap<String, String>,
    /// Maximum LLM fix attempts per package before escalation.
    pub max_attempts: i64,
    /// Maximum number of queue items to process this run.
    pub limit: Option<usize>,
    /// Show prompts without calling the LLM.
    pub dry_run: bool,
    /// Only process items with these error types.
    pub error_types: Option<Vec<String>>,
    /// LLM configuration (from config file, merged with env vars).
    pub llm: LlmConfig,
}
