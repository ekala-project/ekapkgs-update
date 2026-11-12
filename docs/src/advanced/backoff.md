# Backoff Strategy for Failed Package Checks

ekapkgs-update implements an exponential backoff system to manage how frequently packages are checked for updates after failures. This prevents excessive API traffic while ensuring packages are eventually re-checked.

## Overview

When a package check fails to find an update, the system calculates a "next_attempt" timestamp using exponential backoff:

- **First failure**: Re-check in 2 days
- **Second failure** (recent retry): Re-check in 4 days
- **Subsequent failures**: Re-check in 6 days (maximum)

This prevents the system from repeatedly querying the same package when upstream hasn't released a new version, while still allowing updates to be discovered.

## Backoff Schedule

### Backoff Constants

The system uses three backoff durations (all measured in days):

| Failure | Backoff | Duration | Purpose |
|---------|---------|----------|---------|
| First | `FIRST_BACKOFF_DAYS` | 2 days | Initial throttling |
| Second | `SECOND_BACKOFF_DAYS` | 4 days | Escalated throttling |
| Subsequent | `MAX_BACKOFF_DAYS` | 6 days | Maximum throttling |

### Backoff Calculation

The backoff duration is determined by examining the `last_attempted` timestamp:

```
if last_attempted is NULL:
    # First time checking this package
    backoff = 2 days

else if (now - last_attempted) <= 2 days:
    # Last attempt was recent (within 2 days)
    # Package is consistently having no updates
    backoff = 4 days

else:
    # Last attempt was long ago (> 2 days)
    # Increase backoff to maximum to reduce noise
    backoff = 6 days
```

### Timeline Visualization

**Package never checked before:**
```
Time: ──────────────────────────────────────────
      Day 0                              Day 2
      [Check #1]                         [Check #2 scheduled]
      (no update found)                  (2 day backoff)

      Result: next_attempt = now + 2 days
```

**Checked frequently with no updates:**
```
Time: ──────────────────────────────────────────────────────────
      Day 0        Day 2         Day 6                    Day 10
      [Check #1]   [Check #2]    [Check #3]              [Check #4]
      (no update)  (no update)   (no update)             (no update)
      +2d          +4d           +6d                     +6d
                                                         (backoff maxed out)

      Pattern: 2 → 4 → 6 → 6 → 6 → ... (plateaus at 6 days)
```

**Interleaved with successful updates:**
```
Time: ──────────────────────────────────────────────────────────
      Day 0        Day 2                  Day 4
      [Check #1]   [Check #2]            [Check #3]
      (no update)  (update found!)       (no update)
      +2d          reset to +2d          +2d
                   (success resets)

      Result: Success always resets backoff to 2 days
```

## Implementation Details

Backoff state lives in the `updates` table (`next_attempt`, `last_checked`,
`last_attempted`, `failure_count`, `failed_version`). The table is scoped per
repository (`repo_name`; `''` for `run`). The constants are in
`ekapkgs-update/src/database/mod.rs`.

There are two kinds of backoff.

### Check backoff (nothing to do)

`record_no_update()` is called when the checker finds nothing to dispatch. That
happens when the package is already up to date, the version was already
proposed, or upstream could not be queried. The next check is scheduled from
the time since the **previous check**:

```
if never checked:                    backoff = 2 days
elif now - last_checked <= 2 days:   backoff = 4 days
else:                                backoff = 6 days
```

### Failure backoff (an attempt failed)

`record_failed_update()` increments `failure_count`, records the target
version in `failed_version` and schedules the next attempt:

| `failure_count` | Backoff |
|-----------------|---------|
| 1 | 2 days |
| 2 | 4 days |
| 3+ | 6 days |

The package's `status` becomes `failed` and `last_error` holds a one-line
summary. The full log goes to `update_logs`.

### Success reset

`record_successful_update()` sets `status = 'proposed'` and
`proposed_version`, resets `failure_count` to 0, and schedules the next check
in 2 days.

### Watch mode

The `watch` poller reads cheap feeds on every poll, so the check backoff does
not apply to it. It respects the failure backoff only for the **version that
failed**:

- the same release is not retried until `next_attempt`
- a newer release is dispatched immediately

See [watch](../cli/watch.md).

## Practical Examples

### Example 1: Simple Package

A package that receives updates regularly:

```
Session 1: 2024-05-15 08:00
  Check python312Packages.requests (2.31.0)
  → Latest upstream: 2.31.0 (no update)
  → Schedule next: 2024-05-17 08:00 (+2 days)

Session 2: 2024-05-17 08:00
  Check python312Packages.requests (2.31.0)
  → Latest upstream: 2.32.0 (update available!)
  → Create PR, mark successful
  → Schedule next: 2024-05-19 08:00 (+2 days)

Session 3: 2024-05-19 08:00
  Check python312Packages.requests (2.32.0) [updated from PR]
  → Latest upstream: 2.32.0 (no update)
  → Schedule next: 2024-05-21 08:00 (+2 days)
```

### Example 2: Rarely-Updated Package

A package that consistently has no updates:

```
Session 1: 2024-05-15 08:00
  Check unmaintained-old-tool (1.0.0)
  → Latest upstream: 1.0.0 (no update)
  → Days since attempt: None (first time)
  → Schedule next: 2024-05-17 08:00 (+2 days)

Session 2: 2024-05-17 08:00
  Check unmaintained-old-tool (1.0.0)
  → Latest upstream: 1.0.0 (no update)
  → Days since attempt: 2 days (last_attempted was 2024-05-15)
  → Decision: 2 ≤ 2 days? YES → use SECOND_BACKOFF
  → Schedule next: 2024-05-21 08:00 (+4 days)

Session 3: 2024-05-21 08:00
  Check unmaintained-old-tool (1.0.0)
  → Latest upstream: 1.0.0 (no update)
  → Days since attempt: 4 days (last_attempted was 2024-05-17)
  → Decision: 4 ≤ 2 days? NO → use MAX_BACKOFF
  → Schedule next: 2024-05-27 08:00 (+6 days)

Session 4: 2024-05-27 08:00 and beyond
  (Backoff remains at 6 days until an update is found)
```

### Example 3: Failed and Retried

A package whose update fails to build:

```
Session 1: 2024-05-15
  Check my-package (1.0.0) → latest upstream 2.0.0
  → Build fails → status 'failed', failure_count 1, failed_version 2.0.0
  → Next attempt: 2024-05-17 (+2 days)

Session 2: 2024-05-16
  my-package is in backoff → skipped (counted as "backoff" in the session)

Session 3: 2024-05-17
  Check my-package (1.0.0) → latest upstream 2.0.0
  → Build fails again → failure_count 2
  → Next attempt: 2024-05-21 (+4 days)

Session 4: 2024-05-21
  Check my-package (1.0.0) → latest upstream 2.0.1
  → Build succeeds → status 'proposed', failure_count reset to 0
  → Next check: 2024-05-23 (+2 days)
```

With `watch`, the 2.0.1 release would have been dispatched as soon as it
appeared, without waiting for the 2.0.0 backoff to expire.

## Impact on Resource Usage

The backoff system significantly reduces API traffic and system load:

### Without Backoff (hypothetical)

```
200 packages × 4 runs/day = 800 API queries/day
Upstream rate limiting: 60 req/min = 86,400 req/day
Impact: 0.9% of limit
```

### With Backoff (actual)

```
200 packages, averaged:
  - 50% updated in 2 days (always checked)
  - 30% not updated, backoff to 4 days
  - 20% not updated, backoff to 6 days
Average backoff: ~3.4 days

Effective queries: 200 / 3.4 = 59 packages/day × 4 runs
= ~236 API queries/day
Impact: 0.3% of limit
```

**Benefit**: 70% reduction in API load while maintaining update discovery.

## Tuning the Backoff

The backoff constants can be adjusted in the source code if needed:

```rust
// ekapkgs-update/src/database/mod.rs
const FIRST_BACKOFF_DAYS: i64 = 2;    // Modify to change initial backoff
const SECOND_BACKOFF_DAYS: i64 = 4;   // Modify to change retry backoff
const MAX_BACKOFF_DAYS: i64 = 6;      // Modify to change maximum backoff
```

### Tuning Scenarios

**More aggressive (find updates faster):**
```rust
const FIRST_BACKOFF_DAYS: i64 = 1;
const SECOND_BACKOFF_DAYS: i64 = 2;
const MAX_BACKOFF_DAYS: i64 = 4;
```
- Pros: Find updates faster, more responsive
- Cons: Higher API load, more frequent false positives

**More conservative (reduce API load):**
```rust
const FIRST_BACKOFF_DAYS: i64 = 3;
const SECOND_BACKOFF_DAYS: i64 = 7;
const MAX_BACKOFF_DAYS: i64 = 14;
```
- Pros: Minimal API load, fewer queries
- Cons: Delayed update discovery, less responsive

## Integration with Other Features

### Rebuild Analysis

Rebuild analysis can trigger successful updates, resetting backoff:

```
Package: rustc (high rebuild impact)
Session 1: Check → no update → 2 day backoff
Session 2: Check → 1.80.0 available → analyze rebuilds (50+ rebuilds)
          → Skip due to max-rebuilds=30 → mark as "no update" → 4 day backoff
Session 3: Check → 1.80.0 still available → same process → 6 day backoff
          → Eventually, backoff reaches max (6 days)
```

### CVE Analysis

CVE findings don't affect backoff, but may influence prioritization:

```
Package: vulnerable-library
Session 1: Check → 2.0.0 available
          → CVE analysis: Resolves CVE-2024-1234
          → Record successful update → 2 day backoff (for next version)
```

### Max-Rebuilds Filter

The `max-rebuilds` option can interact with backoff:

```
If update would cause too many rebuilds:
  → Update is skipped
  → Treated as "no update found"
  → Standard backoff applies (2 → 4 → 6 days)
```

## Monitoring Backoff State

Query the database to see current backoff status:

```sql
SELECT
    repo_name,
    attr_path,
    status,
    current_version,
    failure_count,
    next_attempt,
    CAST((julianday(next_attempt) - julianday('now')) AS INT) as days_until_check
FROM updates
WHERE next_attempt > datetime('now')
ORDER BY days_until_check DESC
LIMIT 20;
```

**Output:**
```
attr_path                          current_version  days_until_check
─────────────────────────────────  ───────────────  ────────────────
unmaintained-package-1             1.0.0            5
unmaintained-package-2             2.1.0            4
rarely-updated-lib                 1.2.3            2
actively-developed-pkg             5.0.0            1
```

This shows which packages are coming up for re-check soon.

## Best Practices

1. **Trust the backoff**: Don't override it manually unless necessary
2. **Monitor backoff trends**: High backoff indicates stale packages
3. **Adjust for your workflow**: If updates lag, use more aggressive backoff
4. **Reset strategically**: After major maintenance, may need manual reset
5. **Consider rebuild impact**: Use `max-rebuilds` to prevent cascading failures
6. **Track manually skipped updates**: Note why updates were skipped for later

## Troubleshooting

### Package never gets updated

Check the backoff state:

```bash
ekapkgs-update query python312Packages.stale-pkg
# If next_attempt is far in future, backoff is active
```

**Solution**: Check if an update is actually available:

```bash
ekapkgs-update query python312Packages.stale-pkg --verbose
# Shows: current_version, latest_upstream, why backoff is applied
```

### Backoff too aggressive

Reduce the backoff constants (in source) or run more frequently:

```bash
# Run 8 times/day instead of 4 to compress backoff schedule
0 */3 * * * ekapkgs-update run  # Every 3 hours
```

### Backoff too lenient

Increase the backoff constants (in source) or accept delayed updates:

```rust
const FIRST_BACKOFF_DAYS: i64 = 3;
const SECOND_BACKOFF_DAYS: i64 = 7;
const MAX_BACKOFF_DAYS: i64 = 14;
```

## Related Topics

- [Database Schema](./database.md) - `next_attempt` field storage
- [Failure Preservation](./failure-preservation.md) - Failed updates and backoff
- [PR Enhancements](./pr-enhancements.md) - `max-rebuilds` affects backoff
- [Quick Start](../quick-start.md) - Default backoff behavior
