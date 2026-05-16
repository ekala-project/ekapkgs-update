use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use axum::Router;
use axum::routing::get;
use clap::Parser;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::info;

mod routes;
mod state;
mod templates;

use state::AppState;

#[derive(Parser, Debug)]
#[command(name = "ekapkgs-update-web")]
#[command(about = "Web portal for monitoring ekapkgs-update package updates", long_about = None)]
struct Args {
    /// Path to SQLite database
    #[arg(short, long, default_value = "~/.local/share/ekapkgs-update/db.sqlite")]
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
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ekapkgs_update_web=info,tower_http=debug".into()),
        )
        .init();

    let args = Args::parse();

    // Expand shell variables in database path
    let db_path = shellexpand::tilde(&args.database).to_string();
    let db_path_buf = PathBuf::from(&db_path);

    // Initialize database connection (creates database and runs migrations if needed)
    let db_exists = db_path_buf.exists();
    if !db_exists {
        info!(
            "Database not found at {}, creating new database...",
            db_path
        );
    } else {
        info!("Using database at: {}", db_path);
    }

    let db = ekapkgs_update::database::Database::new(&db_path)
        .await
        .context("Failed to connect to database")?;

    if !db_exists {
        info!("✓ Database created successfully");
        info!("📝 Note: Database is empty. Run 'ekapkgs-update run' to populate with update data.");
    } else {
        info!("✓ Database connection established");
    }

    // Create shared application state
    let state = AppState::new(db);

    // Build the router
    let mut app = Router::new()
        .route("/", get(routes::dashboard::index))
        .route("/sessions", get(routes::sessions::list))
        .route("/sessions/:id", get(routes::sessions::detail))
        .route("/packages", get(routes::packages::list))
        .route("/packages/:attr", get(routes::packages::detail))
        .route("/analytics", get(routes::analytics::index))
        // API endpoints for HTMX
        .route("/api/stats", get(routes::dashboard::stats_json))
        .route("/api/sessions", get(routes::sessions::list_json))
        .route("/api/sessions/:id/phases", get(routes::sessions::phases_json))
        // WebSocket for real-time updates
        .route("/ws/live", get(routes::ws::websocket_handler))
        // Serve static files
        .nest_service("/static", ServeDir::new("ekapkgs-update-web/static"))
        .with_state(state)
        .layer(TraceLayer::new_for_http());

    // Add CORS if requested
    if args.cors {
        info!("CORS enabled for public access");
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);
        app = app.layer(cors);
    }

    // Bind and serve
    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .context("Invalid host or port")?;

    info!("Starting server on http://{}", addr);
    info!("Press Ctrl+C to stop");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("Failed to bind to address")?;

    axum::serve(listener, app).await.context("Server error")?;

    Ok(())
}
