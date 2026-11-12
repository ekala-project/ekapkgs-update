# ekapkgs-update-web: implementation notes

Notes for working on the dashboard. [README.md](./README.md) has the user-facing
summary of routes, options and the API.

## Principles

- **Read-only.** Handlers only `SELECT`. The one exception is startup:
  `ekapkgs_update::database::Database::new` creates the file if needed,
  applies migrations, and prunes expired CVE/Repology cache rows.
- **Decoupled from the CLI's types.** The web crate uses only
  `Database::{new, from, pool}` from the library. Every read is a raw `sqlx`
  query in `src/queries/` that maps into row structs owned by this crate. A
  CLI refactor therefore breaks the dashboard only if the schema changes, and
  the in-memory tests catch that because they run the real migrations.
- **Pre-formatted views.** Templates do not compute anything. `src/views/`
  and the route modules turn rows into view models whose fields are display
  strings: relative and absolute times, durations, thousands separators,
  badges with a tone, hrefs. Formatting rules live in `src/format.rs`, which
  has table-driven tests.
- **Lenient input.** Query strings are parsed into a `HashMap` and each
  parameter is validated by hand. An unknown `status`, `sort` or `range` falls
  back to the default, and an out-of-range `page` clamps to the last page, so
  a stale bookmark never produces an error.

## Request flow

```
Router (routes/mod.rs)
  ├── pages       routes/{dashboard,packages,sessions,failures,watch,analytics}.rs
  │                 → queries/* → views/* → templates.rs (Askama) → HTML
  ├── fragments   same handlers' data, rendered without base.html
  ├── /api/*      routes/api.rs → queries/* → serde_json (raw values)
  ├── /ws/live    routes/ws.rs  → queries/live.rs fingerprint
  ├── /static/*   assets.rs (embedded)
  └── fallback    error.rs (404 page)
```

`AppError` (`src/error.rs`) has `NotFound` and `Internal`. Pages render
`templates/error.html`; `/api/*` returns `{"error", "status"}`. Internal
errors are logged with `tracing` and shown without the SQL text.

## Data sources

| Page | Tables |
|---|---|
| Dashboard | `updates` (status counts, latest PRs), `update_sessions` (running), `update_phases` (in-flight packages, recent activity), `watch_cycles`, `watched_repos`, `upstream_sources` |
| Packages | `updates` |
| Package detail | `updates`, `upstream_index`, `upstream_sources`, `update_phases`, `update_logs`, `autofix_queue`, `autofix_attempts` |
| Sessions / detail | `update_sessions`, `update_phases`, `update_logs`, `watch_cycles` (the cycle that created a watch session) |
| Failures | `update_logs` |
| Watch | `watched_repos`, `watch_cycles`, `upstream_sources`, `upstream_index`, `updates` (backlog per repository) |
| Analytics | `update_sessions`, `update_phases`, `updates`, `autofix_queue`, `autofix_attempts` |

Conventions that come from the schema:

- `updates`, `update_phases`, `update_logs` and `upstream_index` are keyed by
  `repo_name`. `''` is the repository used by `run` and single-repo watch. It
  is displayed as `default`, and `?repo=default` selects it.
- **Backlog** means `status = 'outdated'` with `latest_upstream_version` equal
  to `baseline_version`. Watch observed that release at bootstrap and leaves
  it to `run`.
- Finished watch sessions with `packages_attempted = 0` are legacy rows from
  before sessions were created lazily. They are hidden from lists and counts
  unless `show_empty=1`.
- A session's success rate is `succeeded / (succeeded + failed)`. Skipped
  packages were not attempted, so they are excluded.

## Live updates

Each `/ws/live` connection runs its own loop:

1. Every 2 s it computes `queries::live::fingerprint`, a single cheap query
   over indexed maxima and counts: max phase id, running phases, latest cycle
   id and status, running-session counters, max session `updated_at` and
   `started_at`, and max `watched_repos.updated_at`.
2. It sends `{"type":"live","v":"%016x"}` (an FNV-1a hash of the
   fingerprint) on connect and whenever the value changes.
3. It pings after 30 s without sending anything, and exits when the client
   goes away.

`static/js/live.js` remembers the last value across reconnects and dispatches
`live-update` on `<body>` only when it changes. HTMX regions use
`hx-trigger="live-update from:body, every 60s"`. A running session's detail
fragment polls every 30 s as well. When the session finishes, the swapped-in
fragment no longer carries `hx-get`, so polling stops.

The socket only says that something changed and carries no data.
Fragments are ordinary GET handlers, so every live panel also works with
JavaScript off (the content is just as fresh as the page load).

## Charts

Charts are plain HTML and CSS. There is no chart library and no canvas.

- **Segmented bars** (`segbar` macro) for status mixes. Segments use
  `flex-grow`, a 2px surface gap and rounded ends.
- **Horizontal bars** (`hbars` macro) for ranked categories such as error
  types, upstream types and autofix outcomes. The list is folded to an
  "Other" row after the top entries.
- **Column chart** on `/analytics` for daily outcomes. Columns are stacked
  succeeded/failed, gaps are filled with empty days, and the axis uses nice
  ticks (`views/charts.rs`).
- **Meters** for phase failure rates.

Every chart has a legend or a title naming the series, a `data-tip` tooltip
that also shows on keyboard focus, and a `<details>` table view. Colors are
CSS custom properties defined once for light mode and once for dark mode.
Dark mode applies under `prefers-color-scheme` unless
`data-theme="light"`, or when `data-theme="dark"` is set. Status colors are
reserved for status: good, warn, serious and critical. The single data
series uses blue. `forced-colors` and print rules keep marks visible.

## Templates

- `base.html` holds the layout, nav (`aria-current="page"`) and live
  indicator. `macros.html` holds shared helpers. `fragments/` holds the
  partials that HTMX swaps in. Pages include fragments so the first render
  and later refreshes share markup.
- Askama 0.12 quirks:
  - Macro arguments are bound by name, so a parameter that shares its name
    with a template field shadows that field. Every macro parameter
    therefore has a unique name.
  - `{% for x in a.b %}` inside a macro moves the value, so macros always
    loop over `.iter()`.
- Askama escapes all output by default. The tests check this against a
  description containing markup.

## Static assets

`assets.rs` embeds `css/styles.css`, `js/htmx.min.js` and `js/live.js` with
`include_str!`. Responses carry:

- an `ETag` (FNV-1a of the content), with `If-None-Match` answered by 304;
- `Cache-Control: public, max-age=86400`.

Pages reference assets as `?v=<hash of all assets>`, so a new build busts
the cache. Nothing is installed next to the binary, and `nix/package-web.nix`
has no `postInstall`.

## Testing

`src/routes/tests.rs` builds the router on `sqlite::memory:` with
`sqlx::migrate!("../ekapkgs-update/migrations")` and drives it with
`tower::ServiceExt::oneshot`:

- A fixture covers every table the dashboard reads. Timestamps are written as
  `AGO(minutes)` and expanded relative to now, so relative times are stable.
- One test requests every page, fragment and API endpoint against the
  fixture, and another does the same against an empty database (empty
  states).
- Further tests cover:
  - filters, sorting and pagination (including clamping and bad input);
  - 404s (HTML and JSON) and HTML escaping;
  - asset ETag/304 and the cache-busting query;
  - the live fingerprint changing after a write, and `/ws/live` rejecting
    a non-upgrade request.

Formatting, chart scaling and query helpers have unit tests next to their
code.

## Known limitations

- There is no authentication. Bind to localhost or put the portal behind a
  proxy that provides auth.
- `autofix_queue` has no `repo_name`, so the autofix section on a package
  page matches by attribute path across all repositories.
- `/failures` lists `update_logs`. The error-type breakdown on `/analytics`
  counts failed `update_phases`. The two can differ when a failure was
  recorded in only one of them.
- A repository actually named `default` would be indistinguishable from the
  `''` repository in URLs.
- The portal and the CLI must be built from the same migration set: an
  older portal refuses to start against a database migrated by a newer CLI.
