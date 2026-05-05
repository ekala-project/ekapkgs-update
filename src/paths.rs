//! Centralized filesystem path helpers for ekapkgs-update.
//!
//! All on-disk state for the tool lives under a single XDG cache directory
//! resolved via [`directories::ProjectDirs`]. This module is the canonical
//! source of those paths so callers don't repeat the `ProjectDirs::from`
//! incantation (and the associated error handling) inline.
//!
//! Layout under the cache root:
//! - `eval-cache/{commit}-{file_hash}.json` — cached `nix-eval-jobs` output
//! - `logs/nix-eval-jobs.stderr.log` — captured stderr from nix-eval-jobs
//! - `worktrees/update-{attr-path}/` — per-package git worktrees
//! - `updates.db` — SQLite tracking database (default location)

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Application identifier passed to `directories::ProjectDirs`.
const APP_NAME: &str = "ekapkgs-update";

/// Default path for the SQLite tracking database, expressed as a tilde-prefixed
/// string so it is human-readable in `--help` output. The runtime expands this
/// before opening the database.
pub const DEFAULT_DATABASE_PATH: &str = "~/.cache/ekapkgs-update/updates.db";

/// Returns the XDG cache directory used by this tool (typically
/// `~/.cache/ekapkgs-update`).
///
/// # Errors
/// Returns an error if `ProjectDirs` cannot determine a home directory (e.g.
/// `$HOME` is unset and there is no fallback).
pub fn cache_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", APP_NAME)
        .context("Failed to determine cache directory")?;
    Ok(dirs.cache_dir().to_path_buf())
}

/// Returns the directory used to store cached `nix-eval-jobs` results.
///
/// # Errors
/// Propagates errors from [`cache_dir`].
pub fn eval_cache_dir() -> Result<PathBuf> {
    Ok(cache_dir()?.join("eval-cache"))
}

/// Returns the directory used to store nix-eval-jobs stderr logs.
///
/// # Errors
/// Propagates errors from [`cache_dir`].
pub fn logs_dir() -> Result<PathBuf> {
    Ok(cache_dir()?.join("logs"))
}

/// Returns the directory used to store per-package git worktrees.
///
/// # Errors
/// Propagates errors from [`cache_dir`].
pub fn worktrees_dir() -> Result<PathBuf> {
    Ok(cache_dir()?.join("worktrees"))
}
