//! Subprocess wrapper around the `cachix` CLI.
//!
//! We shell out rather than implement the Cachix HTTP/NAR upload protocol to
//! match the existing pattern used for `nix-build`, `git`, and `gh`. The
//! `cachix` binary is added to `PATH` via `wrapProgram` in
//! `nix/package.nix` and to the dev shell in `nix/dev-shell.nix`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::{debug, warn};

/// Push the given store paths to a Cachix cache.
///
/// Returns the subset of paths that were successfully accepted by `cachix
/// push`. Failures are logged but do not abort: callers treat Cachix as a
/// best-effort enhancement to the PR.
pub async fn push_paths(cache_name: &str, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    debug!(
        "Pushing {} store path(s) to Cachix cache '{}'",
        paths.len(),
        cache_name
    );

    let mut cmd = Command::new("cachix");
    cmd.arg("push").arg(cache_name);
    for path in paths {
        cmd.arg(path);
    }

    let output = cmd
        .output()
        .await
        .with_context(|| format!("Failed to spawn `cachix push {cache_name}`"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        warn!(
            "cachix push failed (status={:?}): stderr={} stdout={}",
            output.status.code(),
            stderr.trim(),
            stdout.trim(),
        );
        anyhow::bail!("cachix push failed: {}", stderr.trim());
    }

    debug!(
        "cachix push succeeded for cache '{}' ({} path(s))",
        cache_name,
        paths.len()
    );

    Ok(paths.to_vec())
}

/// Convenience helper: filter a list of `(output_name, store_path)` tuples
/// (the shape returned by [`crate::commands::update::build_and_get_outputs`])
/// down to the actual filesystem paths to feed to `cachix push`.
pub fn store_paths_only<'a, I, P>(outputs: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = &'a (String, P)>,
    P: AsRef<Path> + 'a,
{
    outputs
        .into_iter()
        .map(|(_, p)| p.as_ref().to_path_buf())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_paths_only_strips_output_names() {
        let outputs = vec![
            ("out".to_owned(), PathBuf::from("/nix/store/aaa")),
            ("dev".to_owned(), PathBuf::from("/nix/store/bbb")),
        ];
        let paths = store_paths_only(&outputs);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/nix/store/aaa"),
                PathBuf::from("/nix/store/bbb"),
            ]
        );
    }

    #[test]
    fn store_paths_only_empty_input() {
        let outputs: Vec<(String, PathBuf)> = vec![];
        let paths = store_paths_only(&outputs);
        assert!(paths.is_empty());
    }
}
