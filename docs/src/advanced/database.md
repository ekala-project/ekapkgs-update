# SQLite Database Schema

ekapkgs-update uses SQLite to persistently track package updates, maintain history, cache external API data, and manage backoff state. This document describes the database schema, initialization, and key operations.

## Overview

The database stores:
1. **Update records**: the state of each package per repository (status, versions, failures, backoff, PRs, cached metadata)
2. **Update logs**: failure records for post-mortem analysis
3. **Session tracking**: one row per `run`, and one per `watch` poll cycle that dispatched updates
4. **Phase records**: per-package workflow phases within a session
5. **Watch state**: watched repositories, the poll/index cycle log, upstream source health and the upstream index
6. **Cache entries**: cached CVE and Repology data, to minimize API calls

The database uses SQLite's WAL (Write-Ahead Logging) mode for concurrency. It is
initialized with the schema migrations in `ekapkgs-update/migrations/`. The web
dashboard (`ekapkgs-update-web`) reads the same file.

### Repository scoping

Package rows are keyed by `(repo_name, attr_path)`:

- `repo_name = ''` is used by the `run` command and by legacy single-repo `watch`.
- Multi-repo `watch` uses the `name` of each `[[repos]]` entry. The name
  `default` is reserved, because the dashboard uses it as the label for `''`.

`update_logs`, `update_phases`, `autofix_queue` and `autofix_attempts` carry
`repo_name` too.

## Core Tables

### updates

The current state of each package and when it may be re-checked.

| Column | Type | Description |
|--------|------|-------------|
| `repo_name` | TEXT | Repository scope (`''` = default) |
| `attr_path` | TEXT | Nix attribute path |
| `status` | TEXT | `pending`, `up_to_date`, `outdated`, `proposed`, `failed`, `upstream_error` |
| `current_version` | TEXT | Version in the tree |
| `latest_upstream_version` | TEXT | Best upstream release compatible with the package's strategy |
| `proposed_version` | TEXT | Version that was built and proposed (PR or commit) |
| `baseline_version` | TEXT | `watch`: upstream version recorded by the first observation |
| `failed_version` | TEXT | Target version of the last failed attempt |
| `failure_count` | INTEGER | Consecutive failed attempts (reset on success) |
| `last_error` | TEXT | One-line summary of the last failure / upstream error |
| `last_session_id` | TEXT | Session of the last attempt |
| `last_checked` | TEXT | Last comparison against upstream |
| `last_attempted` | TEXT | Last update attempt |
| `next_attempt` | TEXT | Backoff: not checked / attempted before this |
| `pr_url`, `pr_number` | TEXT, INTEGER | Pull request created for `proposed_version` |
| `rebuild_count` | INTEGER | Packages that rebuild if the update lands |
| `cve_resolved_count`, `cve_introduced_count` | INTEGER | OSV analysis of the proposed update |
| `pname`, `description`, `homepage` | TEXT | Cached package metadata |
| `upstream_type`, `upstream_key`, `upstream_instance` | TEXT | Upstream source |
| `semver_strategy` | TEXT | Effective strategy (`latest`, `major`, `minor`, `patch`) |

**Primary Key:** `(repo_name, attr_path)`

A `proposed` package becomes `up_to_date` when the tree's `current_version`
catches up with `proposed_version` (the PR was merged).

### update_logs

Failure details for post-mortem analysis (`log`, `query`, `report`, `retry`).

| Column | Type | Description |
|--------|------|-------------|
| `drv_path` | TEXT PRIMARY KEY | Derivation path, or `<repo>:<attr_path>@<version>` when there is none (watch) |
| `repo_name` | TEXT | Repository scope |
| `attr_path` | TEXT | Nix attribute path |
| `session_id` | TEXT | Session of the attempt |
| `timestamp` | TEXT | When the failure occurred |
| `status` | TEXT | Always `failed` |
| `phase` | TEXT | Phase that failed (e.g. `build`, `testing`) |
| `error_type` | TEXT | `UpdateError` variant (e.g. `BuildError`, `HashMismatchError`) |
| `error_log` | TEXT | Complete error message and context |
| `old_version`, `new_version` | TEXT | Attempted version change |

### update_sessions

One row per `run`, and one per `watch` poll cycle that dispatched updates.

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PRIMARY KEY | UUID |
| `mode` | TEXT | `run` or `watch` |
| `started_at`, `completed_at`, `updated_at` | TEXT | Timing; `updated_at` is a heartbeat |
| `status` | TEXT | `running`, `completed`, `failed`, `cancelled` |
| `packages_evaluated` | INTEGER | Packages compared against upstream |
| `packages_backoff` | INTEGER | Packages skipped because of backoff |
| `packages_attempted` | INTEGER | Updates started (maintained live) |
| `packages_succeeded` | INTEGER | Updates built and proposed |
| `packages_failed` | INTEGER | Updates that failed |
| `packages_skipped` | INTEGER | Not attempted (setup failure, rebuild limit, dry run) |
| `config_json` | TEXT | Run configuration |

### update_phases

Workflow phases of each package update within a session.

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PRIMARY KEY | Phase record ID |
| `session_id` | TEXT | Session UUID |
| `repo_name`, `attr_path` | TEXT | Package |
| `phase` | TEXT | `metadata_extraction`, `source_discovery`, `version_selection`, `source_hash_update`, `dependency_hash_update`, `build`, `patch_recovery`, `testing`, `agent_fix`, `pr_creation` |
| `started_at`, `completed_at` | TEXT | Timing |
| `duration_ms` | INTEGER | Duration |
| `status` | TEXT | `running`, `success`, `failed`, `skipped` |
| `error_type` | TEXT | `UpdateError` variant, e.g. `BuildError` |
| `error_details` | TEXT | JSON `UpdateError` (tagged with `"type": "build_error"`, ...) |
| `artifacts_path` | TEXT | Preserved failure artifacts (`--preserve-failures`) |

## Watch Tables

### watched_repos

One row per repository watched by `ekapkgs-update watch` (`''` = legacy mode).

| Column | Description |
|--------|-------------|
| `name`, `upstream`, `path`, `entrypoint`, `base_branch`, `fork_remote` | Configuration |
| `head_commit`, `head_subject` | `HEAD` after the last sync |
| `sync_status`, `sync_error`, `last_synced_at` | `ok`, `diverged`, `error`, `unmanaged` |
| `index_status`, `index_error`, `last_indexed_at`, `index_duration_ms` | `indexing`, `ok`, `error` |
| `evaluated_count`, `indexed_count`, `index_skipped`, `index_errors` | Last index build |
| `bootstrapped_at` | First completed poll (baselines recorded) |

### watch_cycles

One row per poll cycle or index build.

| Column | Description |
|--------|-------------|
| `kind` | `poll` or `index` |
| `repo_name` | Index: the repo. Poll: `NULL` (all repos) |
| `started_at`, `completed_at`, `duration_ms`, `status`, `error` | `running`, `completed`, `failed` |
| `bootstrap` | Poll recorded at least one baseline |
| `sources_polled`, `sources_failed`, `new_releases` | Poll statistics |
| `packages_observed` | Poll: packages compared. Index: packages indexed |
| `dispatched`, `updated`, `failed`, `session_id` | Updates started by this poll |

Uneventful poll cycles are pruned after 7 days.

### upstream_sources

Poll health and latest release per unique upstream source.

| Column | Description |
|--------|-------------|
| `upstream_type`, `upstream_key`, `instance` | Source (`instance = ''` when not applicable) |
| `latest_version`, `latest_tag`, `release_count`, `latest_changed_at` | Newest stable release |
| `first_seen_at`, `last_polled_at`, `last_success_at` | Timing |
| `last_error`, `consecutive_failures` | Poll failures |

Join against `upstream_index` with
`COALESCE(upstream_index.instance, '') = upstream_sources.instance`.

### upstream_index

Maps `(repo_name, attr_path)` to its upstream source. Also stores the
version-selection settings the poller uses: `pname`, `semver_strategy`,
`version_regex`, `include_prereleases`, `current_version`, `src_url` and
`indexed_at`.

## Autofix Tables

### autofix_queue

One row per preserved failure awaiting or undergoing an LLM fix. It is unique
on `(repo_name, attr_path, session_id)`, because one watch session can fail the
same attr in several repos. `status` is one of `queued`, `processing`, `fixed`,
`escalated` or `skipped`.

### autofix_attempts

One row per LLM attempt. `queue_id` references `autofix_queue`. `repo_name` is
copied from the queue row when the attempt is inserted.

### autofix_embeddings

Embedded error summaries of past attempts, used for retrieval. These are not
repo-scoped: a fix that worked in one repo is a useful example for another.

## Cache Tables

### cve_cache

Caches CVE/vulnerability data from the OSV database to minimize API calls.

| Column | Type | Description |
|--------|------|-------------|
| `ecosystem` | TEXT | Package ecosystem (e.g., 'PyPI', 'crates.io') |
| `package_name` | TEXT | Name of the package in that ecosystem |
| `version` | TEXT | Specific package version |
| `cached_data` | TEXT | JSON-serialized vulnerability data |
| `cached_at` | TEXT | RFC 3339 timestamp of when data was cached |
| `expires_at` | TEXT | RFC 3339 timestamp of when the cache entry should be refreshed |

**Primary Key:** `(ecosystem, package_name, version)`

### repology_cache

Caches Repology API responses for cross-distribution version comparisons.

| Column | Type | Description |
|--------|------|-------------|
| `package_name` | TEXT PRIMARY KEY | Package name as used in Repology |
| `cached_data` | TEXT | JSON-serialized Repology data |
| `cached_at` | TEXT | RFC 3339 timestamp of when data was cached |
| `expires_at` | TEXT | RFC 3339 timestamp of when the cache entry should be refreshed |

## Backoff Strategy

See [Backoff Strategy](./backoff.md). In short:

- **Checks with nothing to do** (`record_no_update`): 2 → 4 → 6 days, based on the time since the last check.
- **Failed attempts** (`record_failed_update`): 2 → 4 → 6 days, based on `failure_count`.
- **Successful updates** reset `failure_count` and set the backoff to 2 days.

## Rebuild Impact Analysis

The `rebuild_count` field in the `updates` table tracks how many other packages would need to rebuild if an update succeeds. This is computed via `nix why-depends` analysis.

### Rebuild Buckets

Results are categorized into buckets for analytics:

| Bucket | Range | Label |
|--------|-------|-------|
| Small | 0-10 | "0-10" |
| Medium | 11-50 | "11-50" |
| Large | 51-100 | "51-100" |
| Huge | 101+ | "101+" |

### Query Example

Get rebuild count distribution:

```sql
SELECT
    CASE
        WHEN rebuild_count <= 10 THEN '0-10'
        WHEN rebuild_count <= 50 THEN '11-50'
        WHEN rebuild_count <= 100 THEN '51-100'
        ELSE '101+'
    END as bucket,
    COUNT(*) as count
FROM updates
WHERE rebuild_count IS NOT NULL
GROUP BY bucket
ORDER BY
    CASE bucket
        WHEN '0-10' THEN 1
        WHEN '11-50' THEN 2
        WHEN '51-100' THEN 3
        ELSE 4
    END;
```

**Result:**
```
bucket   | count
---------|------
0-10     | 245
11-50    | 89
51-100   | 34
101+     | 12
```

## Database Initialization

The database is initialized by calling `Database::new(db_path)`:

1. **Connection Setup**: Creates/opens SQLite database at the specified path
2. **Options**: Enables WAL mode for better concurrency
3. **Migrations**: Runs schema migrations from `./migrations` directory
4. **Cleanup**: Opportunistically cleans up expired cache entries
5. **Logging**: Reports initialization completion and any cleanup failures (non-fatal)

### Rust Example

```rust
use ekapkgs_update::database::Database;

let db = Database::new("~/.cache/ekapkgs-update/updates.db").await?;

// repo_name '' is the default scope used by `run`
let should_check = db.should_check_update("", "python312Packages.requests").await?;
```

## Common Queries

### Find packages ready to check

```sql
SELECT attr_path, current_version, latest_upstream_version
FROM updates
WHERE next_attempt IS NULL OR next_attempt <= datetime('now')
LIMIT 10;
```

### Find high-impact updates

```sql
SELECT attr_path, current_version, proposed_version, rebuild_count
FROM updates
WHERE proposed_version IS NOT NULL
  AND rebuild_count IS NOT NULL
ORDER BY rebuild_count DESC
LIMIT 20;
```

### Get recent failures for a package

```sql
SELECT timestamp, phase, error_type, old_version, new_version
FROM update_logs
WHERE attr_path = 'python312Packages.requests'
ORDER BY timestamp DESC
LIMIT 5;
```

### Session success rate

```sql
SELECT
    status,
    COUNT(*) as session_count,
    ROUND(AVG(CAST(packages_succeeded AS FLOAT) / packages_attempted * 100), 2) as avg_success_rate
FROM update_sessions
WHERE completed_at >= datetime('now', '-7 days')
GROUP BY status;
```

### Find phases that took longest

```sql
SELECT attr_path, phase, duration_ms
FROM update_phases
WHERE duration_ms IS NOT NULL
ORDER BY duration_ms DESC
LIMIT 10;
```

## Connection Pool

The database uses a connection pool (via `sqlx::SqlitePool`) for efficient concurrent access. The pool:

- Maintains multiple SQLite connections
- Automatically reuses connections across operations
- Implements connection timeouts and resource limits
- Is held for the lifetime of the `Database` struct
- Supports cloning to share across async tasks

### Concurrency Example

```rust
let db = Database::new("updates.db").await?;
let db_clone = db.clone();  // Shares the same connection pool

tokio::spawn(async move {
    db_clone
        .record_successful_update("", "pkg1", "1.0", "2.0", None, None)
        .await
});

db.record_no_update("", "pkg2", "1.0", Some("1.0"), None).await?;
```

## Performance Considerations

1. **Indexing**: `(repo_name, attr_path)` is the primary key of `updates`; `attr_path`, `status` and the upstream columns are indexed as well
2. **WAL Mode**: Enables concurrent reads while writes are in progress
3. **Cache Cleanup**: Expired cache entries are cleaned during initialization, not continuously
4. **Query Planning**: Use `EXPLAIN QUERY PLAN` for complex queries to ensure indexes are used

## Error Handling

Most database operations return `anyhow::Result<T>`. Common error scenarios:

1. **Connection failures**: Database path invalid or inaccessible
2. **Migration failures**: Schema version mismatch or corrupted database
3. **Constraint violations**: ON CONFLICT clauses handle duplicate key scenarios
4. **Transient failures**: Wrapped with context for debugging

All errors include context messages that identify the failing operation.
