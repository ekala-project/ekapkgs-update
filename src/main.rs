use clap::{Parser, Subcommand};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

mod commands;
mod nix;

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
    }

    Ok(())
}
