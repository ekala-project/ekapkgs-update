# SQLite Database Schema

ekapkgs-update uses SQLite to persistently track package updates, maintain history, cache external API data, and manage backoff state. This document describes the database schema, initialization, and key operations.

## Overview

The database stores:
1. **Update records** - Current state, version history, and timing information for each package
2. **Update logs** - Failure records for post-mortem analysis
3. **Session tracking** - Metadata about entire update runs
4. **Phase records** - Granular tracking of update stages within a session
5. **Cache entries** - Cached CVE and Repology data to minimize API calls

The database uses SQLite's WAL (Write-Ahead Logging) mode for concurrency and is initialized with schema migrations.

## Core Tables

### updates

Tracks the current state of each package and when it should be re-checked.

| Column | Type | Description |
|--------|------|-------------|
| `attr_path` | TEXT PRIMARY KEY | Nix attribute path (e.g., `python312Packages.django`) |
| `last_attempted` | TEXT | RFC 3339 timestamp of the last check attempt |
| `next_attempt` | TEXT | RFC 3339 timestamp when the package should be checked again |
| `current_version` | TEXT | Current package version |
| `proposed_version` | TEXT | Next version to attempt (NULL if no update pending) |
| `latest_upstream_version` | TEXT | Latest version available upstream |
| `rebuild_count` | INTEGER | Number of packages that would rebuild if this update succeeds |
| `pr_url` | TEXT | URL of the PR created for this update |
| `pr_number` | INTEGER | GitHub/GitLab PR number |

**Example:**
```
attr_path: "python312Packages.requests"
last_attempted: "2024-05-15T10:30:00Z"
next_attempt: "2024-05-17T10:30:00Z"
current_version: "2.31.0"
proposed_version: "2.32.0"
latest_upstream_version: "2.32.0"
rebuild_count: 15
pr_url: "https://github.com/example/example/pull/12345"
pr_number: 12345
```

### update_logs

Records failure details for post-mortem analysis and debugging.

| Column | Type | Description |
|--------|------|-------------|
| `drv_path` | TEXT PRIMARY KEY | Full derivation path from `/nix/store/` |
| `attr_path` | TEXT | Nix attribute path (for correlation) |
| `timestamp` | TEXT | RFC 3339 timestamp when failure occurred |
| `status` | TEXT | Always 'failed' (for future extensibility) |
| `error_log` | TEXT | Complete error message and context |
| `old_version` | TEXT | Version before failed attempt |
| `new_version` | TEXT | Version that was attempted |

**Example:**
```
drv_path: "/nix/store/abc123def456-python312Packages-requests-2.32.0.drv"
attr_path: "python312Packages.requests"
timestamp: "2024-05-15T10:30:00Z"
status: "failed"
error_log: "Hash mismatch: expected sha256:xxx, got sha256:yyy"
old_version: "2.31.0"
new_version: "2.32.0"
```

### update_sessions

Tracks metadata about entire update runs.

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PRIMARY KEY | UUID of the session |
| `started_at` | TEXT | RFC 3339 timestamp when the session started |
| `completed_at` | TEXT | RFC 3339 timestamp when the session completed (NULL if still running) |
| `status` | TEXT | Session status: 'running', 'completed', 'failed', 'cancelled' |
| `packages_attempted` | INTEGER | Number of packages attempted in this session |
| `packages_succeeded` | INTEGER | Number of packages successfully updated |
| `packages_failed` | INTEGER | Number of packages that failed |
| `packages_skipped` | INTEGER | Number of packages skipped (e.g., no update available) |
| `config_json` | TEXT | JSON serialization of the run configuration |

**Example:**
```
id: "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
started_at: "2024-05-15T08:00:00Z"
completed_at: "2024-05-15T14:30:00Z"
status: "completed"
packages_attempted: 150
packages_succeeded: 120
packages_failed: 15
packages_skipped: 15
config_json: "{...}"
```

### update_phases

Records granular execution stages for each package within a session.

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PRIMARY KEY AUTOINCREMENT | Auto-incrementing phase record ID |
| `session_id` | TEXT | Reference to the update_sessions UUID |
| `attr_path` | TEXT | Nix attribute path being updated |
| `phase` | TEXT | Phase name (e.g., 'fetch', 'build', 'test', 'publish') |
| `started_at` | TEXT | RFC 3339 timestamp when the phase started |
| `completed_at` | TEXT | RFC 3339 timestamp when the phase completed |
| `duration_ms` | INTEGER | Duration in milliseconds |
| `status` | TEXT | Phase status: 'running', 'success', 'failed' |
| `error_type` | TEXT | Type of error if failed (e.g., 'HashMismatch') |
| `error_details` | TEXT | JSON serialization of error details |
| `artifacts_path` | TEXT | Path to preserved failure artifacts |

**Example:**
```
id: 42
session_id: "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
attr_path: "python312Packages.requests"
phase: "build"
started_at: "2024-05-15T08:15:00Z"
completed_at: "2024-05-15T08:18:30Z"
duration_ms: 210000
status: "failed"
error_type: "BuildFailure"
error_details: "{...}"
artifacts_path: "/home/user/.cache/ekapkgs-update/failed/a1b2c3d4.../python312Packages_requests/"
```

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

**Example:**
```
ecosystem: "PyPI"
package_name: "requests"
version: "2.32.0"
cached_data: "[{\"id\":\"CVE-2023-12345\",\"severity\":\"MEDIUM\",...}]"
cached_at: "2024-05-15T08:00:00Z"
expires_at: "2024-05-22T08:00:00Z"
```

### repology_cache

Caches Repology API responses for cross-distribution version comparisons.

| Column | Type | Description |
|--------|------|-------------|
| `package_name` | TEXT PRIMARY KEY | Package name as used in Repology |
| `cached_data` | TEXT | JSON-serialized Repology data |
| `cached_at` | TEXT | RFC 3339 timestamp of when data was cached |
| `expires_at` | TEXT | RFC 3339 timestamp of when the cache entry should be refreshed |

**Example:**
```
package_name: "requests"
cached_data: "{\"repositories\":{\"pypi\":{\"version\":\"2.32.0\"},\"debian\":{\"version\":\"2.31.0\"}}}"
cached_at: "2024-05-15T08:00:00Z"
expires_at: "2024-05-22T08:00:00Z"
```

### backoff_state

Manages exponential backoff for failed package checks.

| Column | Type | Description |
|--------|------|-------------|
| `attr_path` | TEXT PRIMARY KEY | Nix attribute path |
| `consecutive_failures` | INTEGER | Number of consecutive failed attempts |
| `last_failure_time` | TEXT | RFC 3339 timestamp of most recent failure |
| `backoff_multiplier` | REAL | Current backoff multiplier (defaults to 1.0) |

**Note:** This table is implicit in the `next_attempt` field of the `updates` table. See [Backoff Strategy](#backoff-strategy) below.

## Backoff Strategy

The system implements exponential backoff to prevent excessive API traffic while ensuring packages are eventually re-checked.

### Backoff Schedule

- **First failure** (no prior attempt): re-check in **2 days**
- **Second failure** (within 2 days of first): re-check in **4 days**
- **Subsequent failures**: re-check in **6 days** (maximum backoff)
- **Successful update**: reset backoff to **2 days**

### Implementation

When `record_no_update()` is called, the system:

1. Retrieves the package's `last_attempted` timestamp
2. Calculates the days since that attempt
3. Determines the backoff duration:
   ```
   if last_attempted is NULL:
       backoff = 2 days     # First failure
   else if (now - last_attempted) <= 2 days:
       backoff = 4 days     # Recent retry
   else:
       backoff = 6 days     # Older retry
   ```
4. Sets `next_attempt = now + backoff`
5. Updates the database

### Examples

**Scenario 1: Package never checked before**
```
last_attempted: NULL
next_attempt: now + 2 days
```

**Scenario 2: Checked 1 day ago, no update available again**
```
last_attempted: 2024-05-15T08:00:00Z (1 day ago)
now: 2024-05-16T08:00:00Z
Decision: (now - last_attempted) = 1 day <= 2 days → backoff = 4 days
next_attempt: 2024-05-20T08:00:00Z
```

**Scenario 3: Checked 3 days ago, no update available again**
```
last_attempted: 2024-05-12T08:00:00Z (3 days ago)
now: 2024-05-15T08:00:00Z
Decision: (now - last_attempted) = 3 days > 2 days → backoff = 6 days
next_attempt: 2024-05-21T08:00:00Z
```

**Scenario 4: Successful update resets backoff**
```
Any previous state → record_successful_update() called
next_attempt: now + 2 days
```

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

// Now ready to use:
let should_check = db.should_check_update("python312Packages.requests").await?;
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
SELECT timestamp, status, error_log, old_version, new_version
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
    db_clone.record_successful_update("pkg1", "1.0", "2.0").await?
});

db.record_no_update("pkg2", "1.0", "1.0.1").await?;
```

## Performance Considerations

1. **Indexing**: The `attr_path` is a primary key in `updates` for fast lookups
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
