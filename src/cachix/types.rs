//! Types describing the result of a Cachix push.
//!
//! Currently a thin summary used only for log output; nothing in this crate
//! renders this into the PR body.

/// Lightweight summary of a successful Cachix push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachixPushSummary {
    /// Name of the Cachix cache the paths were pushed to.
    pub cache_name: String,

    /// Number of store paths that were uploaded.
    pub pushed_count: usize,
}
