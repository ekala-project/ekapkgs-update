//! Cachix integration: push successful build outputs to a binary cache.
//!
//! Usage is gated on three signals (any one missing → silently skipped, the
//! update is unaffected):
//!
//! 1. The caller did **not** pass `--skip-cachix`.
//! 2. A cache name is configured (CLI `--cachix-cache <NAME>` or the `CACHIX_CACHE_NAME`
//!    environment variable).
//! 3. The `CACHIX_AUTH_TOKEN` environment variable is set so the `cachix` subprocess can
//!    authenticate.
//!
//! All errors are logged at `warn` level and converted into `Ok(None)`. The
//! Cachix push is intentionally a side effect: nothing renders the result
//! into the PR body, since reviewers don't need the raw store paths.

mod push;
mod types;

use std::path::PathBuf;

use anyhow::Result;
use tracing::{debug, info, warn};

pub use self::types::CachixPushSummary;

/// Resolve which Cachix cache to push to, if any.
///
/// Precedence: CLI flag (passed via `cli_cache`) → `CACHIX_CACHE_NAME` env
/// var → `None` (Cachix push disabled).
pub fn resolve_cache_name(cli_cache: Option<&str>) -> Option<String> {
    if let Some(name) = cli_cache {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    match std::env::var("CACHIX_CACHE_NAME") {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_owned()),
        _ => None,
    }
}

/// Push the given build outputs to the configured Cachix cache.
///
/// `outputs` is the `(output_name, store_path)` shape returned by
/// `commands::update::build_and_get_outputs`. **All** entries are pushed.
///
/// Returns:
/// - `Ok(Some(summary))` on a successful push (one or more paths uploaded).
/// - `Ok(None)` when Cachix is disabled, no cache name is configured, the auth token is missing,
///   the `outputs` list is empty, or the push fails (failures are logged at `warn`).
pub async fn push_outputs(
    cache_name: Option<&str>,
    outputs: &[(String, PathBuf)],
    skip: bool,
) -> Result<Option<CachixPushSummary>> {
    if skip {
        debug!("Cachix push disabled via --skip-cachix");
        return Ok(None);
    }

    let Some(cache_name) = cache_name else {
        debug!("Cachix push skipped: no cache name configured");
        return Ok(None);
    };

    if std::env::var("CACHIX_AUTH_TOKEN").is_err() {
        warn!(
            "Cachix push skipped: CACHIX_AUTH_TOKEN not set (cache '{}' configured but \
             unauthenticated)",
            cache_name
        );
        return Ok(None);
    }

    if outputs.is_empty() {
        debug!("Cachix push skipped: no build outputs to push");
        return Ok(None);
    }

    let paths = push::store_paths_only(outputs);

    match push::push_paths(cache_name, &paths).await {
        Ok(pushed) => {
            info!(
                "Pushed {} store path(s) to Cachix cache '{}'",
                pushed.len(),
                cache_name
            );
            Ok(Some(CachixPushSummary {
                cache_name: cache_name.to_owned(),
                pushed_count: pushed.len(),
            }))
        },
        Err(e) => {
            warn!(
                "Cachix push to '{}' failed (continuing without cache): {:#}",
                cache_name, e
            );
            Ok(None)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_cli_returns_trimmed() {
        // When CLI flag is provided non-empty, it is returned (trimmed).
        let got = resolve_cache_name(Some("  from-cli  "));
        assert_eq!(got.as_deref(), Some("from-cli"));
    }

    #[test]
    fn resolve_empty_cli_falls_through_to_env() {
        // Empty / whitespace-only CLI is treated as unset; we fall through to
        // env-var resolution. We don't set the env here (would race with
        // other tests in the same process), so just assert non-empty CLI is
        // honored and empty CLI does not panic.
        assert_eq!(resolve_cache_name(Some("   ")), resolve_cache_name(None));
    }

    #[tokio::test]
    async fn push_outputs_skip_flag_short_circuits() {
        let outputs = vec![("out".to_owned(), PathBuf::from("/nix/store/aaa"))];
        let r = push_outputs(Some("any"), &outputs, true).await.unwrap();
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn push_outputs_no_cache_name_returns_none() {
        let outputs = vec![("out".to_owned(), PathBuf::from("/nix/store/aaa"))];
        let r = push_outputs(None, &outputs, false).await.unwrap();
        assert!(r.is_none());
    }
}
