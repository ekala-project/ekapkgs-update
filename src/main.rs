use clap::{Parser, Subcommand};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

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
mod database;
mod git;
mod github;
mod gitlab;
mod hash_discovery;
mod nix;
mod package;
mod pypi;
mod rewrite;
mod variant_strategy;
mod vcs_sources;

#[cfg(test)]
mod tests;

#[derive(Parser)]
#[command(name = "ekapkgs-update")]
#[command(about = "Update ekapkgs packages", long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the update process
    Run {
        /// Nix file to evaluate
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = "~/.cache/ekapkgs-update/updates.db")]
        database: String,
        /// Upstream git remote. Inferred if left unset. E.g. nixpkgs
        #[arg(long)]
        upstream: Option<String>,
        /// Remote repository to push branches. E.g. my-fork
        #[arg(long, default_value = "origin")]
        fork: String,
        /// Run passthru.tests if available before considering update successful
        #[arg(long)]
        run_passthru_tests: bool,
        /// Check for updates without rewriting, building, committing, or creating PRs
        #[arg(long)]
        dry_run: bool,
        /// Maximum number of concurrent package updates (default: CPU cores / 4)
        #[arg(long)]
        concurrent_updates: Option<usize>,
        /// Skip packages with 'unstable' in their version
        #[arg(long)]
        skip_unstable: bool,
    },
    /// Update a package in a Nix file
    Update {
        /// Nix file to update
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// Attribute path of the package to update
        attr_path: String,
        /// Version selection strategy: latest, major, minor, or patch
        #[arg(long, default_value = "latest")]
        semver: String,
        /// Ignore update script and use generic update method
        #[arg(long, default_value = "false")]
        ignore_update_script: bool,
        /// Create a git commit after successful update
        #[arg(long)]
        commit: bool,
        /// Create a pull request after successful update (implies --commit)
        #[arg(long)]
        create_pr: bool,
        /// Upstream git remote. Inferred if left unset. E.g. nixpkgs.
        /// Only used with --create-pr.
        #[arg(long)]
        upstream: Option<String>,
        /// Remote repository to push branches. E.g. my-fork
        /// Only used with --create-pr.
        #[arg(long, default_value = "origin")]
        fork: String,
        /// Run passthru.tests if available before considering update successful
        #[arg(long)]
        run_passthru_tests: bool,
        /// For mkManyVariants packages: update only this specific variant (e.g., v1_2, v0_20)
        #[arg(long)]
        variant: Option<String>,
        /// For mkManyVariants packages: explicitly update all variants (this is the default)
        #[arg(long)]
        all_variants: bool,
        /// Enable flake mode: update a package exposed by a flake
        #[arg(long)]
        flake: bool,
        /// Flake output prefix (e.g., 'packages.x86_64-linux'). Auto-detected if not specified.
        #[arg(long)]
        flake_output: Option<String>,
        /// Only update source hash, skip dependency hashes (npmDeps, nugetDeps, composerDeps,
        /// etc.)
        #[arg(long)]
        src_only: bool,
    },
    /// Prune maintainers from all .nix files in a directory
    PruneMaintainers {
        /// Directory to process
        directory: String,
        /// Check mode: fail if any changes would be made
        #[arg(long, default_value = "false")]
        check: bool,
    },
    /// Show update failure logs for a package
    Log {
        /// Drv path (e.g., /nix/store/...drv or hash-name.drv) or attr path (e.g.,
        /// python.pkgs.setuptools)
        identifier: String,
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = "~/.cache/ekapkgs-update/updates.db")]
        database: String,
    },
    /// Migrate a package from nixpkgs to ekapkgs paradigms
    Migrate {
        /// Nix file to evaluate (for attr paths)
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// Attribute path or file path to migrate
        target: String,
    },
}

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

    let args = Args::parse();

    match args.command {
        Commands::Run {
            file,
            database,
            upstream,
            fork,
            run_passthru_tests,
            dry_run,
            concurrent_updates,
            skip_unstable,
        } => {
            commands::run::run(
                file,
                database,
                upstream,
                fork,
                run_passthru_tests,
                dry_run,
                concurrent_updates,
                skip_unstable,
            )
            .await?
        },
        Commands::Update {
            file,
            attr_path,
            semver,
            ignore_update_script,
            commit,
            create_pr,
            upstream,
            fork,
            run_passthru_tests,
            variant,
            all_variants,
            flake,
            flake_output,
            src_only,
        } => {
            commands::update::update(
                file,
                attr_path,
                semver,
                ignore_update_script,
                commit,
                create_pr,
                upstream,
                fork,
                run_passthru_tests,
                variant,
                all_variants,
                flake,
                flake_output,
                src_only,
            )
            .await?
        },
        Commands::PruneMaintainers { directory, check } => {
            commands::prune_maintainers::prune_maintainers(directory, check).await?
        },
        Commands::Log {
            identifier,
            database,
        } => commands::log::show_log(database, identifier).await?,
        Commands::Migrate { file, target } => commands::migrate::migrate(file, target).await?,
    }

    Ok(())
}
