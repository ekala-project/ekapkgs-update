use clap::{Parser, Subcommand};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

mod commands;
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
        #[arg(short, long)]
        file: String,
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
    },
    /// Prune maintainers from all .nix files in a directory
    PruneMaintainers {
        /// Directory to process
        directory: String,
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
        Commands::Run { file } => commands::run::run(file).await?,
        Commands::Update {
            file,
            attr_path,
            semver,
        } => commands::update::update(file, attr_path, semver).await?,
        Commands::PruneMaintainers { directory } => {
            commands::prune_maintainers::prune_maintainers(directory).await?
        },
    }

    Ok(())
}
