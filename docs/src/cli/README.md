# Command-Line Interface

ekapkgs-update provides a comprehensive CLI with commands for updating packages, querying failures, managing worktrees, and more.

## Quick Reference

### Core Commands

| Command | Purpose | Common Use |
|---------|---------|------------|
| [`run`](./run.md) | Automated batch updates | `ekapkgs-update run --file default.nix` |
| [`update`](./update.md) | Update single package | `ekapkgs-update update hello` |
| [`status`](./status.md) | View session status | `ekapkgs-update status` |
| [`query`](./query.md) | Search failures | `ekapkgs-update query --since-days 7` |
| [`inspect`](./inspect.md) | View failure details | `ekapkgs-update inspect python312Packages.foo` |

### Debugging & Retry

| Command | Purpose | Common Use |
|---------|---------|------------|
| [`log`](./inspect.md) | Show failure logs | `ekapkgs-update log mypackage` |
| [`retry`](./retry.md) | Retry failed update | `ekapkgs-update retry mypackage` |
| [`worktrees`](./worktrees.md) | Manage artifacts | `ekapkgs-update worktrees list` |
| [`export`](./export-apply.md) | Export for LLM | `ekapkgs-update export mypackage` |
| [`apply`](./export-apply.md) | Apply LLM fixes | `ekapkgs-update apply mypackage --patch fix.patch` |

### Utilities

| Command | Purpose | Common Use |
|---------|---------|------------|
| [`migrate`](./migrate.md) | Migrate to ekapkgs | `ekapkgs-update migrate mypackage` |
| `prune-maintainers` | Remove maintainers | `ekapkgs-update prune-maintainers ./pkgs` |

## Global Options

All commands support these global options:

### `--color <WHEN>`
Control color output:
- `auto` - Color when stdout is a terminal (default)
- `always` - Always use color
- `never` - Never use color

```bash
ekapkgs-update --color never run --dry-run
```

## Common Patterns

### Dry-run Before Actual Update

```bash
# Preview what would be updated
ekapkgs-update run --dry-run --max-rebuilds 10

# Actually do it
ekapkgs-update run --max-rebuilds 10
```

### Update with Tests

```bash
# Single package
ekapkgs-update update gcc --run-passthru-tests

# Batch mode
ekapkgs-update run --run-passthru-tests
```

### Debug Failed Update

```bash
# 1. Check logs
ekapkgs-update log mypackage

# 2. Inspect full context
ekapkgs-update inspect mypackage

# 3. View preserved worktree
ekapkgs-update worktrees show mypackage

# 4. Export for AI analysis
ekapkgs-update export mypackage --format markdown

# 5. Apply fix and retry
ekapkgs-update apply mypackage --patch fix.patch --resume
```

### Query and Filter Failures

```bash
# Recent failures
ekapkgs-update query --since-days 7

# Group by error type
ekapkgs-update query --group-by-error

# Specific error type
ekapkgs-update query --error-type "HashMismatch"

# Filter by package pattern
ekapkgs-update query --package "python%"
```

## Environment Variables

Several commands respect environment variables:

- `GITHUB_TOKEN` - GitHub API token (higher rate limits, PR creation)
- `GITLAB_TOKEN` - GitLab API token
- `SOURCEHUT_TOKEN` - SourceHut API token
- `CACHIX_AUTH_TOKEN` - Cachix authentication token
- `CACHIX_CACHE_NAME` - Default Cachix cache name
- `RUST_LOG` - Logging level (`error`, `warn`, `info`, `debug`, `trace`)

```bash
export RUST_LOG=debug
export GITHUB_TOKEN="ghp_xxxxxxxxxxxx"
ekapkgs-update run --file default.nix
```

## Getting Help

Every command has detailed help:

```bash
# Top-level help
ekapkgs-update --help

# Command-specific help
ekapkgs-update run --help
ekapkgs-update update --help
ekapkgs-update query --help
```

## Exit Codes

ekapkgs-update uses standard Unix exit codes:

- `0` - Success
- `1` - General error
- `2` - Command-line argument error

For `run` command:
- `0` - All updates succeeded
- `1` - One or more updates failed

Check exit code in scripts:
```bash
if ekapkgs-update run --dry-run; then
    echo "Looks good, running for real"
    ekapkgs-update run
else
    echo "Dry-run found issues"
    exit 1
fi
```

## Next Steps

Explore detailed documentation for each command:

- [run - Automated Updates](./run.md) - Batch update entire repositories
- [update - Single Package](./update.md) - Update one package at a time
- [query - Search Failures](./query.md) - Find and analyze failures
- [inspect - Failure Details](./inspect.md) - Deep-dive into failure context
