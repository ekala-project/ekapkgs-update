# retry - Retry Failed Updates

The `retry` command allows you to retry a failed package update, optionally resuming from a specific phase or applying fixes.

## Synopsis

```bash
ekapkgs-update retry [OPTIONS] <ATTR_PATH>
```

## Description

Retry a previously failed update using preserved worktree artifacts. The retry can:
- Resume from a specific phase (if supported)
- Apply a patch before retrying
- Override the version being updated to
- Use preserved context from the original failure

This command requires that the original update ran with `--preserve-failures`.

## Arguments

### `<ATTR_PATH>`
Package attribute path to retry.

```bash
ekapkgs-update retry python312Packages.requests
```

## Options

### Database

#### `--database <PATH>` (short: `-d`)
Path to SQLite database.

**Default:** `~/.cache/ekapkgs-update/db.sqlite3`

```bash
ekapkgs-update retry --database /var/lib/ekapkgs-update/db.sqlite3 mypackage
```

### Phase Resumption

#### `--from-phase <PHASE>`
Resume from specific phase (if supported).

```bash
# Retry from build phase (skip hash updates)
ekapkgs-update retry mypackage --from-phase Build

# Retry from test phase only
ekapkgs-update retry mypackage --from-phase Test
```

**Available phases:**
- `Evaluation` - Re-evaluate package
- `VersionFetch` - Re-fetch available versions
- `UpdateHash` - Re-update source hash
- `UpdateDependencyHashes` - Re-update cargo/npm/vendor hashes
- `Build` - Re-build package
- `Test` - Re-run tests
- `Commit` - Re-create commit
- `PR` - Re-create pull request

**Use cases:**
- Skip successful early phases
- Re-run only failed phase after fixes
- Iterate on specific phase

**Note:** Not all phases support resumption. The command will restart from the beginning if the phase doesn't support resumption.

### Patching

#### `--apply-patch <PATH>`
Apply patch file before retrying.

```bash
# Apply manual fix
ekapkgs-update retry mypackage --apply-patch /tmp/fix.patch

# Apply LLM-generated fix
ekapkgs-update retry mypackage --apply-patch llm-fix.patch
```

The patch is applied to the preserved worktree before the retry begins.

**Patch format:** Standard unified diff format (created with `git diff` or `diff -u`)

### Version Override

#### `--version <VERSION>`
Override version to update to.

```bash
# Try different version
ekapkgs-update retry mypackage --version 2.5.0

# Retry with pre-release
ekapkgs-update retry mypackage --version 2.6.0-beta.1
```

Use cases:
- Original version had issues
- Try older version
- Test pre-release version

## Workflow

1. **Verify Prerequisites**
   - Check that worktree exists
   - Verify database has failure record
   - Ensure patch is valid (if provided)

2. **Restore Context**
   - Load preserved worktree
   - Restore git state
   - Load original configuration

3. **Apply Modifications**
   - Apply patch (if provided)
   - Update version (if provided)
   - Adjust starting phase (if provided)

4. **Execute Retry**
   - Resume from specified phase
   - Follow normal update workflow
   - Record new attempt in database

5. **Cleanup or Preserve**
   - On success: clean up worktree
   - On failure: preserve for another retry

## Examples

### Basic Retry

```bash
# Simple retry after manual fix
cd /tmp/ekapkgs-update-worktrees/mypackage
# ... make manual changes ...
cd -
ekapkgs-update retry mypackage
```

### Retry with Patch

```bash
# Create patch from manual fixes
cd /tmp/ekapkgs-update-worktrees/mypackage
# ... make changes ...
git diff > /tmp/fix.patch
cd -

# Apply and retry
ekapkgs-update retry mypackage --apply-patch /tmp/fix.patch
```

### Phase-Specific Retry

```bash
# Hash update failed, fixed manually, retry from build
ekapkgs-update retry mypackage --from-phase Build

# Build succeeded, but tests failed, retry tests only
ekapkgs-update retry mypackage --from-phase Test
```

### Version Override

```bash
# Original version 2.6.0 failed, try 2.5.1
ekapkgs-update retry mypackage --version 2.5.1

# Original auto-selected version had issues, try specific tag
ekapkgs-update retry mypackage --version v2.5.0
```

### Combined Options

```bash
# Apply fix, skip to build, use different version
ekapkgs-update retry mypackage \
  --apply-patch fix.patch \
  --from-phase Build \
  --version 2.5.0
```

### LLM-Assisted Retry

```bash
# 1. Export failure context
ekapkgs-update export mypackage --format markdown --output context.md

# 2. Get LLM to generate fix (external step)
# ... provide context.md to LLM ...
# ... LLM generates fix.patch ...

# 3. Apply and retry
ekapkgs-update retry mypackage --apply-patch fix.patch
```

## Common Retry Scenarios

### Patch Application Failure

**Original error:** Patch failed to apply

**Solution:**
```bash
# 1. Inspect the failure
ekapkgs-update inspect mypackage

# 2. Go to worktree
cd /tmp/ekapkgs-update-worktrees/mypackage

# 3. Remove or update the problematic patch
git rm patches/outdated.patch

# 4. Create fix patch
git diff > /tmp/remove-patch.patch

# 5. Retry
ekapkgs-update retry mypackage --apply-patch /tmp/remove-patch.patch
```

### Build Failure - Missing Dependency

**Original error:** Command not found during build

**Solution:**
```bash
# Add missing build dependency to Nix expression
cd /tmp/ekapkgs-update-worktrees/mypackage
# Edit default.nix to add dependency
git diff > /tmp/add-dep.patch

ekapkgs-update retry mypackage --apply-patch /tmp/add-dep.patch
```

### Test Failure - Flaky Tests

**Original error:** Tests failed

**Solution:**
```bash
# Disable flaky tests
cd /tmp/ekapkgs-update-worktrees/mypackage
# Add doCheck = false; or adjust test settings
git diff > /tmp/disable-tests.patch

# Retry from build phase (tests will be skipped)
ekapkgs-update retry mypackage \
  --apply-patch /tmp/disable-tests.patch \
  --from-phase Build
```

### Hash Mismatch

**Original error:** Hash mismatch in fixed-output derivation

**Solution:**
```bash
# Sometimes the hash was computed incorrectly
# Retry with the correct version
ekapkgs-update retry mypackage --version 2.5.1

# Or manually fix the hash in worktree
cd /tmp/ekapkgs-update-worktrees/mypackage
# Update hash in default.nix
git diff > /tmp/fix-hash.patch
ekapkgs-update retry mypackage --apply-patch /tmp/fix-hash.patch
```

### Version Selection Issue

**Original error:** Wrong version selected

**Solution:**
```bash
# Retry with explicit version
ekapkgs-update retry mypackage --version 2.5.0

# Or with different semver strategy (requires full re-run)
ekapkgs-update update mypackage --semver minor --commit
```

## Prerequisites

### Preserved Worktree Required

The retry command requires a preserved worktree from a previous failure:

```bash
# Check if worktree exists
ekapkgs-update worktrees show mypackage

# If not found, you need to re-run the original update with --preserve-failures
ekapkgs-update update mypackage --preserve-failures
# (let it fail)
ekapkgs-update retry mypackage
```

### Database Record Required

```bash
# Verify database has failure record
ekapkgs-update inspect mypackage

# If not found, the package wasn't tracked
# Solution: run new update instead
ekapkgs-update update mypackage
```

## Integration with Other Commands

### inspect -> retry

```bash
# Understand what failed
ekapkgs-update inspect mypackage

# Retry based on findings
ekapkgs-update retry mypackage --from-phase Build
```

### worktrees -> retry

```bash
# View preserved worktree
ekapkgs-update worktrees show mypackage

# Make manual changes
cd /tmp/ekapkgs-update-worktrees/mypackage
# ... changes ...

# Retry
ekapkgs-update retry mypackage
```

### export -> apply -> retry

```bash
# Export context for LLM
ekapkgs-update export mypackage --format markdown

# Apply LLM-generated fix (includes retry)
ekapkgs-update apply mypackage --patch fix.patch --resume

# Or manually retry
ekapkgs-update retry mypackage --apply-patch fix.patch
```

### query -> retry (batch)

```bash
# Find all hash mismatch failures
ekapkgs-update query --error-type "HashMismatch" --since-days 7

# Retry each one
for pkg in $(ekapkgs-update query --error-type "HashMismatch" --since-days 7 | grep "Package:" | awk '{print $2}'); do
  echo "Retrying $pkg..."
  ekapkgs-update retry "$pkg"
done
```

## Limitations

### Phase Resumption

Not all phases can be resumed:
- Some phases have side effects (e.g., git commits)
- Some phases depend on previous state
- Some phases are not idempotent

If a phase can't be resumed, the retry starts from the beginning.

### Worktree Staleness

Preserved worktrees can become stale:
- Upstream changes
- Local repository changes
- Time passing

For old worktrees, consider a fresh update instead:
```bash
# Clean old worktree
ekapkgs-update worktrees clean --older-than 7

# Start fresh
ekapkgs-update update mypackage --commit
```

### Patch Conflicts

Patches may not apply cleanly:
- Worktree state changed
- Conflicting modifications
- Invalid patch format

Solution: Create patch manually in the worktree:
```bash
cd /tmp/ekapkgs-update-worktrees/mypackage
# ... make changes ...
# Don't create patch, just retry
cd -
ekapkgs-update retry mypackage
```

## Exit Codes

- `0` - Retry succeeded
- `1` - Retry failed (check logs with `log` or `inspect`)
- `2` - Invalid arguments or prerequisites not met

## See Also

- [run](./run.md) - Batch updates with `--preserve-failures`
- [update](./update.md) - Single package update
- [inspect](./inspect.md) - View failure details
- [worktrees](./worktrees.md) - Manage preserved worktrees
- [export/apply](./export-apply.md) - LLM-assisted fixes
- [Debugging Use Case](../use-cases/debugging.md)
