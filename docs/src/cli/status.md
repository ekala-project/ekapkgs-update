# status - View Update Sessions

The `status` command displays the current and recent update sessions, showing progress, statistics, and overall health of the update process.

## Synopsis

```bash
ekapkgs-update status [OPTIONS]
```

## Description

View a summary of update sessions including:
- Currently running updates
- Recent completed sessions
- Success/failure statistics
- Package counts and timing
- Error summaries

## Options

#### `--database <PATH>` (short: `-d`)
Path to SQLite database.

**Default:** `~/.cache/ekapkgs-update/db.sqlite3`

```bash
ekapkgs-update status --database /var/lib/ekapkgs-update/db.sqlite3
```

## Output

### Active Session

If an update is currently running:

```
Current Update Session:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Session ID: 42
Started: 2024-05-19 10:00:00
Duration: 15m 32s
Status: Running

Progress:
  Completed: 45
  Failed: 3
  Running: 2
  Pending: 15

  Total: 65 packages

Current packages:
  - python312Packages.requests (Build phase)
  - nodejs (UpdateHash phase)

Recent failures:
  - terraform (HashMismatch)
  - gcc (BuildFailure)
  - rust (TestFailure)

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

### Recent Sessions

Shows last 5 completed sessions:

```
Recent Sessions:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Session #41 (2024-05-19 08:00:00)
  Duration: 45m 12s
  Status: Completed
  Success: 58 | Failed: 7 | Skipped: 2 | Total: 67

Session #40 (2024-05-18 20:00:00)
  Duration: 38m 45s
  Status: Completed
  Success: 62 | Failed: 3 | Skipped: 1 | Total: 66

Session #39 (2024-05-18 08:00:00)
  Duration: 42m 18s
  Status: Completed
  Success: 60 | Failed: 5 | Skipped: 0 | Total: 65

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

### No Active Sessions

When no update is running:

```
No active update sessions.

Recent Sessions:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Session #41 (2024-05-19 08:00:00)
  Duration: 45m 12s
  Status: Completed
  Success: 58 | Failed: 7 | Skipped: 2 | Total: 67

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## Use Cases

### Monitoring

```bash
# Check current progress
ekapkgs-update status

# Monitor in a loop
watch -n 30 ekapkgs-update status
```

### CI/CD

```bash
# Check if updates are running
if ekapkgs-update status | grep -q "Status: Running"; then
  echo "Update in progress, waiting..."
  sleep 60
fi
```

### Debugging

```bash
# Check recent session results
ekapkgs-update status

# Get more details on failures
ekapkgs-update query --since-days 1 --status failed
```

### Historical Analysis

```bash
# View session history
ekapkgs-update status

# Check success rate trends over time
# (combine with query for detailed analysis)
ekapkgs-update query --since-days 7 --group-by-error
```

## Session States

### Running
Update session is currently executing. Shows:
- Completed/failed/running/pending package counts
- Currently processing packages
- Recent failures
- Elapsed time

### Completed
Session finished successfully. Shows:
- Total duration
- Success/failure/skipped counts
- Final statistics

### Failed
Session terminated with errors. Shows:
- Error that caused termination
- Partial results
- Recovery options

## Integration with Other Commands

### Status -> Query

```bash
# Check status
ekapkgs-update status

# If failures shown, investigate
ekapkgs-update query --since-days 1 --status failed --group-by-error
```

### Status -> Inspect

```bash
# Check status to identify failed packages
ekapkgs-update status

# Inspect specific failure
ekapkgs-update inspect python312Packages.requests
```

### Status -> Retry

```bash
# View failed packages from recent session
ekapkgs-update status

# Retry specific package
ekapkgs-update retry terraform
```

## Examples

### Basic Usage

```bash
# View current status
ekapkgs-update status

# Use custom database
ekapkgs-update status --database /tmp/updates.db
```

### Monitoring Running Updates

```bash
# Watch status in real-time
watch -n 30 ekapkgs-update status

# Or use a simple loop
while ekapkgs-update status | grep -q "Running"; do
  echo "Still running..."
  sleep 60
done
echo "Update completed!"
```

### Check Last Run Results

```bash
# Quick status check
ekapkgs-update status | head -20

# Full status with recent history
ekapkgs-update status
```

### Automated Checks

```bash
#!/bin/bash
# Check if last session had failures

STATUS_OUTPUT=$(ekapkgs-update status)

if echo "$STATUS_OUTPUT" | grep -q "Failed: [1-9]"; then
  echo "Last update had failures!"
  ekapkgs-update query --since-days 1 --status failed
  exit 1
else
  echo "Last update successful!"
  exit 0
fi
```

### Session Comparison

```bash
# Save current status
ekapkgs-update status > before.txt

# Run updates
ekapkgs-update run

# Compare
ekapkgs-update status > after.txt
diff before.txt after.txt
```

## Performance Metrics

The status command shows:

### Timing
- **Started**: When the session began
- **Duration**: Total elapsed time
- **Average time per package**: Total duration / package count

### Success Rate
- **Success count**: Packages updated successfully
- **Failure count**: Packages that failed to update
- **Success rate**: (Success / Total) * 100%

### Progress
- **Completed**: Finished packages (success + failed)
- **Running**: Currently processing
- **Pending**: Queued for processing
- **Total**: All packages in session

## Exit Codes

- `0` - Status retrieved successfully
- `1` - Database error
- `2` - Invalid arguments

## See Also

- [run](./run.md) - Start batch updates
- [query](./query.md) - Search for specific failures
- [inspect](./inspect.md) - View detailed failure information
- [retry](./retry.md) - Retry failed updates
- [Batch Updates Use Case](../use-cases/batch-updates.md)
