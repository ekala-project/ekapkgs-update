# Debugging Failed Updates

Comprehensive guide to troubleshooting and fixing failed package updates.

## Debug Workflow

### 1. Identify the Failure

```bash
# Check recent failures
ekapkgs-update query --since-days 1 --status failed

# Get high-level overview
ekapkgs-update query --since-days 7 --group-by-error
```

### 2. Inspect Details

```bash
# View comprehensive failure info
ekapkgs-update inspect python312Packages.requests

# View build logs
ekapkgs-update log python312Packages.requests
```

### 3. Examine Preserved Artifacts

```bash
# Check if worktree preserved
ekapkgs-update worktrees show python312Packages.requests

# Navigate to worktree
cd /tmp/ekapkgs-update-worktrees/python312Packages.requests

# Examine changes
git status
git diff
cat build.log
```

### 4. Fix and Retry

```bash
# Option A: Manual fixes in worktree
cd /tmp/ekapkgs-update-worktrees/python312Packages.requests
# ... make changes ...
cd -
ekapkgs-update retry python312Packages.requests

# Option B: Create patch
cd /tmp/ekapkgs-update-worktrees/python312Packages.requests
# ... make changes ...
git diff > /tmp/fix.patch
cd -
ekapkgs-update retry python312Packages.requests --apply-patch /tmp/fix.patch

# Option C: LLM-assisted
ekapkgs-update export python312Packages.requests --format markdown > context.md
# ... get LLM to generate fix.patch ...
ekapkgs-update apply python312Packages.requests --patch fix.patch --resume
```

## Common Failure Types

### Hash Mismatch

**Symptom:**
```
Error: hash mismatch in fixed-output derivation
  specified: sha256-AAAA...
  got:        sha256-BBBB...
```

**Causes:**
- Upstream changed release tarball
- Version tag was moved/updated
- Source URL changed

**Solutions:**

```bash
# 1. Verify the version is correct
ekapkgs-update inspect mypackage | grep version

# 2. Check upstream release
curl -L "https://github.com/owner/repo/archive/v2.5.0.tar.gz" | sha256sum

# 3. Try re-fetching
ekapkgs-update update mypackage --version 2.5.0

# 4. If upstream changed, accept new hash
# (ekapkgs-update does this automatically)
```

### Build Failure

**Symptom:**
```
Error: builder for '/nix/store/...-mypackage-2.5.0.drv' failed with exit code 2
```

**Debug steps:**

```bash
# 1. View full build log
ekapkgs-update log mypackage | less

# 2. Look for specific errors
ekapkgs-update log mypackage | grep -i error
ekapkgs-update log mypackage | grep -i "command not found"

# 3. Check which phase failed
ekapkgs-update log mypackage | grep "setPhase"

# 4. Examine worktree
cd /tmp/ekapkgs-update-worktrees/mypackage
nix-build -A mypackage  # Try building manually
```

**Common causes:**

#### Missing Dependencies

```bash
# Error in log: "command not found: cmake"

# Fix: Add to buildInputs
cat > fix.patch << 'EOF'
--- a/pkgs/mypackage/default.nix
+++ b/pkgs/mypackage/default.nix
@@ -5,6 +5,7 @@
   nativeBuildInputs = [
     pkg-config
+    cmake
   ];
EOF

ekapkgs-update apply mypackage --patch fix.patch --resume
```

#### Incompatible Patches

```bash
# Error in log: "Hunk #1 FAILED"

# Fix: Remove outdated patch
cat > fix.patch << 'EOF'
--- a/pkgs/mypackage/default.nix
+++ b/pkgs/mypackage/default.nix
@@ -10,7 +10,6 @@
   patches = [
-    ./patches/outdated.patch
   ];
EOF

ekapkgs-update apply mypackage --patch fix.patch --resume
```

#### Build System Changes

```bash
# New version changed build system (e.g., setuptools -> poetry)

# Fix: Update build function
cat > fix.patch << 'EOF'
--- a/pkgs/mypackage/default.nix
+++ b/pkgs/mypackage/default.nix
@@ -1,7 +1,7 @@
-buildPythonPackage {
+buildPythonPackage {
+  format = "pyproject";

   nativeBuildInputs = [
+    poetry-core
   ];
EOF

ekapkgs-update apply mypackage --patch fix.patch --resume
```

### Patch Failure

**Symptom:**
```
patching file src/main.c
Hunk #1 FAILED at 45.
1 out of 1 hunk FAILED -- saving rejects to file src/main.c.rej
```

**Solutions:**

```bash
# 1. Examine the patch
cd /tmp/ekapkgs-update-worktrees/mypackage
cat patches/fix.patch

# 2. Check upstream changes
git clone https://github.com/owner/repo /tmp/repo
cd /tmp/repo
git diff v2.4.0..v2.5.0 src/main.c

# 3. Options:
# A. Remove patch if applied upstream
# B. Update patch for new code
# C. Regenerate patch from scratch

# Option A: Remove patch
cat > fix.patch << 'EOF'
--- a/pkgs/mypackage/default.nix
+++ b/pkgs/mypackage/default.nix
@@ -10,7 +10,6 @@
   patches = [
-    ./patches/fix.patch
   ];
EOF

ekapkgs-update apply mypackage --patch fix.patch --resume
```

### Test Failure

**Symptom:**
```
Error: passthru.tests.pytest failed
```

**Debug steps:**

```bash
# 1. View test output
ekapkgs-update log mypackage | grep -A 50 "checkPhase"

# 2. Identify failing tests
ekapkgs-update log mypackage | grep -E "(FAIL|ERROR)"

# 3. Check test dependencies
cd /tmp/ekapkgs-update-worktrees/mypackage
nix-build -A mypackage.tests.pytest
```

**Solutions:**

```bash
# Disable specific tests
cat > fix.patch << 'EOF'
--- a/pkgs/mypackage/default.nix
+++ b/pkgs/mypackage/default.nix
@@ -12,6 +12,10 @@
   checkPhase = ''
     pytest
+    # Disable flaky test
+    --deselect tests/test_flaky.py::test_unstable
   '';
EOF

# Or disable all tests temporarily
cat > fix.patch << 'EOF'
--- a/pkgs/mypackage/default.nix
+++ b/pkgs/mypackage/default.nix
@@ -10,7 +10,7 @@
   };

-  doCheck = true;
+  doCheck = false;
EOF

ekapkgs-update apply mypackage --patch fix.patch --resume
```

### Dependency Hash Mismatch

**Symptom:**
```
Error: cargoHash mismatch
  specified: sha256-AAAA...
  got:        sha256-BBBB...
```

**Solutions:**

```bash
# ekapkgs-update updates these automatically, but if it fails:

# 1. Let it compute new hash
ekapkgs-update update mypackage
# (automatically updates cargoHash/vendorHash/npmDepsHash)

# 2. If --src-only was used, run full update
ekapkgs-update update mypackage
# (without --src-only)

# 3. Manual update (if needed)
cd /tmp/ekapkgs-update-worktrees/mypackage
nix-build -A mypackage 2>&1 | grep "got:"
# Copy the "got:" hash to default.nix

ekapkgs-update retry mypackage
```

### Version Not Found

**Symptom:**
```
Error: No compatible version found matching strategy 'minor'
```

**Solutions:**

```bash
# 1. Check available versions
curl -s https://api.github.com/repos/owner/repo/releases | jq '.[].tag_name'

# 2. Try different strategy
ekapkgs-update update mypackage --semver latest

# 3. Specify explicit version
ekapkgs-update update mypackage --version 2.5.0

# 4. Fix version regex if tags are non-standard
ekapkgs-update update mypackage --version-regex 'release-(.*)'
```

## Advanced Debugging

### Manual Build in Worktree

```bash
# Navigate to worktree
cd /tmp/ekapkgs-update-worktrees/mypackage

# Try building
nix-build -A mypackage

# If fails, examine error
# Make changes
vim pkgs/mypackage/default.nix

# Test again
nix-build -A mypackage

# Create patch when working
git diff > /tmp/fix.patch

# Apply and retry
ekapkgs-update apply mypackage --patch /tmp/fix.patch --resume
```

### Nix REPL Debugging

```bash
# Open REPL
nix repl '<nixpkgs>'

# Load package
:l .
pkg = mypackage

# Inspect attributes
pkg.version
pkg.src
pkg.buildInputs
pkg.passthru

# Try building phases
:b pkg.unpackPhase
:b pkg.buildPhase
```

### Binary Diff Debugging

```bash
# Build old version
nix-build '<nixpkgs>' -A mypackage
cp result/bin/mypackage old-binary

# Build new version
cd /tmp/ekapkgs-update-worktrees/mypackage
nix-build -A mypackage
cp result/bin/mypackage new-binary

# Compare
diff <(strings old-binary) <(strings new-binary)
diff <(ldd old-binary) <(ldd new-binary)
```

### Network Debugging

```bash
# Enable verbose Nix output
nix-build -A mypackage --verbose

# Check network access
nix-build -A mypackage --option sandbox false

# Trace downloads
nix-build -A mypackage --log-format internal-json 2>&1 | \
  jq -r 'select(.action == "download")'
```

## Debugging Tools

### Query Database for Patterns

```bash
# Find similar failures
ekapkgs-update query --error-type "BuildFailure" --since-days 30

# Find packages that consistently fail
ekapkgs-update query --since-days 30 | \
  grep "Package:" | sort | uniq -c | sort -rn

# Find patterns by phase
ekapkgs-update query --phase "Build" --since-days 30
```

### Export for Analysis

```bash
# Export multiple failures
for pkg in $(ekapkgs-update query --since-days 1 --status failed | \
             grep "Package:" | awk '{print $2}'); do
  ekapkgs-update export "$pkg" --format markdown > "$pkg-debug.md"
done

# Analyze common patterns
grep -h "^Error:" *-debug.md | sort | uniq -c
```

### Automated Diagnosis

```bash
#!/bin/bash
# diagnose.sh <package>

PKG="$1"

echo "=== Diagnosing $PKG ==="

# Get error type
ERROR=$(ekapkgs-update inspect "$PKG" | grep "Error Type:" | awk '{print $3}')
echo "Error type: $ERROR"

# Get phase
PHASE=$(ekapkgs-update inspect "$PKG" | grep "Phase:" | awk '{print $2}')
echo "Failed phase: $PHASE"

# Suggest solution based on error type
case "$ERROR" in
    "BuildFailure")
        echo "Suggestion: Check build log for missing dependencies"
        ekapkgs-update log "$PKG" | grep "command not found"
        ;;
    "HashMismatch")
        echo "Suggestion: Re-run update to fetch correct hash"
        echo "  ekapkgs-update update $PKG"
        ;;
    "PatchFailure")
        echo "Suggestion: Remove or update outdated patches"
        ekapkgs-update worktrees show "$PKG" | grep "patches/"
        ;;
    "TestFailure")
        echo "Suggestion: Review failed tests, consider disabling"
        ekapkgs-update log "$PKG" | grep -E "(FAIL|ERROR)"
        ;;
esac
```

## Prevention

### Add Tests Before Updating

```nix
# Add basic tests to catch regressions
passthru.tests = {
  version = ...;
  basic-functionality = ...;
};
```

### Use Dry-Run First

```bash
# Always preview updates
ekapkgs-update update mypackage --dry-run

# Check if version makes sense
# Check release notes upstream
```

### Preserve Failures for Learning

```bash
# Always preserve failures during development
ekapkgs-update run --preserve-failures

# Review patterns over time
ekapkgs-update query --since-days 30 --group-by-error
```

## See Also

- [inspect command](../cli/inspect.md) - View failure details
- [retry command](../cli/retry.md) - Retry with fixes
- [export/apply commands](../cli/export-apply.md) - LLM assistance
- [worktrees command](../cli/worktrees.md) - Manage artifacts
