# Batch Update Workflows

The `run` command automates updates for entire repositories, processing multiple packages concurrently.

## Basic Batch Updates

### Update All Packages

```bash
# Update all packages in default.nix
ekapkgs-update run

# Preview without making changes
ekapkgs-update run --dry-run

# Update specific file
ekapkgs-update run --file pkgs/default.nix
```

### Conservative Batch Update

```bash
# Skip unstable versions, limit rebuilds
ekapkgs-update run \
  --skip-unstable \
  --max-rebuilds 100 \
  --analyze-rebuilds
```

## Concurrency Control

### Adjust Parallelism

```bash
# Default: CPU cores / 4
ekapkgs-update run

# High parallelism (faster, more resources)
ekapkgs-update run --concurrent-updates 16

# Low parallelism (stable, easier debugging)
ekapkgs-update run --concurrent-updates 4

# Single-threaded (debugging)
ekapkgs-update run --concurrent-updates 1
```

### Interactive Mode

```bash
# Review each PR before submission
ekapkgs-update run --interactive
```

**Prompts for each update:**
- Show package, version change, and diff
- Options: `[s]ubmit`, `[e]dit`, `[sk]ip`, `[q]uit`
- Forces single-threaded execution

## Quality Control

### Testing

```bash
# Run passthru.tests for all packages
ekapkgs-update run --run-passthru-tests

# Combined with other checks
ekapkgs-update run \
  --run-passthru-tests \
  --analyze-rebuilds \
  --max-rebuilds 200
```

### Rebuild Analysis

```bash
# Analyze impact of each update
ekapkgs-update run --analyze-rebuilds

# Skip high-impact updates
ekapkgs-update run \
  --analyze-rebuilds \
  --max-rebuilds 50
```

### Filtering

```bash
# Skip unstable versions
ekapkgs-update run --skip-unstable

# Skip all optional checks (faster)
ekapkgs-update run \
  --skip-cve \
  --skip-repology \
  --skip-directory-diff \
  --skip-cachix
```

## Production Workflows

### Scheduled Updates

```bash
#!/bin/bash
# Daily update cron job

export GITHUB_TOKEN="ghp_..."
export CACHIX_AUTH_TOKEN="..."

ekapkgs-update run \
  --file /path/to/pkgs/default.nix \
  --database /var/lib/ekapkgs-update/db.sqlite3 \
  --upstream nixpkgs \
  --fork origin \
  --skip-unstable \
  --run-passthru-tests \
  --analyze-rebuilds \
  --max-rebuilds 100 \
  --cachix-cache my-cache \
  --concurrent-updates 8

# Check results
ekapkgs-update status --database /var/lib/ekapkgs-update/db.sqlite3
```

### Staged Rollout

```bash
# Stage 1: Low-impact updates only
ekapkgs-update run \
  --analyze-rebuilds \
  --max-rebuilds 10 \
  --concurrent-updates 4

# Stage 2: Medium-impact updates
ekapkgs-update run \
  --analyze-rebuilds \
  --max-rebuilds 50

# Stage 3: High-impact updates (manual review)
ekapkgs-update run \
  --analyze-rebuilds \
  --max-rebuilds 500 \
  --interactive
```

### CI Pipeline

```yaml
# .github/workflows/update.yml
name: Package Updates

on:
  schedule:
    - cron: '0 2 * * *'  # 2 AM daily
  workflow_dispatch:

jobs:
  update:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: DeterminateSystems/nix-installer-action@main

      - uses: DeterminateSystems/magic-nix-cache-action@main

      - name: Run updates
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          ekapkgs-update run \
            --dry-run \
            --skip-unstable \
            --max-rebuilds 100

      - name: Report results
        if: always()
        run: |
          ekapkgs-update status
          ekapkgs-update query --since-days 1 --group-by-error
```

## Failure Handling

### Preserve Failures for Debugging

```bash
# Keep failed worktrees
ekapkgs-update run --preserve-failures

# Review failures
ekapkgs-update worktrees list
ekapkgs-update query --since-days 1 --status failed

# Debug specific failure
ekapkgs-update inspect python312Packages.requests
ekapkgs-update retry python312Packages.requests
```

### Automatic Cleanup

```bash
# Run updates
ekapkgs-update run --preserve-failures

# Clean old failures weekly
ekapkgs-update worktrees clean --older-than 7
```

## Monitoring and Reporting

### Check Progress

```bash
# Monitor running updates
watch -n 30 ekapkgs-update status

# Query recent failures
ekapkgs-update query --since-days 1 --status failed

# Group by error type
ekapkgs-update query --since-days 7 --group-by-error
```

### Success Rate Analysis

```bash
# Check last session results
ekapkgs-update status

# Weekly report
ekapkgs-update query --since-days 7 --status success | wc -l
ekapkgs-update query --since-days 7 --status failed | wc -l

# Error patterns
ekapkgs-update query --since-days 7 --group-by-error
```

## Performance Optimization

### Fast Updates

```bash
# Skip optional checks
ekapkgs-update run \
  --skip-cve \
  --skip-repology \
  --skip-directory-diff \
  --skip-cachix \
  --concurrent-updates 16
```

### Resource Management

```bash
# Limit resource usage
ekapkgs-update run \
  --concurrent-updates 4 \
  --max-rebuilds 50

# Balance speed and stability
ekapkgs-update run \
  --concurrent-updates 8 \
  --skip-cve \
  --skip-repology
```

## Advanced Patterns

### Selective Updates

```bash
# Update only Python packages
# (requires filtering at nix level)
nix eval -f . --apply 'pkgs:
  builtins.attrNames (
    pkgs.lib.filterAttrs (n: v:
      pkgs.lib.hasPrefix "python" n
    ) pkgs
  )' | jq -r '.[]' | while read pkg; do
  ekapkgs-update update "$pkg" --commit
done
```

### Two-Phase Updates

```bash
# Phase 1: Dry-run and analyze
ekapkgs-update run --dry-run > updates-available.txt

# Review updates-available.txt

# Phase 2: Execute selected updates
ekapkgs-update run \
  --max-rebuilds 100 \
  --interactive
```

### Retry Failed Updates

```bash
# Initial run with preservation
ekapkgs-update run --preserve-failures

# Identify failures
ekapkgs-update query --since-days 1 --status failed

# Retry each failure
ekapkgs-update query --since-days 1 --status failed | \
  grep "Package:" | awk '{print $2}' | \
  while read pkg; do
    ekapkgs-update retry "$pkg"
  done
```

## Best Practices

### Start Conservative

```bash
# First run: dry-run with limits
ekapkgs-update run \
  --dry-run \
  --skip-unstable \
  --max-rebuilds 50

# Second run: actual updates
ekapkgs-update run \
  --skip-unstable \
  --max-rebuilds 50 \
  --concurrent-updates 4
```

### Gradual Rollout

```bash
# Day 1: Preview
ekapkgs-update run --dry-run

# Day 2: Low-impact only
ekapkgs-update run --max-rebuilds 10

# Day 3: Medium-impact
ekapkgs-update run --max-rebuilds 100

# Day 4: Manual review for rest
ekapkgs-update run --interactive
```

### Database Maintenance

```bash
# Regular cleanup
ekapkgs-update worktrees clean --older-than 7

# Backup database before major runs
cp ~/.cache/ekapkgs-update/db.sqlite3{,.backup}

# Archive old sessions (manual SQL)
sqlite3 ~/.cache/ekapkgs-update/db.sqlite3 \
  "DELETE FROM update_sessions WHERE created_at < datetime('now', '-30 days')"
```

## See Also

- [run command](../cli/run.md) - Full command reference
- [status command](../cli/status.md) - Monitor progress
- [query command](../cli/query.md) - Analyze results
- [CI/CD Integration](./ci-cd.md) - Automation examples
