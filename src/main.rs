use clap::{Parser, Subcommand};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

mod commands;
mod database;
mod git;
mod github;
mod gitlab;
mod nix;
mod package;
mod pypi;
mod rewrite;
mod vcs_sources;

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
    },
    /// Prune maintainers from all .nix files in a directory
    PruneMaintainers {
        /// Directory to process
        directory: String,
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
        } => {
            commands::run::run(
                file,
                database,
                upstream,
                fork,
                run_passthru_tests,
                dry_run,
                concurrent_updates,
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
            )
            .await?
        },
        Commands::PruneMaintainers { directory } => {
            commands::prune_maintainers::prune_maintainers(directory).await?
        },
        Commands::Log {
            identifier,
            database,
        } => commands::log::show_log(database, identifier).await?,
    }

    Ok(())
}
