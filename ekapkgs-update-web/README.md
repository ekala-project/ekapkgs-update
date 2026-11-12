# ekapkgs-update-web

A read-only web dashboard for the SQLite database that `ekapkgs-update run`
and `ekapkgs-update watch` write to. It shows what is running, what each
package's update state is, why things failed, how watch mode is doing, and
trends over time. Pages update live while the CLI works.

- **Backend:** Rust, Axum 0.7, raw `sqlx` queries
- **Templates:** Askama (compiled into the binary), HTMX for partial refresh
- **Live updates:** one WebSocket per tab that signals "something changed";
  panels re-fetch their own HTML fragment
- **Assets:** CSS and JS embedded with `include_str!`, so the binary is
  self-contained

The dashboard never writes update state. On startup, `Database::new` creates
the database file if it is missing and applies the CLI's migrations, so the
portal can start before the CLI has ever run.

## Running

```bash
# From the workspace root, inside `nix develop`
cargo run -p ekapkgs-update-web -- --database ~/.cache/ekapkgs-update/updates.db

# Or
./ekapkgs-update-web/start-web.sh     # honours EKAPKGS_DATABASE, EKAPKGS_WEB_PORT,
                                      # EKAPKGS_WEB_HOST, EKAPKGS_WEB_CORS
```

| Option | Default | |
|---|---|---|
| `-d, --database` | `~/.cache/ekapkgs-update/updates.db` | Same default as the CLI |
| `-p, --port` | `3000` | |
| `--host` | `127.0.0.1` | |
| `--cors` | off | Allow any origin (for API consumers on other domains) |

Logging is controlled by `RUST_LOG` (default
`ekapkgs_update_web=info,tower_http=info`).

## Pages

Every list is filterable through query parameters, so any view can be
bookmarked or linked. The repository `''` used by `run` and single-repo
watch is shown, and selected, as `default`.

| Route | What it shows |
|---|---|
| `/` | Package status tiles (tracked, up to date, outdated, proposed, failed, upstream errors) plus the 30-day success rate; running sessions with their in-flight packages; recent update activity; latest PRs; a watch summary |
| `/packages` | Every tracked attribute. Filters `q`, `repo`, `status` (including `backlog`), `upstream`; `sort` = `attr`, `status`, `checked`, `rebuilds`, `failures`; `page` |
| `/packages/:attr?repo=` | One attribute in one repository: update state and backoff, PR, upstream source and its health, upstream index entries, phase history (one row per session), failure logs, autofix queue and attempts. Links to the same attribute in other repositories |
| `/sessions` | Run and watch sessions. Filters `status`, `mode` (`run` or `watch`); finished watch sessions that attempted nothing are hidden unless `show_empty=1` |
| `/sessions/:id` | Session summary and counters, the watch cycle that started it, per-package outcome with phase chips (filter `outcome`), structured phase errors, failure logs, and the session's configuration |
| `/failures` | Failure logs (`update_logs`). Filters `q`, `repo`, `phase`, `error_type`; the last 6,000 characters of each log are shown |
| `/watch` | Watched repositories (sync and index state, HEAD), poll and index cycles (filter `kind`), source health by type, failing sources, recently changed releases |
| `/analytics` | For `range` = `7d`, `30d` (default), `90d`, `all`: daily outcomes, phase failure rates and durations, error types, most failing packages, status by repository, upstream types, rebuild impact, autofix outcomes |

Unknown pages, sessions and packages return a styled 404; database errors
return a 500 page (JSON for `/api/*`).

## Live updates

`/ws/live` re-reads a cheap fingerprint of the database every 2 seconds per
connection (latest phase id, running phases, latest watch cycle, running
session counters, latest session, repository updates) and sends
`{"type":"live","v":"<hash>"}` when it changes. It pings after 30 seconds of
silence to keep proxies from closing the connection.

`static/js/live.js` turns a changed value into a `live-update` event on
`<body>`. Panels declare `hx-trigger="live-update from:body, every 60s"` and
re-fetch their fragment. The 60-second timer is the fallback when the socket
is down. The header indicator shows `connecting`, `live` or `offline`, and
the client reconnects with exponential backoff (1 to 30 seconds).

Fragments (HTML without the layout):

- `/fragments/stats`, `/fragments/active`, `/fragments/watch`, `/fragments/activity`
- `/fragments/sessions` (accepts the `/sessions` filters)
- `/fragments/sessions/:id` (stops polling once the session finishes)

## JSON API

Raw database values (RFC 3339 timestamps, unformatted numbers). List
endpoints take `limit` (default 50, max 500) and `page`.

| Endpoint | Returns |
|---|---|
| `GET /api/stats` | `total_packages`, `success_rate`, `active_updates`, `total_sessions`, `running_sessions`, `packages` (status counts), `outcomes_30d`, `upstream_sources` |
| `GET /api/sessions` | `{sessions, total, page, per_page}`. Accepts the `/sessions` filters |
| `GET /api/sessions/:id/phases` | The session's phase rows, in execution order |
| `GET /api/packages` | `{packages, total, page, per_page, sort, status_counts}`. Accepts the `/packages` filters |
| `GET /api/packages/:attr?repo=` | `update`, `repos`, `index`, `sources`, `phases`, `logs`, `autofix.{queue, attempts}` |
| `GET /api/watch` | `repos`, `cycles`, `cycle_total`, `source_types`, `failing_sources`, `recent_releases` |

`/api/sessions` used to return a bare array. It now returns an object so
that it can carry the total and page.

## Layout

```
ekapkgs-update-web/
├── src/
│   ├── main.rs          # CLI options, database, router, CORS/trace layers
│   ├── state.rs         # AppState (wraps Database)
│   ├── error.rs         # AppError: HTML or JSON 404/500, fallback handler
│   ├── assets.rs        # Embedded static files, ETag + Cache-Control
│   ├── format.rs        # Relative times, durations, numbers, badges, pager, URLs
│   ├── queries/         # Raw sqlx reads, one module per area (+ live fingerprint)
│   ├── views/           # Row → display view models; charts.rs builds bar/column data
│   ├── templates.rs     # Askama template structs
│   └── routes/          # Page, fragment, API and WebSocket handlers; tests.rs
├── templates/           # base, pages, macros.html, error.html, fragments/
└── static/              # css/styles.css, js/htmx.min.js, js/live.js
```

## Development

```bash
nix develop --command cargo test -p ekapkgs-update-web
nix develop --command cargo clippy -p ekapkgs-update-web --all-targets
```

`src/routes/tests.rs` runs every page, fragment and API endpoint against an
in-memory database with the real migrations. It does this once with a
fixture that covers every table and once with an empty database. It also
checks 404s, HTML escaping, pagination, asset caching and the live
fingerprint.

Templates and assets are compiled in. After editing them, rebuild; there
is nothing to install next to the binary.

See [WEB-SERVER.md](./WEB-SERVER.md) for implementation notes and
[NIXOS-MODULE.md](./NIXOS-MODULE.md) for deployment.
