# Daemon Mode

Daemon mode provides automated, continuous package updates for large package sets. It's designed for repository maintenance and CI/CD integration.

## When to Use Daemon Mode

Daemon mode is ideal for:

- **Large package repositories** - Maintaining hundreds or thousands of packages
- **Automated workflows** - CI/CD pipelines that automatically update packages
- **Regular maintenance** - Daily/weekly automated update checks
- **Package discovery** - Finding all outdated packages at once
- **Batch operations** - Updating many packages efficiently

For updating individual packages, consider [Manual Updates](./manual-updates.md) instead.

## Basic Workflow

### Simple Daemon Run

Check all packages for updates and apply them:

```bash
ekapkgs-update run --file ./default.nix
```

**What happens:**

1. **Discovery** - All packages in `default.nix` are enumerated
2. **Check** - Each package is checked for updates
3. **Filter** - Packages with `skip = true` are skipped
4. **Update** - Packages with updates are updated sequentially or in parallel
5. **Track** - Update history saved to SQLite database

**Output example:**

```
INFO Found 150 packages to check
INFO Checking mypackage (1/150)
INFO mypackage: 1.2.3 -> 1.3.0 (update available)
INFO Updating mypackage
INFO mypackage: Update successful
INFO Checking anotherpackage (2/150)
INFO anotherpackage: 2.0.0 (already up to date)
...
INFO Summary: 150 checked, 12 updated, 138 up-to-date, 0 failed
```

### Dry Run Mode

Preview what would be updated without making changes:

```bash
ekapkgs-update run --dry-run --file ./default.nix
```

**Shows:**

- Packages with available updates
- Version changes that would occur
- Packages that would be skipped

**Does NOT:**

- Modify any files
- Build packages
- Create commits or PRs
- Update the database

**Use cases:**

- Testing configuration before automation
- Seeing what's outdated
- Validating API tokens and connectivity
- CI checks without side effects

### With Automatic PRs

Automatically create pull requests for updates:

```bash
ekapkgs-update run \
  --file ./default.nix \
  --create-pr \
  --upstream nixpkgs \
  --fork origin
```

**What happens:**

Each successfully updated package gets:
1. A Git commit
2. A branch pushed to your fork
3. A pull request created on upstream

**Requirements:**

- `gh` CLI tool installed and authenticated
- Git remotes properly configured
- Write access to fork repository

## Database Tracking

Daemon mode uses SQLite to track update history and prevent duplicate work.

### Default Database Location

```bash
~/.cache/ekapkgs-update/updates.db
```

The database stores:
- Last check timestamp for each package
- Last successful update timestamp
- Failed update attempts and error logs
- Version history

### Custom Database Location

```bash
ekapkgs-update run --database ./my-updates.db
```

**Use cases:**

- Per-repository tracking
- Sharing database across CI runs
- Persistent storage in Docker containers

### Database Benefits

- **Deduplication** - Avoid checking same package repeatedly
- **Failure tracking** - Record and review failed updates
- **History** - Audit trail of all updates
- **Rate limiting** - Avoid hammering upstream APIs

### Viewing Update Logs

```bash
ekapkgs-update log mypackage
```

Shows recent failed update attempts with timestamps and errors.

## Concurrency Control

### Default Concurrency

By default, daemon mode updates `CPU cores / 4` packages concurrently.

On an 8-core machine: 2 concurrent updates

### Custom Concurrency

```bash
# Update 8 packages at once
ekapkgs-update run --concurrent-updates 8

# Sequential updates (no parallelism)
ekapkgs-update run --concurrent-updates 1
```

**Considerations:**

- **Higher concurrency** = faster but more resource intensive
- **Lower concurrency** = slower but more stable
- **Network** - API rate limits may restrict high concurrency
- **Build resources** - Nix builds consume CPU/RAM/disk

**Recommended values:**

- Small machines (2-4 cores): 1-2
- Medium machines (8 cores): 2-4
- Large machines (16+ cores): 4-8
- CI environments: 2-4 (conservative)

## Filtering Packages

### Skip Unstable Versions

Ignore packages with "unstable" in their version:

```bash
ekapkgs-update run --skip-unstable
```

**Skips:**

- `2.0.0-unstable-2024-01-15`
- `1.5.0-unstable`
- `3.0.0-alpha-unstable`

**Useful for:**

- Avoiding rolling release channels
- Focusing on stable releases only
- Reducing noise in large package sets

### Skip via Passthru Attributes

Packages can opt out individually:

```nix
passthru.ekapkgs-update.skip = true;
```

See [Skip Attribute](../passthru-attributes/skip.md) for details.

### Skip by Name (Custom Script)

For more complex filtering, wrap ekapkgs-update:

```bash
#!/bin/bash
for pkg in $(nix-instantiate --eval --expr '...' | jq -r '.[]'); do
  if [[ "$pkg" != "blacklisted-package" ]]; then
    ekapkgs-update update "$pkg" --file ./default.nix
  fi
done
```

## Rebuild Analysis

### Analyze Rebuild Impact

Show how many packages would rebuild for each update:

```bash
ekapkgs-update run --analyze-rebuilds
```

**Output:**

```
INFO mypackage: 1.2.3 -> 1.3.0 (45 rebuilds)
INFO anotherpackage: 2.0.0 -> 2.1.0 (3 rebuilds)
```

**Note:** This is expensive as it requires evaluating the package graph.

### Limit Rebuilds

Skip updates that would cause too many rebuilds:

```bash
ekapkgs-update run --max-rebuilds 100
```

**Example:**

If updating `openssl` would rebuild 5,000 packages, it's skipped.

**Use cases:**

- Avoiding massive CI build queues
- Focusing on low-impact updates
- Gradual update rollout

## PR Enhancements

Daemon mode can automatically enhance pull requests with additional information.

### CVE Checking

Automatically check for CVEs (Common Vulnerabilities and Exposures):

```bash
ekapkgs-update run --create-pr
# CVE checking is enabled by default
```

PR description includes:

```markdown
## Security

No known CVEs fixed in this update.
```

or

```markdown
## Security

This update fixes:
- CVE-2024-1234: Remote code execution vulnerability
- CVE-2024-5678: Denial of service vulnerability
```

**Disable:**

```bash
ekapkgs-update run --create-pr --skip-cve
```

### Repology Integration

Cross-check versions across Linux distributions:

```bash
ekapkgs-update run --create-pr
# Repology checking is enabled by default
```

PR description shows:

```markdown
## Repology

- Arch Linux: 1.3.0
- Debian: 1.2.5
- Fedora: 1.3.0
- Ubuntu: 1.2.3
```

**Disable:**

```bash
ekapkgs-update run --create-pr --skip-repology
```

### Directory Diff

Show package size changes:

```bash
ekapkgs-update run --create-pr
# Directory diff is enabled by default
```

PR description includes:

```markdown
## Package Size

Before: 45.2 MB
After: 46.1 MB
Change: +900 KB (+2.0%)
```

**Disable:**

```bash
ekapkgs-update run --create-pr --skip-directory-diff
```

## Cachix Integration

Push successful builds to Cachix for faster CI:

```bash
export CACHIX_AUTH_TOKEN="your-token"
export CACHIX_CACHE_NAME="my-cache"

ekapkgs-update run --cachix-cache my-cache
```

or

```bash
ekapkgs-update run --cachix-cache my-cache
# Uses CACHIX_CACHE_NAME env var
```

**What happens:**

1. Package is updated
2. Build succeeds
3. Build output pushed to Cachix
4. Subsequent builds/CI can use cached result

**Disable:**

```bash
ekapkgs-update run --skip-cachix
```

## CI/CD Integration

### GitHub Actions

```yaml
name: Update Packages

on:
  schedule:
    - cron: '0 0 * * *'  # Daily at midnight
  workflow_dispatch:

jobs:
  update:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - uses: cachix/install-nix-action@v20
        with:
          nix_path: nixpkgs=channel:nixos-unstable

      - uses: cachix/cachix-action@v12
        with:
          name: my-cache
          authToken: '${{ secrets.CACHIX_AUTH_TOKEN }}'

      - name: Install ekapkgs-update
        run: nix-env -iA nixpkgs.ekapkgs-update

      - name: Install gh CLI
        run: nix-env -iA nixpkgs.gh

      - name: Configure git
        run: |
          git config user.name "ekapkgs-bot"
          git config user.email "bot@example.com"

      - name: Run updates
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CACHIX_AUTH_TOKEN: ${{ secrets.CACHIX_AUTH_TOKEN }}
        run: |
          ekapkgs-update run \
            --file ./default.nix \
            --create-pr \
            --upstream upstream \
            --fork origin \
            --concurrent-updates 4 \
            --max-rebuilds 100 \
            --cachix-cache my-cache
```

### GitLab CI

```yaml
update-packages:
  image: nixos/nix:latest
  script:
    - nix-env -iA nixpkgs.ekapkgs-update
    - nix-env -iA nixpkgs.gh
    - git config user.name "ekapkgs-bot"
    - git config user.email "bot@example.com"
    - |
      ekapkgs-update run \
        --file ./default.nix \
        --create-pr \
        --concurrent-updates 4
  variables:
    GITHUB_TOKEN: $GITHUB_TOKEN
    GITLAB_TOKEN: $GITLAB_TOKEN
  only:
    - schedules
```

### Cron Job

```bash
#!/bin/bash
# /etc/cron.daily/ekapkgs-update

cd /path/to/repo
export GITHUB_TOKEN="ghp_..."
export CACHIX_AUTH_TOKEN="..."

ekapkgs-update run \
  --file ./default.nix \
  --create-pr \
  --upstream nixpkgs \
  --fork origin \
  --concurrent-updates 2 \
  --skip-unstable \
  --max-rebuilds 50 \
  --database /var/lib/ekapkgs-update/updates.db

```

Save logs:

```bash
ekapkgs-update run --file ./default.nix 2>&1 | \
  tee -a /var/log/ekapkgs-update.log
```

## Advanced Workflows

### Staggered Updates

Update packages in batches:

```bash
# Monday: Update first 50 packages
ekapkgs-update run --concurrent-updates 1 # will process in order

# Tuesday: Continue from where we left off
# Database tracks which packages were updated
ekapkgs-update run --concurrent-updates 1
```

### Selective Updates

Update only specific package subsets:

```bash
# Update only Python packages
nix-instantiate --eval --expr '
  with import ./default.nix {};
  lib.attrNames python3Packages
' | jq -r '.[]' | while read pkg; do
  ekapkgs-update update "python3Packages.$pkg" --commit
done
```

### Two-Phase Updates

First dry-run, then actual updates:

```bash
# Phase 1: Dry run to see what's available
ekapkgs-update run --dry-run > updates-available.txt

# Review updates-available.txt

# Phase 2: Actually update
ekapkgs-update run --create-pr
```

## Monitoring and Alerting

### Log Parsing

```bash
# Count successful updates
grep "Update successful" /var/log/ekapkgs-update.log | wc -l

# Find failed updates
grep "Update failed" /var/log/ekapkgs-update.log

# Extract updated packages
grep "Update successful" /var/log/ekapkgs-update.log | \
  awk '{print $2}'
```

### Prometheus Metrics

Export metrics for monitoring (requires custom wrapper):

```bash
#!/bin/bash
# prometheus-wrapper.sh

output=$(ekapkgs-update run --file ./default.nix 2>&1)

checked=$(echo "$output" | grep "checked" | awk '{print $2}')
updated=$(echo "$output" | grep "updated" | awk '{print $4}')
failed=$(echo "$output" | grep "failed" | awk '{print $8}')

cat <<EOF > /var/lib/prometheus/node-exporter/ekapkgs-update.prom
# HELP ekapkgs_packages_checked Number of packages checked
# TYPE ekapkgs_packages_checked gauge
ekapkgs_packages_checked $checked

# HELP ekapkgs_packages_updated Number of packages updated
# TYPE ekapkgs_packages_updated gauge
ekapkgs_packages_updated $updated

# HELP ekapkgs_packages_failed Number of packages that failed to update
# TYPE ekapkgs_packages_failed gauge
ekapkgs_packages_failed $failed
EOF
```

### Email Notifications

```bash
#!/bin/bash
# email-wrapper.sh

output=$(ekapkgs-update run --file ./default.nix 2>&1)
exit_code=$?

if [ $exit_code -ne 0 ]; then
  echo "$output" | mail -s "ekapkgs-update failed" admin@example.com
fi
```

## Best Practices

### Start Small

When first setting up daemon mode:

1. Start with `--dry-run` to understand the scope
2. Use `--concurrent-updates 1` initially
3. Test with a subset of packages
4. Gradually increase concurrency and coverage

### Resource Management

- **CPU** - Limit concurrent updates on smaller machines
- **Network** - Configure API tokens to avoid rate limits
- **Disk** - Ensure adequate space for Nix store
- **Memory** - Watch for OOM with high concurrency

### Error Handling

- **Review logs** - Check `ekapkgs-update log <package>` for failures
- **Retry manually** - Failed packages can be updated manually with `--force`
- **Incremental updates** - Use `--src-only` for problematic packages

### Security

- **API tokens** - Use secrets management, not hardcoded values
- **PR review** - Don't blindly merge automated PRs
- **Testing** - Use `--run-passthru-tests` for critical packages
- **CVE checks** - Keep CVE checking enabled

## Troubleshooting

### No Packages Found

```
INFO Found 0 packages to check
```

**Causes:**

- Wrong file path
- File doesn't export packages properly
- All packages have `skip = true`

**Debug:**

```bash
nix-instantiate --eval --expr '
  with import ./default.nix {};
  builtins.attrNames (lib.filterAttrs (n: v: lib.isDerivation v) pkgs)
' | head
```

### Rate Limiting

```
ERROR GitHub API rate limit exceeded
```

**Fix:**

```bash
export GITHUB_TOKEN="ghp_..."
```

See [Installation](../installation.md#api-tokens-recommended).

### Database Locked

```
ERROR Database is locked
```

**Causes:**

- Another ekapkgs-update instance running
- Interrupted previous run

**Fix:**

```bash
# Find and kill other instances
pkill ekapkgs-update

# Or use a different database
ekapkgs-update run --database ./updates-2.db
```

### Out of Memory

**Symptoms:**

- Builds failing with OOM errors
- System becoming unresponsive

**Fix:**

```bash
# Reduce concurrency
ekapkgs-update run --concurrent-updates 1

# Or increase swap space
```

## See Also

- [Manual Updates](./manual-updates.md) - Single package updates
- [CLI Reference](../cli-reference.md) - Complete command documentation
- [Passthru Attributes](../passthru-attributes.md) - Per-package configuration
- [PR Enhancements](../advanced/pr-enhancements.md) - Detailed PR feature documentation
