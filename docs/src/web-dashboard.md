# Web Dashboard

`ekapkgs-update-web` is a read-only dashboard for the database that
`ekapkgs-update run` and `ekapkgs-update watch` write to. Use it to see:

- what is running right now and which packages are in flight;
- the update state of every tracked package, per repository;
- why updates failed: structured phase errors and build logs;
- how watch mode is doing: repository sync and index, poll cycles, upstream
  source health;
- trends over time: daily outcomes, phase failure rates, error types,
  rebuild impact, autofix results.

Pages refresh live while the CLI works. The dashboard has no buttons that
change anything. To retry, export or apply fixes, use the CLI (see
[retry](./cli/retry.md) and [export / apply](./cli/export-apply.md)).

## Installation

### NixOS module

```nix
{
  services.ekapkgs-update = {
    enable = true;
    web = {
      enable = true;
      host = "127.0.0.1";   # default; use a reverse proxy for remote access
      port = 3000;          # default
      # database = "/var/lib/ekapkgs-update/updates.db";  # default: the service's database
      # openFirewall = false;
      # cors = false;
    };
  };
}
```

This adds `ekapkgs-update-web.service`. It runs as the same user and group
as the main service and reads the same database. See
[NixOS Module](./nixos-module.md#webenable) for every option.

### Standalone

```bash
nix build .#ekapkgs-update-web
./result/bin/ekapkgs-update-web --database ~/.cache/ekapkgs-update/updates.db
xdg-open http://127.0.0.1:3000
```

| Option | Default | Description |
|---|---|---|
| `-d, --database` | `~/.cache/ekapkgs-update/updates.db` | SQLite database (same default as the CLI) |
| `-p, --port` | `3000` | Port to listen on |
| `--host` | `127.0.0.1` | Address to bind |
| `--cors` | off | Send permissive CORS headers, for API clients on other origins |

Set `RUST_LOG` to change log verbosity (default
`ekapkgs_update_web=info,tower_http=info`).

If the database does not exist yet, it is created and migrated, and the
dashboard shows empty states until the CLI records something. Templates,
CSS and JavaScript are compiled into the binary, so the binary can run from
any directory.

### Behind a reverse proxy

Live updates use a WebSocket at `/ws/live`, so the proxy must forward
upgrades. Without that, pages still work and panels fall back to refreshing
every 60 seconds.

```nix
{
  services.nginx.virtualHosts."updates.example.com" = {
    enableACME = true;
    forceSSL = true;
    locations."/" = {
      proxyPass = "http://127.0.0.1:3000";
      proxyWebsockets = true;
    };
  };
}
```

Caddy (`reverse_proxy localhost:3000`) forwards WebSockets without extra
configuration.

## Pages

Filters are query parameters, so any filtered view can be bookmarked or
shared. Times are relative; hover one to see the exact UTC time. The
repository used by `run` and single-repo watch is shown as `default`.

### Dashboard: `/`

- Package status tiles (tracked, up to date, outdated, proposed, failed,
  upstream errors) and the 30-day success rate. Each tile links to the
  filtered package list.
- Running sessions with their progress counters and the package and phase
  currently in flight.
- Recent update activity and the latest pull requests.
- A watch summary: the last poll, upstream source health, backlog, and each
  repository's sync and index state.

### Packages: `/packages`

Every tracked attribute with its current and latest version, status, PR, last
check, rebuild count and failure count.

- Filters: free-text `q` (attribute path or pname), `repo`, `status`,
  `upstream` (source type).
- Sort by attribute, status, last checked, rebuilds or failures.
- Status `backlog` lists packages that are outdated by a release watch
  already saw at bootstrap. Watch leaves those to `run`.

### Package detail: `/packages/<attr>?repo=<name>`

Everything known about one attribute in one repository:

- **Update state:** status, versions (current, latest upstream, proposed,
  baseline, failed), failure count, last error, and the backoff until the
  next attempt.
- **PR**, pname, description, homepage and the semver strategy.
- **Upstream:** the watch index entries (source URL, version regex,
  prereleases) and the health of the upstream source.
- **Update history:** one row per session, with each phase's outcome and
  duration. Failed phases show their structured error and preserved
  artifacts path.
- **Failure logs** and **autofix** queue entries and attempts.

If the attribute exists in several repositories, links switch between them.

### Sessions: `/sessions` and `/sessions/<id>`

- The list shows every `run` and `watch` session with status, duration,
  outcome bar and success rate.
  - Filter by `status` and `mode`.
  - Finished watch sessions that attempted nothing are hidden by default
    (`show_empty=1` shows them).
- The detail page shows:
  - counters (evaluated, backoff, attempted, succeeded, failed, skipped);
  - for watch sessions, the poll cycle that dispatched them;
  - every package attempted, with per-phase chips, which can be filtered
    by outcome;
  - phase errors, failure logs and the recorded configuration.

  A running session's page updates live.

### Failures: `/failures`

Failure logs, newest first. Filter by text, repository, phase and error type.
Each entry shows the version change, the phase, the error type and the last
6,000 characters of the log.

### Watch: `/watch`

- **Repositories:** sync status, checked-out HEAD, index status, counts
  and duration, and whether bootstrap has finished.
- **Poll and index cycles:** sources polled and failed, releases found,
  updates dispatched, and the session they created.
- **Upstream sources:** health by type, the failing sources with their
  last error, and recently changed releases.

### Analytics: `/analytics`

Choose a range: 7, 30 or 90 days, or all time. The page shows:

- daily outcomes (succeeded and failed per day), with totals and the
  success rate;
- per-phase runs, failure rate, and average, p50, p90 and maximum
  durations;
- error types and the most failing packages;
- package status by repository and by upstream type;
- the packages with the largest rebuild impact;
- autofix queue outcomes and attempt results with token usage.

Every chart has a legend, a tooltip on hover or keyboard focus, and a
"Table view" with the underlying numbers. Light and dark mode follow the
system setting.

## Live updates

Each open tab holds one WebSocket to `/ws/live`.

- The server checks a cheap database fingerprint every 2 seconds and
  notifies the tab only when it changes.
- The tab then re-fetches the JSON for the page on screen and redraws it.
- The indicator in the header shows `live`, `connecting` or `offline`.
  While offline, the client reconnects with backoff up to 30 seconds, and
  panels refresh once a minute.

## JSON API

The same data is available as JSON, with raw values and RFC 3339
timestamps. List endpoints accept `limit` (default 50, maximum 500) and
`page`, plus the same filters as the matching page.

| Endpoint | Returns |
|---|---|
| `GET /api/stats` | Package status counts, 30-day outcomes and success rate, running sessions, upstream source totals |
| `GET /api/dashboard` | Everything on the overview page: counts, current and previous 30-day outcomes, running sessions and their phases, watch summary, recent activity and PRs |
| `GET /api/sessions` | `{sessions, total, page, per_page, hidden_empty, facets}` |
| `GET /api/sessions/<id>` | `session` (with `config_json`), `phases`, `logs`, `cycle` |
| `GET /api/sessions/<id>/phases` | Phase rows in execution order |
| `GET /api/packages` | `{packages, total, all_total, page, per_page, sort, sorts, status_counts, repos, upstream_types}` |
| `GET /api/packages/<attr>?repo=` | `update`, `repos`, `index`, `sources`, `phases`, `logs`, `autofix` |
| `GET /api/failures` | `{logs, total, page, per_page, facets}` |
| `GET /api/watch` | `repos`, `cycles`, `cycle_total`, `cycle_counts`, `last_poll`, `source_types`, `failing_sources`, `recent_releases` |
| `GET /api/analytics?range=` | The aggregates behind the analytics page |

```bash
curl -s 'http://127.0.0.1:3000/api/packages?status=failed&limit=10' | jq '.packages[].attr_path'
```

Unknown sessions, packages and `/api` paths return `404` with `{"error": ..., "status": 404}`.

## Security

- The dashboard only reads. At startup it applies the CLI's database
  migrations and prunes expired cache rows, the same as the CLI does when
  it opens the database.
- There is no authentication. Keep the default `127.0.0.1` bind, or put
  the dashboard behind a proxy that authenticates (for example nginx
  `basicAuthFile`, or an SSO proxy). Build logs and repository paths are
  visible to anyone who can reach it.
- The frontend renders database text as text (never as HTML), and all
  queries are parameterised.

## Troubleshooting

**The dashboard is empty.** Check that `--database` points at the file the
CLI writes. The CLI default is `~/.cache/ekapkgs-update/updates.db`; the
NixOS service uses `/var/lib/ekapkgs-update/updates.db`.

```bash
sqlite3 ~/.cache/ekapkgs-update/updates.db 'SELECT COUNT(*) FROM updates;'
```

**The service fails to start with a migration error.** The database was
migrated by a newer `ekapkgs-update` than the dashboard. Deploy both from
the same revision.

**The indicator stays `offline`.** The WebSocket is being blocked. Enable
WebSocket forwarding in the reverse proxy (see above). Pages keep working
and refresh every minute.

**Logs.**

```bash
journalctl -u ekapkgs-update-web.service -f
```

## See also

- [NixOS Module](./nixos-module.md): declarative setup
- [Systemd Service](./systemd.md): manual service configuration
- [status](./cli/status.md) and [query](./cli/query.md): CLI alternatives
