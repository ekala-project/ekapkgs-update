# worktrees - Manage Preserved Artifacts

The `worktrees` subcommands manage preserved failure artifacts, allowing you to list, inspect, and clean up preserved worktrees.

## Synopsis

```bash
ekapkgs-update worktrees <SUBCOMMAND>

# List all preserved worktrees
ekapkgs-update worktrees list [OPTIONS]

# Show details of specific worktree
ekapkgs-update worktrees show <ATTR_PATH> [OPTIONS]

# Clean up old worktrees
ekapkgs-update worktrees clean [OPTIONS]
```

## Description

When updates fail and `--preserve-failures` is enabled, ekapkgs-update preserves:
- Modified source files in a git worktree
- Build logs and error output
- Package metadata and context
- All artifacts needed for retry

The `worktrees` commands help manage these preserved artifacts.

## Subcommands

### list

List all preserved failed worktrees.

#### Synopsis
```bash
ekapkgs-update worktrees list [OPTIONS]
```

#### Options

**`--database <PATH>` (short: `-d`)**

Path to SQLite database.

**Default:** `~/.cache/ekapkgs-update/db.sqlite3`

```bash
ekapkgs-update worktrees list --database /var/lib/ekapkgs-update/db.sqlite3
```

#### Output

```
Preserved Worktrees:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Package: python312Packages.requests
Path: /tmp/ekapkgs-update-worktrees/python312Packages.requests
Created: 2024-05-19 10:30:15 (2 hours ago)
Size: 45 MB
Status: failed
Phase: Build
Error: BuildFailure

Package: terraform
Path: /tmp/ekapkgs-update-worktrees/terraform
Created: 2024-05-19 09:15:42 (3 hours ago)
Size: 128 MB
Status: failed
Phase: UpdateHash
Error: HashMismatch

Package: gcc
Path: /tmp/ekapkgs-update-worktrees/gcc
Created: 2024-05-18 15:22:10 (1 day ago)
Size: 2.1 GB
Status: failed
Phase: Build
Error: BuildFailure

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Total preserved worktrees: 3
Total disk usage: 2.3 GB
```

#### Examples

```bash
# List all preserved worktrees
ekapkgs-update worktrees list

# Sort by age (oldest first)
ekapkgs-update worktrees list | sort -k 4

# Count preserved worktrees
ekapkgs-update worktrees list | grep -c "Package:"
```

### show

Show details of a specific preserved worktree.

#### Synopsis
```bash
ekapkgs-update worktrees show <ATTR_PATH> [OPTIONS]
```

#### Arguments

**`<ATTR_PATH>`**

Package attribute path.

```bash
ekapkgs-update worktrees show python312Packages.requests
```

#### Options

**`--database <PATH>` (short: `-d`)**

Path to SQLite database.

**Default:** `~/.cache/ekapkgs-update/db.sqlite3`

```bash
ekapkgs-update worktrees show mypackage --database /var/lib/ekapkgs-update/db.sqlite3
```

#### Output

```
Preserved Worktree Details:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Package: python312Packages.requests
Attribute Path: python312Packages.requests
Worktree Path: /tmp/ekapkgs-update-worktrees/python312Packages.requests

Status Information:
  Status: failed
  Phase: Build
  Error Type: BuildFailure
  Timestamp: 2024-05-19 10:30:15

Version Information:
  Current: 2.31.0
  Target: 2.32.0
  Strategy: latest

Files Modified:
  - pkgs/python-modules/requests/default.nix

Build Artifacts:
  - build.log (125 KB)
  - result -> /nix/store/...-python3.12-requests-2.32.0
  - .nix-patches/ (3 patches)

Git Status:
  Branch: update/python312Packages.requests-2.32.0
  Modified files: 1
  Untracked files: 0

Disk Usage: 45 MB

Available Actions:
  1. Inspect worktree: cd /tmp/ekapkgs-update-worktrees/python312Packages.requests
  2. View logs: cat /tmp/ekapkgs-update-worktrees/python312Packages.requests/build.log
  3. Retry: ekapkgs-update retry python312Packages.requests
  4. Export: ekapkgs-update export python312Packages.requests
  5. Clean: ekapkgs-update worktrees clean --older-than 0

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

#### Examples

```bash
# Show worktree details
ekapkgs-update worktrees show python312Packages.requests

# Check if worktree exists
if ekapkgs-update worktrees show mypackage &>/dev/null; then
  echo "Worktree exists"
else
  echo "No preserved worktree"
fi

# Get worktree path
WORKTREE=$(ekapkgs-update worktrees show mypackage | grep "Worktree Path:" | awk '{print $3}')
cd "$WORKTREE"
```

### clean

Clean up old preserved worktrees.

#### Synopsis
```bash
ekapkgs-update worktrees clean [OPTIONS]
```

#### Options

**`--older-than <DAYS>`**

Remove artifacts older than N days.

**Default:** `7`

```bash
# Clean worktrees older than 7 days (default)
ekapkgs-update worktrees clean

# Clean worktrees older than 30 days
ekapkgs-update worktrees clean --older-than 30

# Clean all worktrees (older than 0 days)
ekapkgs-update worktrees clean --older-than 0
```

#### Output

```
Cleaning preserved worktrees older than 7 days...
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Removing: gcc (created 8 days ago)
  Path: /tmp/ekapkgs-update-worktrees/gcc
  Size: 2.1 GB
  ✓ Removed

Removing: nodejs (created 10 days ago)
  Path: /tmp/ekapkgs-update-worktrees/nodejs
  Size: 256 MB
  ✓ Removed

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Cleaned 2 worktrees
Freed 2.4 GB of disk space

Remaining worktrees: 1
```

#### Examples

```bash
# Regular cleanup (weekly cron job)
ekapkgs-update worktrees clean

# Aggressive cleanup
ekapkgs-update worktrees clean --older-than 3

# Clean everything
ekapkgs-update worktrees clean --older-than 0

# Dry-run (show what would be cleaned)
ekapkgs-update worktrees list | awk '/Created:/ { ... }' # manual filtering
```

## Use Cases

### Discovery

```bash
# What failures are preserved?
ekapkgs-update worktrees list

# Get details on specific failure
ekapkgs-update worktrees show mypackage
```

### Manual Debugging

```bash
# Find worktree
ekapkgs-update worktrees show mypackage

# Navigate to it
cd /tmp/ekapkgs-update-worktrees/mypackage

# Examine files
ls -la
cat build.log
git status
git diff

# Make changes
vim pkgs/mypackage/default.nix

# Retry
ekapkgs-update retry mypackage
```

### Disk Space Management

```bash
# Check total disk usage
ekapkgs-update worktrees list | grep "Total disk usage:"

# Clean old worktrees
ekapkgs-update worktrees clean --older-than 7

# Aggressive cleanup when disk is full
ekapkgs-update worktrees clean --older-than 1
```

### Batch Operations

```bash
# List all worktrees and iterate
ekapkgs-update worktrees list | grep "Package:" | while read -r _ pkg; do
  echo "Processing $pkg..."
  ekapkgs-update retry "$pkg"
done
```

### CI/CD Integration

```bash
# Preserve failures in CI
ekapkgs-update run --preserve-failures

# Archive worktrees as artifacts
tar czf worktrees.tar.gz /tmp/ekapkgs-update-worktrees/

# Download and inspect locally
tar xzf worktrees.tar.gz
ekapkgs-update worktrees list
```

## Worktree Structure

A preserved worktree contains:

```
/tmp/ekapkgs-update-worktrees/python312Packages.requests/
├── .git/                    # Git worktree metadata
├── pkgs/                    # Modified package files
│   └── python-modules/
│       └── requests/
│           └── default.nix
├── build.log                # Build output
├── result                   # Symlink to build output
├── .ekapkgs-update/         # Metadata
│   ├── context.json         # Update context
│   ├── original-version     # Version info
│   └── error-details        # Error information
└── .nix-patches/            # Patch files (if any)
    ├── fix-tests.patch
    └── update-deps.patch
```

## Worktree Locations

**Default base directory:** `/tmp/ekapkgs-update-worktrees/`

**Directory naming:** `<attr-path>` (with dots preserved)

Examples:
- `/tmp/ekapkgs-update-worktrees/hello`
- `/tmp/ekapkgs-update-worktrees/python312Packages.requests`
- `/tmp/ekapkgs-update-worktrees/haskellPackages.pandoc`

## Integration with Other Commands

### worktrees -> inspect

```bash
# List preserved worktrees
ekapkgs-update worktrees list

# Get detailed failure info
ekapkgs-update inspect python312Packages.requests
```

### worktrees -> retry

```bash
# Show worktree details
ekapkgs-update worktrees show mypackage

# Make manual changes
cd /tmp/ekapkgs-update-worktrees/mypackage
# ... edit files ...

# Retry
ekapkgs-update retry mypackage
```

### worktrees -> export -> apply

```bash
# Check worktree exists
ekapkgs-update worktrees show mypackage

# Export for LLM
ekapkgs-update export mypackage --format markdown

# Apply LLM fix
ekapkgs-update apply mypackage --patch fix.patch --resume
```

### run -> worktrees -> clean

```bash
# Run with preservation
ekapkgs-update run --preserve-failures

# Review failures
ekapkgs-update worktrees list

# Clean up after fixes
ekapkgs-update worktrees clean --older-than 7
```

## Automatic Cleanup

Worktrees are automatically cleaned up:

**On successful retry:**
- Worktree is removed
- Database record updated
- Disk space freed

**On manual removal:**
- Use `worktrees clean` command
- Or manually `rm -rf /tmp/ekapkgs-update-worktrees/<package>`

**Never automatically cleaned:**
- Failed retries preserve the worktree
- Use explicit `worktrees clean` command
- Survives reboots (unless `/tmp` is cleared)

## Best Practices

### Regular Cleanup

```bash
# Weekly cron job
0 0 * * 0 ekapkgs-update worktrees clean --older-than 7
```

### Disk Space Monitoring

```bash
# Check disk usage
df -h /tmp
ekapkgs-update worktrees list | grep "Total disk usage:"

# Alert if usage is high
USAGE=$(ekapkgs-update worktrees list | grep "Total disk usage:" | awk '{print $4}' | sed 's/GB//')
if (( $(echo "$USAGE > 10" | bc -l) )); then
  echo "WARNING: Worktrees using ${USAGE}GB"
  ekapkgs-update worktrees clean --older-than 3
fi
```

### Selective Preservation

```bash
# Only preserve failures in CI (not local development)
if [ -n "$CI" ]; then
  ekapkgs-update run --preserve-failures
else
  ekapkgs-update run
fi
```

### Archive Important Failures

```bash
# Before cleaning, archive important failures
for pkg in critical-package important-package; do
  if ekapkgs-update worktrees show "$pkg" &>/dev/null; then
    tar czf "${pkg}-worktree.tar.gz" "/tmp/ekapkgs-update-worktrees/$pkg"
  fi
done

# Then clean
ekapkgs-update worktrees clean
```

## Troubleshooting

### Worktree Not Found

```bash
$ ekapkgs-update worktrees show mypackage
Error: No preserved worktree found for mypackage

# Solution: Re-run update with --preserve-failures
ekapkgs-update update mypackage --preserve-failures
```

### Disk Full

```bash
$ ekapkgs-update run --preserve-failures
Error: No space left on device

# Solution: Clean old worktrees
ekapkgs-update worktrees clean --older-than 1

# Or clean all
ekapkgs-update worktrees clean --older-than 0
```

### Corrupted Worktree

```bash
$ cd /tmp/ekapkgs-update-worktrees/mypackage
$ git status
fatal: not a git repository

# Solution: Remove and retry
rm -rf /tmp/ekapkgs-update-worktrees/mypackage
ekapkgs-update update mypackage --preserve-failures
```

## Exit Codes

- `0` - Command succeeded
- `1` - Error (e.g., worktree not found, cleanup failed)
- `2` - Invalid arguments

## See Also

- [run](./run.md) - Batch updates with `--preserve-failures`
- [update](./update.md) - Single update with preservation
- [retry](./retry.md) - Retry using preserved worktree
- [inspect](./inspect.md) - View failure details
- [export/apply](./export-apply.md) - LLM-assisted fixes
- [Failure Preservation](../advanced/failure-preservation.md)
- [Debugging Use Case](../use-cases/debugging.md)
