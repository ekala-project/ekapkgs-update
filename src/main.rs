use clap::Parser;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

mod cli;

/// Increase the file descriptor limit to avoid "Too many open files" errors
fn increase_fd_limit() -> anyhow::Result<()> {
    use rlimit::Resource;

    // Get current limits
    let (soft, hard) = Resource::NOFILE.get()?;

    // Try to set soft limit to hard limit (maximum allowed)
    if soft < hard {
        Resource::NOFILE.set(hard, hard)?;
        tracing::debug!("Increased file descriptor limit from {} to {}", soft, hard);
    }

    Ok(())
}

mod commands;
mod config;
mod cve;
mod database;
mod directory_diff;
mod git;
mod github;
mod gitlab;
mod hash_discovery;
mod nix;
mod package;
mod pypi;
mod repology;
mod rewrite;
mod variant_strategy;
mod vcs_sources;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Increase file descriptor limit to avoid "Too many open files" errors
    // when processing many packages concurrently
    if let Err(e) = increase_fd_limit() {
        tracing::warn!("Failed to increase file descriptor limit: {}", e);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with_ansi(true)
        .with_level(true)
        .with_target(true)
        .with_timer(tracing_subscriber::fmt::time())
        .init();

    let args = cli::Args::parse();
    args.command.execute().await
}
