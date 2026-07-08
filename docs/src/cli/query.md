# query - Search Update Failures

The `query` command searches and filters update failures from the database. It's essential for analyzing patterns, debugging issues, and understanding update trends.

## Synopsis

```bash
ekapkgs-update query [OPTIONS]
```

## Description

Query the update database to find failures and patterns. The command searches two data sources:

- **Phase records** (`update_phases` table) — detailed per-phase tracking from instrumented update runs.
- **Update logs** (`update_logs` table) — failure records from `run --commit-strategy branch` and other run-mode updates.

When phase records match the query, those are shown. Otherwise, the command falls back to update logs, which is where most `run` command failures are recorded.

## Options

### Database

#### `--database <PATH>` (short: `-d`)
Path to SQLite database.

**Default:** `~/.cache/ekapkgs-update/updates.db`

```bash
ekapkgs-update query --database /var/lib/ekapkgs-update/updates.db
```

### Filtering

#### `--status <STATUS>`
Filter by update status.

**Values:** `success`, `failed`, `running`, `skipped`

```bash
# Find all failures
ekapkgs-update query --status failed

# Find all failures, limited to 20
ekapkgs-update query --status failed --limit 20
```

#### `--package <PATTERN>`
Filter by package name using SQL LIKE pattern.

```bash
# All Python package failures
ekapkgs-update query --status failed --package "python%"

# Specific package
ekapkgs-update query --status failed --package "terraform"

# All packages containing "rust"
ekapkgs-update query --status failed --package "%rust%"
```

**SQL LIKE patterns:**
- `%` — Matches any sequence of characters
- `_` — Matches any single character

#### `--since-days <N>`
Filter to entries from the last N days.

```bash
# Last 24 hours
ekapkgs-update query --status failed --since-days 1

# Last week
ekapkgs-update query --status failed --since-days 7
```

#### `--limit <N>`
Limit number of results.

```bash
# Show only 10 most recent failures
ekapkgs-update query --status failed --limit 10
```

#### `--error-type <TYPE>`
Filter by error type (phase records only).

```bash
ekapkgs-update query --error-type "BuildFailure"
```

#### `--phase <PHASE>`
Filter by update phase (phase records only).

```bash
ekapkgs-update query --phase "Build"
```

### Grouping

#### `--group-by-error`
Group results by error type and show counts (phase records only).

```bash
ekapkgs-update query --group-by-error
```

## Querying Failed Packages

After a `run` completes, use `--status failed` to see which packages failed to build:

```bash
# List all failures from the most recent run
ekapkgs-update query --status failed

# List failures for Python packages only
ekapkgs-update query --status failed --package "python312Packages.%"

# List failures from the last day
ekapkgs-update query --status failed --since-days 1 --limit 50
```

### Example output

```
Found 30 failed update(s)

Package                                  Old             New             When            Error
------------------------------------------------------------------------------------------------------------------------
shadow                                   4.18.0          4.19.4          13h ago         Package build failed after update with no reversed
rust-cbindgen                            0.29.2          0.29.4          13h ago         Could not extract correct cargoHash from build err
python312Packages.cffi                   2.0.0           2.1.0           14h ago         Package build failed after update with no reversed
python312Packages.wheel                  0.46.1          0.47.0          14h ago         Could not extract correct hash from build error:
```

Each row shows:
- **Package** — the attribute path of the failed package
- **Old / New** — the version transition that was attempted
- **When** — relative timestamp of the failure
- **Error** — first line of the error message (truncated)

### Getting full error details

Once you identify a failed package, use `log` or `inspect` to see the full error:

```bash
# View full failure log for a package
ekapkgs-update log shadow

# View detailed failure information
ekapkgs-update inspect shadow
```

## Examples

### After a batch run

```bash
# 1. See what failed
ekapkgs-update query --status failed --limit 30

# 2. Investigate a specific failure
ekapkgs-update log python312Packages.cffi

# 3. Retry a failed update manually
ekapkgs-update update python312Packages.cffi --commit
```

### Filter by package ecosystem

```bash
# All Python 3.12 failures
ekapkgs-update query --status failed --package "python312Packages.%"

# All AWS SDK failures
ekapkgs-update query --status failed --package "aws-%"

# All Haskell failures
ekapkgs-update query --status failed --package "haskellPackages.%"
```

### Time-scoped queries

```bash
# What failed in the last run (assuming it was today)
ekapkgs-update query --status failed --since-days 1

# What failed this week
ekapkgs-update query --status failed --since-days 7
```

## Integration with Other Commands

### Query -> Log -> Manual Fix

```bash
# 1. Find recent failures
ekapkgs-update query --status failed --since-days 1

# 2. View the full error for a package
ekapkgs-update log shadow

# 3. Fix the package manually, then retry
ekapkgs-update update shadow --commit
```

### Query -> Export -> LLM

```bash
# 1. Find failures to analyze
ekapkgs-update query --status failed --limit 5

# 2. Export for AI analysis
ekapkgs-update export python312Packages.requests --format markdown

# 3. Apply AI-generated fix
ekapkgs-update apply python312Packages.requests --patch fix.patch --resume
```

## Exit Codes

- `0` — Query executed successfully (results may be empty)
- `1` — Database error or query failure
- `2` — Invalid arguments

## See Also

- [log / inspect](./inspect.md) — View detailed failure information
- [status](./status.md) — View update session status
- [retry](./retry.md) — Retry failed updates
- [Debugging Use Case](../use-cases/debugging.md)
