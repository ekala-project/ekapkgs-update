//! Process bootstrap helpers used by `main`.
//!
//! Currently exposes a single helper, [`increase_fd_limit`], which raises
//! the soft `RLIMIT_NOFILE` to the hard limit so concurrent package
//! processing does not run into "Too many open files" errors.

use rlimit::Resource;

/// Increase the file descriptor limit to avoid "Too many open files" errors
/// when processing many packages concurrently.
///
/// Reads the current soft/hard `RLIMIT_NOFILE` and, if the soft limit is
/// below the hard limit, raises it to the hard limit. A no-op when the soft
/// limit already matches the hard limit.
///
/// # Errors
/// Returns the underlying [`std::io::Error`] from `getrlimit`/`setrlimit` if
/// either call fails.
pub fn increase_fd_limit() -> anyhow::Result<()> {
    let (soft, hard) = Resource::NOFILE.get()?;

    if soft < hard {
        Resource::NOFILE.set(hard, hard)?;
        tracing::debug!("Increased file descriptor limit from {} to {}", soft, hard);
    }

    Ok(())
}
