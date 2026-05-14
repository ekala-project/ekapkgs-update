# ekapkgs-update-web

Web portal for monitoring ekapkgs-update package updates in real-time.

## Overview

A standalone web service that provides visibility into the ekapkgs-update package update process with:
- Real-time session monitoring with WebSocket updates
- Historical analytics and charts
- Package status tracking
- Error analysis and debugging

## Architecture

- **Backend**: Rust + Axum web framework
- **Templates**: HTMX + server-side rendering with Askama
- **Database**: Shared read-only access to ekapkgs-update SQLite database
- **Real-time**: WebSocket for live session updates

## Project Structure

```
ekapkgs-update-web/
├── Cargo.toml
├── src/
│   ├── main.rs              # Server setup and CLI
│   ├── state.rs             # Shared application state
│   ├── templates.rs         # Askama template definitions
│   └── routes/              # Route handlers
│       ├── mod.rs
│       ├── dashboard.rs     # Dashboard with live stats
│       ├── sessions.rs      # Session list and details
│       ├── packages.rs      # Package listing
│       ├── analytics.rs     # Charts and analytics
│       └── ws.rs            # WebSocket real-time updates
├── templates/               # HTML templates (Askama)
│   ├── base.html
│   ├── dashboard.html
│   ├── sessions.html
│   ├── session_detail.html
│   ├── packages.html
│   └── analytics.html
└── static/                  # Static assets
    ├── css/styles.css
    └── js/htmx.min.js
```

## Features

### Dashboard (`/`)
- Live statistics: total packages, success rate, active updates
- Current session progress with real-time updates
- Recent activity timeline
- Auto-refresh via HTMX

### Sessions (`/sessions`)
- List all update sessions
- Filter by status (running/completed/failed/cancelled)
- Session details with phase-by-phase breakdown
- Success/failure analysis

### Packages (`/packages`)
- Searchable package list
- Current vs latest version tracking
- Update status indicators
- PR links

### Analytics (`/analytics`)
- Error distribution charts
- Phase statistics (success/failure rates, durations)
- Success rate trends over time

## API Endpoints

- `GET /` - Dashboard (SSR)
- `GET /sessions` - Sessions list (SSR)
- `GET /sessions/:id` - Session detail (SSR)
- `GET /packages` - Package list (SSR)
- `GET /analytics` - Analytics (SSR)
- `GET /api/stats` - Live stats (JSON)
- `GET /api/sessions` - Sessions (JSON)
- `GET /api/sessions/:id/phases` - Session phases (JSON)
- `WebSocket /ws/live` - Real-time updates

## Usage

### Development

```bash
# From workspace root
cargo run -p ekapkgs-update-web -- \\
  --database ~/.local/share/ekapkgs-update/db.sqlite \\
  --port 3000
```

### Production

```bash
cargo build --release -p ekapkgs-update-web

./target/release/ekapkgs-update-web \\
  --database /path/to/db.sqlite \\
  --host 0.0.0.0 \\
  --port 3000 \\
  --cors
```

### CLI Options

- `--database, -d`: Path to SQLite database (default: `~/.local/share/ekapkgs-update/db.sqlite`)
- `--port, -p`: Port to listen on (default: `3000`)
- `--host`: Host to bind to (default: `127.0.0.1`)
- `--cors`: Enable CORS for public access

## Implementation Status

### ✅ Completed
- [x] Cargo workspace setup
- [x] Web server with Axum
- [x] Database integration (shared with ekapkgs-update)
- [x] All route handlers
- [x] WebSocket support for real-time updates
- [x] Askama templates for all pages
- [x] CSS styling (minimal, clean design)
- [x] HTMX integration for dynamic updates
- [x] Serde support for UpdateSession, PhaseRecord, SessionStatus

### ⚠️ In Progress
- [ ] Fix remaining template syntax issues:
  - Replace `|round` filter (use `|int` or format in Rust)
  - Fix Option handling in templates (use `.is_some()` or `{% if let Some(...) %}`)
  - Remove `.format()` calls on DateTimes (already using Display)

### 🔮 Future Enhancements
- [ ] Custom Askama filters for date formatting
- [ ] Authentication for interactive features
- [ ] Retry/skip buttons (requires mutations + auth)
- [ ] Advanced filtering and search
- [ ] Email/webhook notifications
- [ ] Performance caching
- [ ] Docker deployment
- [ ] Systemd service file

## Database Schema

The web portal reads from the following tables in the ekapkgs-update database:

- `updates`: Package tracking (versions, PRs, rebuild counts)
- `update_sessions`: Session lifecycle and statistics
- `update_phases`: Phase-by-phase execution records
- `update_logs`: Error logs and failure details
- `cve_cache`, `repology_cache`: External API caches

## Design Decisions

1. **Read-only access**: Web portal never modifies the database, ensuring safety
2. **Standalone service**: Separate binary for flexible deployment
3. **SSR + HTMX**: Simple, fast, minimal JavaScript
4. **Real-time via WebSocket**: Live updates without polling overhead
5. **Shared workspace**: Type reuse from main ekapkgs-update crate

## Security

- Read-only database access (SELECT only)
- No authentication in MVP (safe for read-only public deployment)
- CORS optional and configurable
- Input validation on all queries
- SQL injection protection via prepared statements

## Performance

- Minimal resource usage (Rust async)
- Efficient database queries (indexed lookups)
- Static file serving via tower-http
- WebSocket for efficient real-time updates
- No ORM overhead (direct SQLx queries)

## Contributing

See parent repository for contribution guidelines.

## License

Same as parent project.
