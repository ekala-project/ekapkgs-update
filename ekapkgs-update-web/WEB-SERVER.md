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
- **Raw API, formatting in the client.** The server returns raw values
  (RFC 3339 timestamps, integer milliseconds, unformatted counts). The Elm
  frontend does all display work: relative and absolute times, durations,
  thousands separators, badges and tones, hrefs (`frontend/src/Format.elm`).
  One page is one `GET /api/…` request, so every response carries
  everything its page shows, including filter facets and counts.
- **Lenient input.** Query strings are parsed into a `HashMap` and each
  parameter is validated by hand. An unknown `status`, `sort`, `kind` or
  `range` falls back to the default, and an out-of-range `page` is clamped
  to the last page (`params::clamp_page`; the response's `page` is the
  effective one, which the frontend displays), so a stale bookmark never
  produces an error or an empty page.

## Request flow

```
Router (routes/mod.rs)
  ├── page paths  /, /packages[/:attr], /sessions[/:id], /failures, /watch,
  │               /analytics → assets::index_response (the app shell, 200)
  ├── /api/*      routes/api.rs → queries/* → serde_json (raw values);
  │               its own fallback is a JSON 404
  ├── /ws/live    routes/ws.rs  → queries/live.rs fingerprint
  ├── /static/*   assets.rs (embedded css, elm.js, app.js)
  └── fallback    the app shell with status 404

Browser
  index.html → elm.js (Browser.application) + app.js (ports, WebSocket)
  Main.elm: URL → Route → GET /api/… → Page.X.decoder → Page.X.view
```

`AppError` (`src/error.rs`) has `NotFound` and `Internal`, and always
responds with `{"error", "status"}`. Internal errors are logged with
`tracing` and returned without the SQL text. The frontend shows a 404 or
500 page using the `error` message. When a refresh fails after data has
loaded, it keeps the last data and shows a notice instead.

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

`static/js/app.js` remembers the last value across reconnects and sends it
to the Elm `liveUpdate` port only when it changes. Main then re-fetches the
current page's JSON. Responses are numbered, and only the latest one is
applied, so a slow response cannot overwrite a newer one. Main also
re-fetches every 60 s as a fallback, and ticks the clock every 15 s so that
relative times stay current. Connection state reaches the header indicator
through the `liveStatus` port.

The socket only says that something changed and carries no data. The
dashboard needs JavaScript. The shell's `<noscript>` points at `/api`.

## Charts

Charts are plain HTML and CSS. There is no chart library and no canvas.

- **Segmented bars** (`View.segbar`) for status mixes. Segments use
  `flex-grow`, a 2px surface gap and rounded ends.
- **Horizontal bars** (`View.hbars`) for ranked categories such as error
  types, upstream types and autofix outcomes. The list is folded to an
  "Other" row after the top entries.
- **Column chart** on `/analytics` for daily outcomes. Columns are stacked
  succeeded/failed, gaps are filled with empty days, and the axis uses nice
  ticks (`Page/Analytics.elm`).
- **Meters** for phase failure rates.

Every chart has a legend or a title naming the series, a `data-tip` tooltip
that also shows on keyboard focus, and a `<details>` table view. Colors are
CSS custom properties defined once for light mode and once for dark mode.
Dark mode applies under `prefers-color-scheme` unless
`data-theme="light"`, or when `data-theme="dark"` is set. Status colors are
reserved for status: good, warn, serious and critical. The single data
series uses blue. `forced-colors` and print rules keep marks visible.

## Frontend

- `frontend/` is an Elm 0.19 application (`elm.json`). `Main.elm` owns the
  model, the fetch/refresh cycle, layout and the error pages. Each page
  module under `Page/` exposes `Data`, `decoder` and
  `view : Ctx -> Data -> Ctx.Page` and decodes its own endpoint. Shared
  decoders live in `Api.elm` and shared view helpers in `View.elm`.
- Routing uses path URLs (`Route.elm`). `Route.apiUrl` maps a page and its
  query string to the endpoint, passing the query through unchanged, so the
  page's URL and the API share one set of filter parameters.
- Filter forms keep unsent input as drafts in the model (`Msg.Draft`).
  Submitting pushes a new URL (`Msg.Submit`) and resets `page`.
- Elm renders database text as text nodes, so descriptions and logs cannot
  inject markup. URLs from the database go through `Format.safeUrl`.
- `build.rs` runs `elm make src/Main.elm --optimize` into `OUT_DIR`, and
  `assets.rs` embeds the result. If `EKAPKGS_WEB_ELM_JS` is set, that file
  is used instead, and a missing file is a hard error. The Nix package sets it
  to the output of
  `nix/package-web-frontend.nix`, which builds offline from the pins in
  `nix/web-frontend/` (`elm-srcs.nix`, `registry.dat`). Regenerate them with
  `nix/web-frontend/update.sh` after changing `elm.json`.
- Without `elm` on `PATH` (and no `EKAPKGS_WEB_ELM_JS`), `build.rs` warns
  and embeds a stub `elm.js`. Its `Elm.Main.init` renders a "frontend not
  built" notice linking to `/api/dashboard` and returns an app without
  ports, which `app.js` tolerates. That keeps `cargo build --workspace` and
  `cargo test --workspace` working without `elm`. An Elm compile error is
  still a hard error, and `EKAPKGS_WEB_REQUIRE_ELM=1` makes a missing `elm`
  one too. The build re-runs when `PATH` changes. The source is exposed to
  the crate as `EKAPKGS_WEB_FRONTEND` (`prebuilt`, `elm` or `stub`), and
  the asset test checks the stub when it is embedded.

## Static assets

`assets.rs` embeds `css/styles.css`, `js/app.js` and the compiled
`js/elm.js` with `include_str!`. Responses carry:

- a weak `ETag` (`W/"<FNV-1a of the content>"`), with `If-None-Match`
  answered by 304 (weak comparison, so the strong form matches too);
- `Cache-Control: public, max-age=86400`.

Sizes on the wire are kept down in two places:

- **Minification.** `nix/package-web-frontend.nix` runs `elm make --optimize`
  and then uglify-js with the recipe from Elm's minification guide (a
  compress pass with `pure_funcs` for `F2`..`F9`/`A2`..`A9`, `pure_getters`,
  `keep_fargs=false`, `unsafe_comps`, `unsafe`, then a mangle pass). That
  takes `elm.js` from about 528 KB to 120 KB. Local `cargo` builds use the
  unminified output of `build.rs`.
- **Compression.** `routes::router` ends with tower-http's gzip
  `CompressionLayer`, so the tests see it too. It picks an
  encoding from `Accept-Encoding`, adds `Vary: Accept-Encoding`, and covers
  the JSON API, the shell and the assets alike: the minified `elm.js` is
  about 38 KB gzipped. Bodies under 32 bytes are sent as is, which leaves
  304s and the `/ws/live` upgrade untouched. Because one asset can go out
  in several encodings, its `ETag` is weak. Brotli is not enabled: at
  tower-http's default quality it came out larger than gzip for `elm.js`
  (38.8 KB vs 38.2 KB), and browsers prefer it whenever they offer it.

The shell (`index.html`, served for every page path) references assets as
`?v=<hash of all assets>`, so a new build busts the cache. The shell itself
is sent with `Cache-Control: no-cache`. Nothing is installed next to the binary, and `nix/package-web.nix`
has no `postInstall`.

## Testing

`src/routes/tests.rs` builds the router on `sqlite::memory:` with
`sqlx::migrate!("../ekapkgs-update/migrations")` and drives it with
`tower::ServiceExt::oneshot`:

- A fixture covers every table the dashboard reads. Timestamps are written as
  `AGO(minutes)` and expanded relative to now, so relative times are stable.
- Each API endpoint has a test against the fixture, and one test requests
  every endpoint against an empty database.
- Further tests cover:
  - filters, sorting and pagination (including bad input);
  - JSON 404s, and the app shell on page routes (200) and unknown paths (404);
  - asset ETag/304, cache headers, and that `elm.js` and `app.js` are present;
  - gzip responses for `elm.js`, the API and the shell when
    `Accept-Encoding` allows them (also when `br` is offered first), and 304s for a compressed asset's ETag;
  - the live fingerprint changing after a write, and `/ws/live` rejecting
    a non-upgrade request.

Query and parameter helpers have unit tests next to their code. The Elm
code has no unit tests (`elm-test` needs node). It is type-checked by every
build.

## Known limitations

- There is no authentication. Bind to localhost or put the portal behind a
  proxy that provides auth.
- `/failures` lists `update_logs`. The error-type breakdown on `/analytics`
  counts failed `update_phases`. The two can differ when a failure was
  recorded in only one of them.
- A repository may not be named `default`, because the dashboard uses that
  name for the `''` repository in URLs. The CLI config and the NixOS module
  reject it.
- ekapkgs packages `elm` only for x86_64-linux. On aarch64-linux the dev
  shell has no `elm`, so `cargo` builds embed the stub frontend, and the
  Nix web package cannot be built there.
- The portal and the CLI must be built from the same migration set: an
  older portal refuses to start against a database migrated by a newer CLI.
