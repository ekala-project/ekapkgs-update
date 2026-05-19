# Failure Preservation and Artifact Management

When package updates fail, ekapkgs-update can preserve the complete failure context—including worktrees, logs, diffs, and error details—for manual inspection, debugging, and potential remediation using LLM-assisted tools.

## Overview

The failure preservation system:

1. **Captures full context** when an update fails at any phase
2. **Stores artifacts** in an organized cache directory
3. **Generates metadata** for analysis (diffs, error context, package metadata)
4. **Enables remediation** via export/apply workflow
5. **Manages cleanup** of old artifacts to prevent disk bloat

## Artifact Structure

Failed update artifacts are stored in:

```
~/.cache/ekapkgs-update/failed/
  {session_id}/
    {normalized_attr_path}/
      worktree/              # Complete copy of the failed git worktree
      build.log              # Build output (if available)
      test-output.log        # Test output (if available)
      changes.diff           # git diff of uncommitted changes
      metadata.json          # Package metadata (versions, ecosystem, etc.)
      error-context.json     # Error details in LLM-friendly format
      manifest.json          # Index and manifest of all artifacts
```

### Path Normalization

Attribute paths are normalized when creating artifact directories by replacing `.` and `/` with `_`:

```
python312Packages.requests      → python312Packages_requests
python3Packages/numpy/full      → python3Packages_numpy_full
my.deeply.nested.package.name   → my_deeply_nested_package_name
```

## Preserved Artifacts

### worktree/

A complete recursive copy of the git worktree where the update failed.

**Purpose**: Allows manual inspection of:
- Applied patches and file modifications
- Package metadata and configuration
- Build system artifacts
- Any intermediate state at the point of failure

**Size**: Typically 10-500 MB depending on the package

**Usage**:
```bash
cd ~/.cache/ekapkgs-update/failed/{session_id}/{attr_path}/worktree
git diff HEAD                  # See what was changed
git log -1                     # See the last update attempt
ls -la                         # Inspect package structure
```

### build.log

Complete output from the failed build attempt.

**Contents**:
- Compiler errors and warnings
- Linker errors
- Test failures
- Build system diagnostics
- Full stderr and stdout

**Size**: Typically 100 KB - 10 MB

**Usage**:
```bash
grep -i "error\|failed" ~/.cache/ekapkgs-update/failed/{session_id}/{attr_path}/build.log
```

### test-output.log

Output from test/check phases (if they ran before failure).

**Contents**:
- Test framework output
- Individual test results
- Test assertions and backtraces
- Coverage reports (if generated)

**Size**: Typically 50 KB - 5 MB

### changes.diff

Git diff showing all uncommitted changes made during the update.

**Format**: Unified diff format
- Lines prefixed with `-` are removals
- Lines prefixed with `+` are additions
- Context lines shown with 3 lines before/after

**Usage**:
```bash
cat ~/.cache/ekapkgs-update/failed/{session_id}/{attr_path}/changes.diff | less
```

**Example**:
```diff
diff --git a/pkgs/python-modules/requests/default.nix b/pkgs/python-modules/requests/default.nix
index abc123..def456 100644
--- a/pkgs/python-modules/requests/default.nix
+++ b/pkgs/python-modules/requests/default.nix
@@ -5,10 +5,10 @@
   pname = "requests";
-  version = "2.31.0";
+  version = "2.32.0";

   src = fetchPypi {
     inherit pname version;
-    hash = "sha256-abc123==";
+    hash = "sha256-def456==";  # NEW (needs verification)
   };
```

### metadata.json

Package metadata extracted from the failed worktree.

**Contents**:
```json
{
  "pname": "requests",
  "version": "2.32.0",
  "src_url": "https://github.com/psf/requests",
  "ecosystem": "PyPI",
  "build_system": "setuptools",
  "homepage": "https://requests.readthedocs.io/",
  "maintainers": ["alice", "bob"],
  "meta": {
    "description": "A simple HTTP library for Python",
    "license": "Apache-2.0"
  }
}
```

**Usage**: Helps understand package context without examining the worktree

### error-context.json

Machine-readable error details optimized for LLM analysis.

**Structure**:
```json
{
  "error_type": "BuildFailure",
  "error_message": "error: linker `cc` not found\ncheck...",
  "phase": "build",
  "old_version": "2.31.0",
  "new_version": "2.32.0",
  "version_change_type": "patch",
  "affected_packages_downstream": 15,
  "suggested_actions": [
    "Verify hash is correct",
    "Check for new dependencies",
    "Run tests locally",
    "Review upstream changelog"
  ]
}
```

**Purpose**:
- Provides structured context to LLMs
- Enables automated error classification
- Suggests remediation steps
- Tracks version change severity

### manifest.json

Index of all artifacts for a failed update.

**Contents**:
```json
{
  "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "attr_path": "python312Packages.requests",
  "failed_phase": "build",
  "timestamp": "2024-05-15T10:30:00Z",
  "worktree_path": "/home/user/.cache/ekapkgs-update/failed/.../worktree",
  "build_log_path": "/home/user/.cache/ekapkgs-update/failed/.../build.log",
  "test_output_path": null,
  "diff_path": "/home/user/.cache/ekapkgs-update/failed/.../changes.diff",
  "metadata_path": "/home/user/.cache/ekapkgs-update/failed/.../metadata.json",
  "error_context_path": "/home/user/.cache/ekapkgs-update/failed/.../error-context.json"
}
```

**Usage**: Quick reference for all artifact locations

## Worktree Management

Preserved worktrees are full git clones in a failed state. They remain until explicitly cleaned up.

### Inspecting a Worktree

```bash
# Find the artifact directory
ls ~/.cache/ekapkgs-update/failed/*/*/manifest.json

# Read the manifest to locate the worktree
cat ~/.cache/ekapkgs-update/failed/{session_id}/{attr_path}/manifest.json

# Navigate to the worktree
cd ~/.cache/ekapkgs-update/failed/{session_id}/{attr_path}/worktree

# See what was changed
git status
git diff HEAD

# Check the commit history
git log --oneline -5

# Examine the actual file modifications
cat default.nix
```

### Repairing a Worktree

If you find the issue and fix it locally:

```bash
cd ~/.cache/ekapkgs-update/failed/{session_id}/{attr_path}/worktree

# Make repairs
vim default.nix
nix build .#python312Packages.requests  # Test locally

# Export the fix back to your package repository
cp default.nix /path/to/your/nixpkgs/pkgs/...
```

### Using Worktrees with Export/Apply

Export the failure context and use ekapkgs-update's LLM integration to suggest fixes:

```bash
ekapkgs-update export \
  ~/.cache/ekapkgs-update/failed/{session_id}/{attr_path}/ \
  --format llm > failure-context.json

# Send to Claude, GPT, or local LLM for analysis and fix suggestions
cat failure-context.json | claude-api

# Apply suggested fixes
ekapkgs-update apply \
  /path/to/your/nixpkgs \
  < suggested-fixes.json
```

## Cleanup Policies

### Automatic Cleanup

Expired cache entries (CVE, Repology) are cleaned during database initialization. Failed artifacts are NOT automatically cleaned.

### Manual Cleanup

Remove failures older than 30 days:

```bash
# List old failures (this is informational)
find ~/.cache/ekapkgs-update/failed -type d -mtime +30

# Remove them
find ~/.cache/ekapkgs-update/failed -type d -mtime +30 -exec rm -rf {} +
```

### Programmatic Cleanup

The `cleanup_old_failures()` function removes artifacts older than a threshold:

```rust
use ekapkgs_update::commands::run::preservation::cleanup_old_failures;

let removed = cleanup_old_failures(30).await?;  // Remove artifacts >30 days old
println!("Removed {} artifact sets", removed);
```

### Storage Estimates

A single failed update preserves approximately:
- **Small package**: 50-100 MB
- **Medium package**: 200-500 MB
- **Large package (e.g., rustc)**: 1-10 GB

With many failed updates, cleanup is recommended:

```bash
# Check disk usage
du -sh ~/.cache/ekapkgs-update/failed

# Aggressive cleanup (keep last 7 days)
find ~/.cache/ekapkgs-update/failed -type d -mtime +7 -exec rm -rf {} +

# Moderate cleanup (keep last 30 days)
find ~/.cache/ekapkgs-update/failed -type d -mtime +30 -exec rm -rf {} +
```

## Phase Tracking

The database tracks which phase failed via `update_phases` table:

| Phase | Description | Typical Failure Reasons |
|-------|-------------|------------------------|
| `fetch` | Download source code | Network errors, unavailable source |
| `hash_verification` | Validate source hash | Wrong hash, corrupted download |
| `format` | Apply formatting/patching | Patch conflicts, format errors |
| `build` | Compile/build package | Missing dependencies, compilation errors |
| `test` | Run test suite | Test failures, incompatibilities |
| `publish` | Create and push PR | Git/GitHub API errors |

Each phase has a corresponding artifact directory if it fails.

## Error Context Format

The `error-context.json` is structured for programmatic analysis:

```json
{
  "error_type": "string",          // Error classification
  "error_message": "string",       // Full error text
  "error_summary": "string",       // 1-2 line summary
  "phase": "string",               // Which phase failed
  "old_version": "string",
  "new_version": "string",
  "version_change_type": "string", // "major" | "minor" | "patch"
  "affected_packages_downstream": number,
  "suggested_actions": ["string"],
  "logs": {
    "build_log": "string",         // Tail of build log
    "test_output": "string",       // Tail of test output
    "stderr": "string"
  }
}
```

## Integration with LLM Tools

The preservation system is designed to work with `ekapkgs-update export` and `ekapkgs-update apply` for LLM-assisted remediation:

### Workflow

1. **Failure occurs** → artifacts automatically preserved
2. **Export context** → `ekapkgs-update export` packages artifacts as JSON
3. **Send to LLM** → Claude, GPT, or local model analyzes
4. **Get suggestions** → LLM provides fix recommendations
5. **Apply fixes** → `ekapkgs-update apply` integrates suggested changes

### Example

```bash
# Export failure
session_id="a1b2c3d4-e5f6-7890-abcd-ef1234567890"
attr_path="python312Packages.requests"

ekapkgs-update export \
  ~/.cache/ekapkgs-update/failed/$session_id/$attr_path \
  --format llm > failure.json

# Analyze with Claude
cat failure.json | python3 -c "
import sys, json
from anthropic import Anthropic

context = json.load(sys.stdin)
client = Anthropic()
response = client.messages.create(
    model='claude-3-5-sonnet-20241022',
    max_tokens=1024,
    messages=[{
        'role': 'user',
        'content': f'Why did this Nix package update fail? {context}'
    }]
)
print(response.content[0].text)
"

# Get structured fix suggestions
ekapkgs-update export \
  ~/.cache/ekapkgs-update/failed/$session_id/$attr_path \
  --format llm | \
  jq '.error_context.suggested_actions[]'
```

## Best Practices

1. **Regular inspection**: Check failed artifacts periodically to identify systemic issues
2. **Archive important failures**: Copy critical artifacts before cleanup
3. **Validate fixes**: Always test locally before applying suggestions from LLMs
4. **Clean aggressively**: Large worktrees consume significant disk space
5. **Version control**: Keep your Nix repo synchronized before applying auto-fixes
6. **Monitor trends**: Track which packages fail most often

## Troubleshooting

### Artifacts not being preserved

Check that the `--preserve-failures` flag is enabled:

```bash
ekapkgs-update run \
  --config config.toml \
  --preserve-failures
```

### Worktree disk space growing

List largest failed artifacts:

```bash
du -sh ~/.cache/ekapkgs-update/failed/*/* | sort -hr | head -10
```

Clean up aggressively:

```bash
rm -rf ~/.cache/ekapkgs-update/failed
```

### Cannot read error context

Verify JSON is well-formed:

```bash
jq . ~/.cache/ekapkgs-update/failed/{session}/{attr}/error-context.json
```

If parsing fails, the error may have contained invalid JSON. Check the raw error log instead:

```bash
cat ~/.cache/ekapkgs-update/failed/{session}/{attr}/build.log
```

## Related Topics

- [Database Schema](./database.md) - How failures are tracked in `update_logs`
- [Security Considerations](./security.md) - Protecting sensitive data in artifacts
- [Debugging Failures](../use-cases/debugging.md) - Step-by-step debugging guide
