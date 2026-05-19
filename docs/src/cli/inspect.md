# inspect & log - View Failure Details

The `inspect` and `log` commands provide detailed information about package update failures, including logs, context, and debugging information.

## Synopsis

```bash
# Detailed failure context
ekapkgs-update inspect [OPTIONS] <IDENTIFIER>

# Build and error logs
ekapkgs-update log [OPTIONS] <IDENTIFIER>
```

## Description

### inspect

Provides comprehensive failure information:
- Error type and phase
- Full error message
- Package metadata
- File locations
- Preserved worktree path (if available)
- Retry suggestions

### log

Shows raw logs from failed operations:
- Build output
- Test results
- Error messages
- Nix evaluation logs

## Arguments

### `<IDENTIFIER>`

Package identifier, can be:
- **Attribute path**: `python312Packages.requests`
- **Derivation path**: `/nix/store/...-python3.12-requests-2.32.0.drv`
- **Derivation name**: `hash-name.drv`

```bash
# Using attribute path (most common)
ekapkgs-update inspect python312Packages.requests

# Using store path
ekapkgs-update inspect /nix/store/xyz...-python3.12-requests-2.32.0.drv

# Using derivation name
ekapkgs-update log abc123-python3.12-requests-2.32.0.drv
```

## Options

#### `--database <PATH>` (short: `-d`)
Path to SQLite database.

**Default:** `~/.cache/ekapkgs-update/db.sqlite3`

```bash
ekapkgs-update inspect --database /var/lib/ekapkgs-update/db.sqlite3 mypackage
```

## Output Examples

### inspect Output

```
Package Failure Details:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Package: python312Packages.requests
Attribute: python312Packages.requests
Status: failed
Phase: Build
Error Type: BuildFailure

Error Message:
builder for '/nix/store/...-python3.12-requests-2.32.0.drv' failed with exit code 2

Details:
  Version attempted: 2.31.0 -> 2.32.0
  Updated file: /path/to/pkgs/python-modules/requests/default.nix
  Build output: /nix/store/...-python3.12-requests-2.32.0.drv
  Timestamp: 2024-05-19 10:30:15

Preserved Artifacts:
  Worktree: /tmp/ekapkgs-update-worktrees/python312Packages.requests
  Build log: /tmp/ekapkgs-update-worktrees/python312Packages.requests/build.log

Context:
  Session ID: 42
  Attempt: 1 of 3
  Previous failures: 2 (2024-05-18, 2024-05-17)

Suggested Actions:
  1. View full logs: ekapkgs-update log python312Packages.requests
  2. Inspect worktree: cd /tmp/ekapkgs-update-worktrees/python312Packages.requests
  3. Export for LLM: ekapkgs-update export python312Packages.requests
  4. Retry with patch: ekapkgs-update retry python312Packages.requests --apply-patch fix.patch

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

### log Output

```
Build Log for python312Packages.requests:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

@nix { "action": "setPhase", "phase": "unpackPhase" }
unpacking sources
unpacking source archive /nix/store/...-source.tar.gz
source root is source
setting SOURCE_DATE_EPOCH to timestamp 1234567890 of file source/setup.py

@nix { "action": "setPhase", "phase": "patchPhase" }
patching sources
applying patch /nix/store/...-fix-tests.patch
patching file tests/test_adapters.py
Hunk #1 FAILED at 45.
1 out of 1 hunk FAILED -- saving rejects to file tests/test_adapters.py.rej

@nix { "action": "setPhase", "phase": "buildPhase" }
Executing pipBuildPhase
Creating a wheel...
error: builder for '/nix/store/...-python3.12-requests-2.32.0.drv' failed with exit code 2

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Exit code: 2
Phase: buildPhase
Error: Patch application failed
```

## Use Cases

### Initial Failure Investigation

```bash
# 1. Check what failed
ekapkgs-update query --since-days 1 --status failed

# 2. Inspect specific failure
ekapkgs-update inspect python312Packages.requests

# 3. View full logs
ekapkgs-update log python312Packages.requests
```

### Debugging Build Failures

```bash
# View failure details
ekapkgs-update inspect mypackage

# Check logs for specific error
ekapkgs-update log mypackage | grep -A 10 "error:"

# Navigate to preserved worktree
WORKTREE=$(ekapkgs-update inspect mypackage | grep "Worktree:" | awk '{print $2}')
cd "$WORKTREE"
```

### Understanding Patterns

```bash
# Inspect multiple related failures
for pkg in pkg1 pkg2 pkg3; do
  echo "=== $pkg ==="
  ekapkgs-update inspect "$pkg"
done
```

### Pre-Retry Analysis

```bash
# Before retrying, understand what went wrong
ekapkgs-update inspect mypackage

# Check if worktree is preserved
ekapkgs-update worktrees show mypackage

# Review logs for context
ekapkgs-update log mypackage | less
```

### LLM-Assisted Debugging

```bash
# 1. Inspect to understand the failure
ekapkgs-update inspect mypackage

# 2. Export for AI analysis
ekapkgs-update export mypackage --format markdown --output context.md

# 3. Review logs
ekapkgs-update log mypackage >> context.md

# 4. Provide context.md to LLM for analysis
```

## Common Failure Types

### HashMismatch

```bash
$ ekapkgs-update inspect terraform

Error Type: HashMismatch
Phase: UpdateHash
Message: hash mismatch in fixed-output derivation

Common causes:
  - Upstream changed release tarball
  - Version tag was moved
  - Different source than expected

Solutions:
  - Verify version is correct
  - Check upstream for tarball changes
  - Use --version-regex if tag format changed
```

### BuildFailure

```bash
$ ekapkgs-update inspect gcc

Error Type: BuildFailure
Phase: Build
Message: builder failed with exit code 2

Common causes:
  - Incompatible patches
  - Missing dependencies
  - Build system changes

Solutions:
  - Check build logs: ekapkgs-update log gcc
  - Review patches in preserved worktree
  - Update or remove outdated patches
```

### TestFailure

```bash
$ ekapkgs-update inspect python312Packages.pytest

Error Type: TestFailure
Phase: Test
Message: passthru.tests.pytest failed

Common causes:
  - Test suite changes
  - New test dependencies
  - Environment differences

Solutions:
  - Review test logs: ekapkgs-update log python312Packages.pytest
  - Check test dependencies
  - Consider skipping flaky tests
```

### PatchFailure

```bash
$ ekapkgs-update inspect nodejs

Error Type: PatchFailure
Phase: Build
Message: patch application failed

Common causes:
  - Code changed in new version
  - Patch already applied upstream
  - Conflicting changes

Solutions:
  - Remove outdated patches
  - Update patch for new code
  - Check if patch is still needed
```

## Filtering Logs

### grep for Errors

```bash
# Find error lines
ekapkgs-update log mypackage | grep -i error

# Context around errors
ekapkgs-update log mypackage | grep -C 5 "error:"

# Find specific phase
ekapkgs-update log mypackage | grep -A 20 "buildPhase"
```

### Extract Specific Information

```bash
# Get failed files
ekapkgs-update log mypackage | grep "FAILED"

# Find missing dependencies
ekapkgs-update log mypackage | grep "command not found"

# Check test results
ekapkgs-update log mypackage | grep -E "(PASS|FAIL|ERROR)"
```

### Save for Analysis

```bash
# Save logs to file
ekapkgs-update log mypackage > /tmp/mypackage-build.log

# Compare with previous version
ekapkgs-update log mypackage > new.log
# (rebuild old version)
nix-build -A mypackage 2>&1 > old.log
diff old.log new.log
```

## Integration with Other Commands

### inspect -> log -> export

```bash
# 1. Quick overview
ekapkgs-update inspect mypackage

# 2. Detailed logs
ekapkgs-update log mypackage | less

# 3. Export for LLM
ekapkgs-update export mypackage --format markdown
```

### inspect -> worktrees -> retry

```bash
# 1. Understand failure
ekapkgs-update inspect mypackage

# 2. Examine preserved worktree
ekapkgs-update worktrees show mypackage

# 3. Manual fixes in worktree
cd /tmp/ekapkgs-update-worktrees/mypackage
# ... make changes ...

# 4. Retry
ekapkgs-update retry mypackage
```

### inspect -> query

```bash
# Inspect specific package
ekapkgs-update inspect mypackage

# If it's a pattern, find similar failures
ERROR_TYPE=$(ekapkgs-update inspect mypackage | grep "Error Type:" | awk '{print $3}')
ekapkgs-update query --error-type "$ERROR_TYPE"
```

## Output Sections

### inspect Sections

1. **Package Information**
   - Attribute path
   - Package name
   - Version information

2. **Failure Details**
   - Error type
   - Phase where failure occurred
   - Error message

3. **Context**
   - File locations
   - Derivation paths
   - Timestamp

4. **Artifacts**
   - Preserved worktree location
   - Build logs
   - Related files

5. **History**
   - Previous attempts
   - Session information
   - Related failures

6. **Suggestions**
   - Next steps
   - Related commands
   - Recovery options

### log Sections

1. **Build Phases**
   - unpackPhase
   - patchPhase
   - configurePhase
   - buildPhase
   - checkPhase
   - installPhase

2. **Error Information**
   - Exit codes
   - Error messages
   - Stack traces

3. **Nix Metadata**
   - Phase transitions
   - Environment variables
   - Derivation info

## Exit Codes

- `0` - Information retrieved successfully
- `1` - Package not found in database or error retrieving information
- `2` - Invalid arguments

## See Also

- [query](./query.md) - Search for failures
- [retry](./retry.md) - Retry failed updates
- [export](./export-apply.md) - Export for LLM analysis
- [worktrees](./worktrees.md) - Manage preserved artifacts
- [Debugging Use Case](../use-cases/debugging.md)
