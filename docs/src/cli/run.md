# run - Automated Batch Updates

The `run` command automates the process of updating multiple packages in a repository. It evaluates a Nix file, identifies updatable packages, and processes them concurrently.

## Synopsis

```bash
ekapkgs-update run [OPTIONS]
```

## Description

The `run` command is designed for automated, unattended batch updates of package repositories. It:

1. Evaluates the specified Nix file to discover all packages
2. Filters out packages marked with `passthru.ekapkgs-update.skip`
3. Checks for available updates using VCS sources (GitHub, GitLab, PyPI, etc.)
4. Updates packages concurrently with configurable parallelism
5. Optionally runs tests, analyzes rebuilds, and creates pull requests
6. Tracks all updates in a SQLite database for debugging and retry

## Options

### File and Database

#### `--file <FILE>` (short: `-f`)
Nix file to evaluate for discovering packages.

**Default:** `default.nix`

```bash
# Use custom entry point
ekapkgs-update run --file ./pkgs/default.nix

# Common in flake-based repos
ekapkgs-update run --file flake.nix
```

#### `--database <PATH>` (short: `-d`)
Path to SQLite database for tracking update sessions and failures.

**Default:** `~/.cache/ekapkgs-update/db.sqlite3`

```bash
ekapkgs-update run --database /var/lib/ekapkgs-update/db.sqlite3
```

### Git Configuration

#### `--upstream <REMOTE>`
Upstream git remote name. Used as the base branch for pull requests.

**Default:** Auto-detected from git config (typically `upstream` or `nixpkgs`)

```bash
# Explicitly set upstream
ekapkgs-update run --upstream nixpkgs

# PRs will be created against upstream/master
```

#### `--fork <REMOTE>`
Remote repository to push update branches to.

**Default:** `origin`

```bash
# Push to personal fork
ekapkgs-update run --fork my-fork
```

**Note:** The fork remote must be configured with push permissions. This is where update branches will be pushed before creating PRs.

### Testing and Validation

#### `--run-passthru-tests`
Run `passthru.tests` if available before considering an update successful.

```bash
ekapkgs-update run --run-passthru-tests
```

When enabled:
- Packages with `passthru.tests` will have their tests executed
- Update only succeeds if all tests pass
- Test results are recorded in the database
- Failed tests preserve the worktree for debugging

**Example package with tests:**
```nix
{
  mypackage = pkgs.buildPythonPackage {
    # ... package definition ...

    passthru.tests = {
      pytest = pkgs.runCommand "mypackage-pytest" {} ''
        ${mypackage}/bin/mypackage --version
        touch $out
      '';
    };
  };
}
```

### Dry Run and Preview

#### `--dry-run`
Check for updates without rewriting files, building, committing, or creating PRs.

```bash
ekapkgs-update run --dry-run
```

Useful for:
- Previewing available updates
- Checking rate limits
- Validating configuration
- CI pipelines that only report available updates

**Output example:**
```
Found 15 packages with updates available:
  - python312Packages.requests: 2.31.0 -> 2.32.0
  - nodejs: 20.10.0 -> 20.11.0
  - terraform: 1.6.0 -> 1.7.0

Dry-run mode: no changes were made
```

### Concurrency Control

#### `--concurrent-updates <N>`
Maximum number of packages to update concurrently.

**Default:** CPU cores / 4 (minimum 1)

```bash
# Update 8 packages at once
ekapkgs-update run --concurrent-updates 8

# Single-threaded (useful for debugging)
ekapkgs-update run --concurrent-updates 1
```

**Considerations:**
- Higher values = faster but more memory/CPU usage
- Lower values = slower but more stable
- Set to 1 when using `--interactive`

### Filtering and Skipping

#### `--skip-unstable`
Skip packages with 'unstable' in their version string.

```bash
ekapkgs-update run --skip-unstable
```

Skips versions like:
- `1.2.3-unstable-2024-01-01`
- `unstable-2024-01-01`
- `20240101-unstable`

Useful for:
- Production deployments
- Stable release channels
- Avoiding nightly/alpha/beta versions

#### `--skip-cve`
Skip CVE vulnerability checking via OSV.dev.

```bash
ekapkgs-update run --skip-cve
```

By default, ekapkgs-update checks for known CVEs. This option disables that check to speed up updates.

#### `--skip-repology`
Skip Repology cross-distribution version checking.

```bash
ekapkgs-update run --skip-repology
```

Repology helps detect if you're updating to an outdated version. Skipping it speeds up the process but may miss version discrepancies.

### Rebuild Analysis

#### `--analyze-rebuilds`
Analyze and report rebuild counts for each update.

```bash
ekapkgs-update run --analyze-rebuilds
```

For each update:
1. Runs `nix-diff` to count affected packages
2. Reports rebuild count in PR description
3. Records metrics in database

**PR output example:**
```markdown
## Rebuild Analysis

This update would rebuild:
- 3 direct dependencies
- 127 total packages

Impact: medium
```

#### `--max-rebuilds <N>`
Skip updates that would cause more than N package rebuilds.

```bash
# Only accept updates that rebuild fewer than 100 packages
ekapkgs-update run --analyze-rebuilds --max-rebuilds 100
```

**Requires:** `--analyze-rebuilds`

Use cases:
- Minimize CI load
- Avoid large rebuild cascades
- Time-sensitive update windows
- Resource-constrained environments

### Pull Request Options

#### `--skip-directory-diff`
Skip generating directory structure diffs in PR body.

```bash
ekapkgs-update run --skip-directory-diff
```

By default, PRs include before/after directory listings. This option disables that feature to speed up PR creation.

#### `--interactive`
Interactive mode: prompt before submitting each PR with summary and commit info.

```bash
ekapkgs-update run --interactive
```

**Behavior:**
- Forces single-threaded execution (`--concurrent-updates 1`)
- Shows PR summary, commit, and diff before submission
- Prompts: `[s]ubmit / [e]dit / [sk]ip / [q]uit`
- Allows manual review of each update

**Example interaction:**
```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Package: python312Packages.requests
Version: 2.31.0 -> 2.32.0
Branch: update/python312Packages.requests-2.32.0

Commit message:
python312Packages.requests: 2.31.0 -> 2.32.0

Files changed: 1
 pkgs/python-modules/requests/default.nix | 4 ++--

[s]ubmit / [e]dit / [sk]ip / [q]uit?
```

### Cachix Integration

#### `--skip-cachix`
Skip pushing build outputs to Cachix.

```bash
ekapkgs-update run --skip-cachix
```

#### `--cachix-cache <NAME>`
Cachix cache name to push successful builds to.

**Environment variable:** `CACHIX_CACHE_NAME`
**Requires:** `CACHIX_AUTH_TOKEN` environment variable

```bash
export CACHIX_AUTH_TOKEN="eyJhbG..."
ekapkgs-update run --cachix-cache my-cache
```

Benefits:
- Pre-populate cache for users
- Speed up CI builds
- Share build artifacts across machines

### Failure Preservation

#### `--preserve-failures`
Preserve failed worktrees and artifacts for later inspection.

```bash
ekapkgs-update run --preserve-failures
```

When enabled:
- Failed update worktrees are preserved in `/tmp/ekapkgs-update-worktrees/`
- Includes modified files, build logs, and context
- Allows retry with `ekapkgs-update retry`
- Enables LLM-assisted debugging with `ekapkgs-update export/apply`

**Cleanup:**
```bash
# List preserved failures
ekapkgs-update worktrees list

# Clean up old failures (older than 7 days)
ekapkgs-update worktrees clean --older-than 7
```

## Examples

### Basic Usage

```bash
# Update all packages in default.nix
ekapkgs-update run

# Dry-run to see what would be updated
ekapkgs-update run --dry-run
```

### Production Pipeline

```bash
# Conservative production updates
ekapkgs-update run \
  --skip-unstable \
  --run-passthru-tests \
  --max-rebuilds 50 \
  --analyze-rebuilds \
  --cachix-cache production
```

### CI/CD Integration

```bash
# GitHub Actions: report available updates
ekapkgs-update run \
  --dry-run \
  --skip-cve \
  --skip-repology
```

### Interactive Review

```bash
# Review each update before submitting
ekapkgs-update run \
  --interactive \
  --run-passthru-tests \
  --analyze-rebuilds
```

### Debugging Setup

```bash
# Preserve failures for later investigation
ekapkgs-update run \
  --concurrent-updates 1 \
  --preserve-failures \
  --run-passthru-tests
```

### Lightweight Updates

```bash
# Fast updates without extra checks
ekapkgs-update run \
  --skip-cve \
  --skip-repology \
  --skip-cachix \
  --skip-directory-diff
```

## Workflow

The `run` command follows this workflow for each package:

1. **Discovery**: Evaluate Nix file to find all packages
2. **Filtering**: Skip packages with `passthru.ekapkgs-update.skip = true`
3. **Version Check**: Query VCS source for latest compatible version
4. **Update**: Rewrite Nix file with new version and hashes
5. **Build**: Build package to verify correctness
6. **Test** (optional): Run `passthru.tests` if `--run-passthru-tests`
7. **Rebuild Analysis** (optional): Count affected packages if `--analyze-rebuilds`
8. **Commit**: Create git commit with standardized message
9. **PR**: Push branch and create pull request
10. **Cachix** (optional): Push build outputs if configured

## Database Tracking

All updates are tracked in the SQLite database:

```bash
# View recent update sessions
ekapkgs-update status

# Query failures from last 7 days
ekapkgs-update query --since-days 7

# View specific failure details
ekapkgs-update inspect python312Packages.requests
```

## Exit Codes

- `0` - All updates succeeded
- `1` - One or more updates failed (check logs with `query` or `log`)
- `2` - Invalid arguments

## See Also

- [update](./update.md) - Update a single package
- [status](./status.md) - View update session status
- [query](./query.md) - Search for failures
- [retry](./retry.md) - Retry failed updates
- [Batch Updates Use Case](../use-cases/batch-updates.md)
