# ekapkgs-update-web

A read-only web dashboard for the SQLite database that `ekapkgs-update run`
and `ekapkgs-update watch` write to. It shows what is running, what each
package's update state is, why things failed, how watch mode is doing, and
trends over time. Pages update live while the CLI works.

- **Backend:** Rust, Axum 0.7, raw `sqlx` queries
- **Frontend:** an Elm 0.19 single-page application (`frontend/`) over the
  JSON API. `build.rs` compiles it, and the result is embedded in the binary
- **Live updates:** one WebSocket per tab that signals "something changed";
  the app re-fetches the current page's JSON
- **Assets:** CSS, the compiled `elm.js` and `app.js` embedded with
  `include_str!`, so the binary is self-contained. The Nix build minifies
  `elm.js` with uglify-js, and every response (assets and JSON) is
  gzip-compressed when the client accepts it

The dashboard never writes update state while it runs. On startup,
`Database::new` creates the database file if it is missing and applies the
CLI's migrations, so the portal can start before the CLI has ever run. It
then repairs state the SQL migrations cannot derive in databases upgraded
from before the dashboard data model: proposals the tree has already passed
are marked merged, and backfilled failures get their error summary. On a
database the current CLI maintains, this changes nothing.

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

Every page route returns the same HTML shell, and the Elm app routes on the
client from the path. Unknown paths return the shell with status 404, and the
app shows its "not found" page. Unknown sessions and packages are 404s from
the API, which the app shows the same way. Database errors are JSON 500s.

## Live updates

`/ws/live` re-reads a cheap fingerprint of the database every 2 seconds per
connection (latest phase id, running phases, latest watch cycle, running
session counters, latest session, repository updates) and sends
`{"type":"live","v":"<hash>"}` when it changes. It pings after 30 seconds of
silence to keep proxies from closing the connection.

`static/js/app.js` owns the socket. It sends a changed value to the Elm
`liveUpdate` port, and the app re-fetches the JSON for the page on screen.
A 60-second timer in the app is the fallback when the socket is down. The
header indicator (fed through the `liveStatus` port) shows `connecting`,
`live` or `offline`, and the client reconnects with exponential backoff
(1 to 30 seconds).

## JSON API

Raw database values (RFC 3339 timestamps, unformatted numbers). List
endpoints take `limit` (default 50, max 500) and `page`.

| Endpoint | Returns |
|---|---|
| `GET /api/stats` | `total_packages`, `success_rate`, `active_updates`, `total_sessions`, `running_sessions`, `packages` (status counts), `outcomes_30d`, `upstream_sources` |
| `GET /api/dashboard` | `counts`, `current` / `previous` (30-day outcomes), `running`, `running_phases`, `watch.{repos, last_poll, source_types}`, `activity`, `recent_prs` |
| `GET /api/packages` | `{packages, total, all_total, page, per_page, sort, sorts, status_counts, repos, upstream_types}`. Accepts the `/packages` filters |
| `GET /api/packages/:attr?repo=` | `attr_path`, `repo_name`, `update`, `repos`, `index`, `sources`, `phases`, `logs`, `autofix.{queue, attempts}` |
| `GET /api/sessions` | `{sessions, total, page, per_page, hidden_empty, facets}`. Accepts the `/sessions` filters |
| `GET /api/sessions/:id` | `session` (including `config_json`), `phases`, `logs`, `cycle` |
| `GET /api/sessions/:id/phases` | The session's phase rows, in execution order |
| `GET /api/failures` | `{logs, total, page, per_page, facets.{repos, phases, error_types}}`. Accepts the `/failures` filters |
| `GET /api/watch` | `repos`, `kind`, `cycles`, `cycle_total`, `cycle_counts`, `page`, `per_page`, `last_poll`, `source_types`, `failing_sources`, `recent_releases` |
| `GET /api/analytics?range=` | Outcomes, daily trend, error types, phase stats and percentiles, status by repository, upstream types, rebuild buckets, autofix outcomes, top failing packages |

Unknown `/api` paths, sessions and packages return
`404 {"error", "status"}`.

## Layout

```
ekapkgs-update-web/
├── build.rs             # Compiles frontend/ with `elm make --optimize` into OUT_DIR
├── frontend/            # Elm application
│   ├── elm.json
│   └── src/
│       ├── Main.elm     # Browser.application, fetch/refresh, layout, ports
│       ├── Route.elm    # Path → Route, Route → /api URL
│       ├── Api.elm      # Shared JSON decoders and HTTP error handling
│       ├── Format.elm   # Times, durations, numbers, badges, URLs
│       ├── View.elm     # Shared view helpers (chips, pager, bars, filters)
│       ├── Ctx.elm, Msg.elm
│       └── Page/        # One module per page: Data, decoder, view
├── src/
│   ├── main.rs          # CLI options, database, router, CORS/trace layers
│   ├── state.rs         # AppState (wraps Database)
│   ├── error.rs         # AppError → JSON 404/500
│   ├── params.rs        # Query-parameter helpers (repo labels, pages, ratios)
│   ├── assets.rs        # App shell, embedded static files, weak ETag + Cache-Control
│   ├── queries/         # Raw sqlx reads, one module per area (+ live fingerprint)
│   └── routes/          # Page routes (shell), JSON API, WebSocket, compression; tests.rs
└── static/              # css/styles.css, js/app.js (elm.js comes from build.rs)
```

## Development

```bash
nix develop --command cargo test -p ekapkgs-update-web
nix develop --command cargo clippy -p ekapkgs-update-web --all-targets
```

`build.rs` runs `elm make --optimize` whenever a file under `frontend/`
changes. `elm` comes from the dev shell on x86_64-linux and
aarch64-linux (the aarch64 binary is the official elm release, added in
`nix/overlay.nix` because ekapkgs only pins the x86_64 one). An Elm compile
error fails the build. If `elm` is not on `PATH`, the build still succeeds:
it prints a `cargo:warning` and embeds a stub `elm.js` that only shows a
"frontend not built" notice linking to `/api/dashboard`. The API and the
tests work as usual. Set `EKAPKGS_WEB_REQUIRE_ELM=1` to make a missing
`elm` an error. The build re-runs when `PATH` changes, so building again
inside `nix develop` picks up the real frontend.

To skip the compile, set `EKAPKGS_WEB_ELM_JS` to an already-compiled
`elm.js`. A missing file there is an error, never a stub. The Nix package
does this, building the frontend
in a separate derivation (`nix/package-web-frontend.nix`) with dependencies
pinned in `nix/web-frontend/`. After changing `frontend/elm.json`, run
`nix/web-frontend/update.sh` to refresh those pins.

For a quick type check of the frontend alone:

```bash
cd ekapkgs-update-web/frontend && elm make src/Main.elm --output=/dev/null
```

The Elm sources are formatted with `elm-format`, which `nix fmt .` runs over
every `*.elm` file (along with rustfmt and nixfmt). ekapkgs does not package
it, so `nix/elm-format.nix` builds it from source; the dev shell provides it
too, for editor integration or `elm-format --yes src/`.

The frontend's unit tests live in `frontend/tests/`. The runner needs elm,
jq and QuickJS (`qjs`), all in the dev shell on x86_64-linux:

```bash
nix develop --command ekapkgs-update-web/frontend/test-runner/run.sh
nix build .#checks.x86_64-linux.web-frontend-tests   # same, in the sandbox
```

Pass a seed (`run.sh 42`) to vary fuzz tests. A new test module must be
named `*Test.elm` and expose `suite : Test`. The runner picks it up
automatically.

`src/routes/tests.rs` drives every API endpoint against an in-memory
database with the real migrations. It does this once with a fixture that
covers every table and once with an empty database. It also checks filters,
pagination, JSON 404s, the app shell on page and unknown routes, asset
caching and the live fingerprint.

Assets are compiled in, so rebuild after editing them. There is nothing
to install next to the binary.

See [WEB-SERVER.md](./WEB-SERVER.md) for implementation notes and
[NIXOS-MODULE.md](./NIXOS-MODULE.md) for deployment.
