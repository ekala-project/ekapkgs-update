//! Read-only web dashboard for the ekapkgs-update SQLite database.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

mod assets;
mod error;
mod params;
mod queries;
mod routes;
mod state;

use state::AppState;

#[derive(Parser, Debug)]
#[command(name = "ekapkgs-update-web")]
#[command(about = "Web portal for monitoring ekapkgs-update package updates", long_about = None)]
struct Args {
    /// Path to SQLite database (the same default as the `ekapkgs-update` CLI)
    #[arg(short, long, default_value = ekapkgs_update::paths::DEFAULT_DATABASE_PATH)]
    database: String,

    /// Port to listen on
    #[arg(short, long, default_value = "3000")]
    port: u16,

    /// Host to bind to
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Enable CORS for public access
    #[arg(long)]
    cors: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ekapkgs_update_web=info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();

    let db_path = shellexpand::tilde(&args.database).to_string();
    if PathBuf::from(&db_path).exists() {
        info!("Using database at {db_path}");
    } else {
        info!("Database not found at {db_path}; creating an empty one");
    }

    // `Database::new` creates the file if needed and applies migrations, so
    // the dashboard can start before the CLI has ever run.
    let db = ekapkgs_update::database::Database::new(&db_path)
        .await
        .context("Failed to connect to database")?;

    let mut app = routes::router(AppState::new(db)).layer(TraceLayer::new_for_http());

    if args.cors {
        info!("CORS enabled for public access");
        app = app.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    }

    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .context("Invalid host or port")?;
    info!("Listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("Failed to bind to address")?;
    axum::serve(listener, app).await.context("Server error")?;

    Ok(())
}
