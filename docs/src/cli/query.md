# query - Search Update Failures

The `query` command searches and filters update failures from the database. It's essential for analyzing patterns, debugging issues, and understanding update trends.

## Synopsis

```bash
ekapkgs-update query [OPTIONS]
```

## Description

Query the update database to find failures, successful updates, and patterns. Results can be filtered by error type, phase, status, package name, and time range.

## Options

### Database

#### `--database <PATH>` (short: `-d`)
Path to SQLite database.

**Default:** `~/.cache/ekapkgs-update/db.sqlite3`

```bash
ekapkgs-update query --database /var/lib/ekapkgs-update/db.sqlite3
```

### Filtering

#### `--error-type <TYPE>`
Filter by error type.

```bash
# Find all hash mismatches
ekapkgs-update query --error-type "HashMismatch"

# Find build failures
ekapkgs-update query --error-type "BuildFailure"

# Find network errors
ekapkgs-update query --error-type "NetworkError"
```

**Common error types:**
- `HashMismatch` - Hash verification failed
- `BuildFailure` - Package build failed
- `TestFailure` - passthru.tests failed
- `NetworkError` - Network/API issues
- `PatchFailure` - Patch application failed
- `VersionNotFound` - No compatible version found
- `EvaluationError` - Nix evaluation failed

#### `--phase <PHASE>`
Filter by update phase.

```bash
# Find failures during hash update
ekapkgs-update query --phase "UpdateHash"

# Find failures during build
ekapkgs-update query --phase "Build"

# Find failures during test
ekapkgs-update query --phase "Test"
```

**Update phases:**
- `Evaluation` - Package evaluation
- `VersionFetch` - Fetching available versions
- `UpdateHash` - Updating source hash
- `UpdateDependencyHashes` - Updating cargo/npm/vendor hashes
- `Build` - Building package
- `Test` - Running tests
- `Commit` - Creating git commit
- `PR` - Creating pull request

#### `--status <STATUS>`
Filter by update status.

**Values:** `success`, `failed`, `running`, `skipped`

```bash
# Find all failures
ekapkgs-update query --status failed

# Find successful updates
ekapkgs-update query --status success

# Find skipped packages
ekapkgs-update query --status skipped
```

#### `--package <PATTERN>`
Filter by package name using SQL LIKE pattern.

```bash
# All Python packages
ekapkgs-update query --package "python%"

# Specific package
ekapkgs-update query --package "terraform"

# All packages containing "lib"
ekapkgs-update query --package "%lib%"
```

**SQL LIKE patterns:**
- `%` - Matches any sequence of characters
- `_` - Matches any single character
- Case-insensitive by default

#### `--since-days <N>`
Filter to entries from the last N days.

```bash
# Last week
ekapkgs-update query --since-days 7

# Last 24 hours
ekapkgs-update query --since-days 1

# Last month
ekapkgs-update query --since-days 30
```

#### `--limit <N>`
Limit number of results.

```bash
# Show only 10 most recent failures
ekapkgs-update query --limit 10

# Show top 50 results
ekapkgs-update query --limit 50
```

### Grouping

#### `--group-by-error`
Group results by error type and show counts.

```bash
ekapkgs-update query --group-by-error
```

**Example output:**
```
Error Type Summary:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
HashMismatch        : 23 occurrences
BuildFailure        : 15 occurrences
TestFailure         : 8 occurrences
PatchFailure        : 5 occurrences
NetworkError        : 3 occurrences
VersionNotFound     : 2 occurrences
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Total failures: 56
```

## Examples

### Basic Queries

```bash
# All failures (no filters)
ekapkgs-update query

# Recent failures (last 7 days)
ekapkgs-update query --since-days 7

# Limited results
ekapkgs-update query --limit 20
```

### Error Analysis

```bash
# Group failures by error type
ekapkgs-update query --group-by-error

# Find all hash mismatches
ekapkgs-update query --error-type "HashMismatch"

# Recent hash mismatches
ekapkgs-update query --error-type "HashMismatch" --since-days 7
```

### Package Searches

```bash
# All Python package failures
ekapkgs-update query --package "python%"

# Specific package history
ekapkgs-update query --package "terraform"

# All Haskell packages
ekapkgs-update query --package "haskellPackages%"
```

### Phase Analysis

```bash
# Failures during build phase
ekapkgs-update query --phase "Build"

# Hash update failures
ekapkgs-update query --phase "UpdateHash"

# Test failures
ekapkgs-update query --phase "Test"
```

### Combined Filters

```bash
# Recent Python build failures
ekapkgs-update query \
  --package "python%" \
  --phase "Build" \
  --since-days 7

# Recent hash mismatches, top 10
ekapkgs-update query \
  --error-type "HashMismatch" \
  --since-days 7 \
  --limit 10

# All test failures for a specific package
ekapkgs-update query \
  --package "gcc" \
  --phase "Test" \
  --status "failed"
```

### Status Queries

```bash
# All successful updates from last 24 hours
ekapkgs-update query --status success --since-days 1

# Find what's currently running
ekapkgs-update query --status running

# Find skipped packages
ekapkgs-update query --status skipped --group-by-error
```

### Debugging Workflows

```bash
# 1. Get overview
ekapkgs-update query --group-by-error --since-days 7

# 2. Investigate specific error type
ekapkgs-update query --error-type "HashMismatch" --limit 10

# 3. Check specific package
ekapkgs-update query --package "mypackage"

# 4. Get detailed info
ekapkgs-update inspect mypackage
```

## Output Format

### Standard Output

```
Update Failures:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Package: python312Packages.requests
Status: failed
Phase: Build
Error: BuildFailure
Updated: 2024-05-19 10:30:15
Message: builder for '/nix/store/...-python3.12-requests-2.32.0.drv' failed with exit code 1

Package: terraform
Status: failed
Phase: UpdateHash
Error: HashMismatch
Updated: 2024-05-19 09:15:42
Message: hash mismatch in fixed-output derivation

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Total results: 2
```

### Grouped Output

```
Error Type Summary:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

HashMismatch (15):
  - python312Packages.requests (2024-05-19 10:30:15)
  - nodejs (2024-05-19 09:45:22)
  - terraform (2024-05-19 09:15:42)
  ... and 12 more

BuildFailure (8):
  - gcc (2024-05-19 11:00:00)
  - rust (2024-05-19 10:45:30)
  - llvm (2024-05-19 10:20:15)
  ... and 5 more

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Total failures: 23
```

## Common Patterns

### Weekly Review

```bash
# Overview of last week
ekapkgs-update query --since-days 7 --group-by-error

# Most problematic packages
ekapkgs-update query --since-days 7 --limit 20
```

### CI/CD Monitoring

```bash
# Check if any failures in last run
if ekapkgs-update query --since-days 1 --status failed | grep -q "Total results: 0"; then
  echo "All updates succeeded"
else
  echo "Some updates failed, checking details..."
  ekapkgs-update query --since-days 1 --status failed --group-by-error
fi
```

### Debugging Specific Issues

```bash
# Find all instances of hash mismatch
ekapkgs-update query --error-type "HashMismatch"

# Find which packages consistently fail
ekapkgs-update query --status failed --limit 100 | grep "Package:" | sort | uniq -c | sort -rn
```

### Package Maintenance

```bash
# Check update history for a package before making changes
ekapkgs-update query --package "mypackage"

# See if test failures are common
ekapkgs-update query --phase "Test" --since-days 30
```

## Integration with Other Commands

### Query -> Inspect -> Retry

```bash
# 1. Find recent failures
ekapkgs-update query --since-days 1 --status failed

# 2. Inspect specific failure
ekapkgs-update inspect python312Packages.requests

# 3. View preserved worktree
ekapkgs-update worktrees show python312Packages.requests

# 4. Retry after fixing
ekapkgs-update retry python312Packages.requests
```

### Query -> Export -> LLM

```bash
# 1. Find failures to analyze
ekapkgs-update query --phase "Build" --limit 5

# 2. Export for AI analysis
ekapkgs-update export python312Packages.requests --format markdown

# 3. Apply AI-generated fix
ekapkgs-update apply python312Packages.requests --patch fix.patch --resume
```

## Database Schema

The query command searches these database tables:

- `update_attempts` - Individual update attempts
- `update_sessions` - Batch update runs
- `error_types` - Error classifications
- `packages` - Package metadata

For schema details, see [Database Schema](../advanced/database.md).

## Performance

For large databases:

```bash
# Use limit to improve query speed
ekapkgs-update query --limit 100

# Filter by time to reduce dataset
ekapkgs-update query --since-days 7

# Use specific filters instead of scanning all records
ekapkgs-update query --package "python%" --since-days 7
```

## Exit Codes

- `0` - Query executed successfully (results may be empty)
- `1` - Database error or query failure
- `2` - Invalid arguments

## See Also

- [inspect](./inspect.md) - View detailed failure information
- [log](./inspect.md) - Show failure logs
- [status](./status.md) - View update session status
- [retry](./retry.md) - Retry failed updates
- [Debugging Use Case](../use-cases/debugging.md)
