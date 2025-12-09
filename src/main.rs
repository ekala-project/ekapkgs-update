use clap::{Parser, Subcommand};

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
        /// File or directory to process
        #[arg(short, long)]
        file: Option<String>,
    },
}

fn main() {
    let args = Args::parse();

    match args.command {
        Commands::Run { file } => {
            if let Some(file) = file {
                println!("Running update with file: {}", file);
            } else {
                println!("Running update with no file specified");
            }
        }
    }
}
