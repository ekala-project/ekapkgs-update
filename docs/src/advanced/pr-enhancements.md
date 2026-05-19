# PR Enhancements and Analysis Features

ekapkgs-update can enrich pull requests with additional analysis and metadata beyond basic version bumps. These optional enhancements help reviewers understand the impact and risks of each update.

## Overview

PR enhancements include:

1. **CVE Analysis** - Security vulnerability checking
2. **Repology Integration** - Cross-distribution version comparison
3. **Directory Diff** - Visual comparison of changes
4. **Rebuild Analysis** - Impact assessment for reverse dependencies
5. **Cachix Integration** - Pre-build and cache results

## Configuration

All enhancements are configured via `PrEnhancementsConfig`:

```nix
# In ekapkgs-update configuration
pr-enhancements = {
  # Enable/disable features
  skip-cve-check = false;        # Default: false (check enabled)
  skip-repology = false;         # Default: false (check enabled)
  directory-diff = true;         # Default: true
  analyze-rebuilds = true;       # Default: true
  max-rebuilds = 100;            # Skip if rebuilds > this
  skip-cachix = false;           # Default: false (push enabled)
  cachix-cache = "my-cache";     # Cache name for Cachix
};
```

### CLI Overrides

Override configuration from the command line:

```bash
ekapkgs-update run \
  --config config.toml \
  --no-cve-check \
  --no-repology \
  --directory-diff \
  --analyze-rebuilds \
  --max-rebuilds 50 \
  --cachix-cache my-cache
```

## CVE Analysis

Analyzes security vulnerabilities for old and new versions using the OSV (Open Source Vulnerabilities) database.

### What It Does

For each update, checks:
1. **Resolved CVEs** - Vulnerabilities fixed by this update
2. **Introduced CVEs** - New vulnerabilities in the newer version
3. **Persistent CVEs** - Vulnerabilities in both versions

### Configuration

```nix
pr-enhancements.skip-cve-check = false;  # Enable (default)
```

Disable for packages where CVE data is unavailable:

```bash
ekapkgs-update run --no-cve-check
```

### Supported Ecosystems

CVE data is available for:

| Ecosystem | Package Types | Data Source |
|-----------|--------------|-------------|
| PyPI | Python packages | OSV + NVD |
| crates.io | Rust packages | OSV + Cargo |
| npm | JavaScript packages | OSV + npm |
| Maven | Java packages | OSV + Maven |
| gem | Ruby packages | OSV + Rubygems |
| Go | Go modules | OSV + Go |

### Example PR Body Enhancement

```markdown
## CVE Analysis

### Resolved (2 CVEs fixed in 2.32.0)
- CVE-2024-1234: Information Disclosure (Medium)
- CVE-2024-5678: Denial of Service (High)

### Introduced (0 new vulnerabilities)

### Persistent (1 CVE in both versions)
- CVE-2023-9999: Low Severity - monitoring

**Recommendation**: Security improvements detected, prioritize review.
```

### Implementation Details

```rust
pub async fn analyze_cve_changes(
    pool: &SqlitePool,
    metadata: &PackageMetadata,
    old_version: &str,
    new_version: &str,
    skip_cve_check: bool,
) -> Result<CveAnalysis> {
    // Detect ecosystem (PyPI, crates.io, etc.)
    // Fetch vulnerability data from OSV API
    // Cache results in database
    // Compare old vs new version CVEs
    // Categorize: resolved, introduced, persistent
}
```

### Caching

CVE data is cached in the database to avoid repeated API calls:

```
cve_cache table:
  ecosystem: "PyPI"
  package_name: "requests"
  version: "2.32.0"
  cached_data: "[{...}]"
  cached_at: "2024-05-15T08:00:00Z"
  expires_at: "2024-05-22T08:00:00Z"  (7 day TTL)
```

Expired entries are cleaned during database initialization.

### Handling Missing Data

If CVE data is unavailable:
- Ecosystem not recognized → Note in logs, continue
- API error → Log warning, skip CVE check for that update
- No vulnerabilities found → Report "0 known CVEs"

## Repology Integration

Compares the proposed version against other distributions to identify anomalies.

### What It Does

Queries Repology API to find:
- Version status across distributions (Debian, Alpine, Fedora, etc.)
- If proposed version is notably behind or ahead
- If new version has issues in other distributions

### Configuration

```nix
pr-enhancements.skip-repology = false;  # Enable (default)
```

Disable for specialized packages:

```bash
ekapkgs-update run --no-repology
```

### Example PR Body Enhancement

```markdown
## Repology Status

Current (2.31.0):
- Debian: 2.31.0 (latest)
- Fedora: 2.31.0 (latest)
- Alpine: 2.31.0 (latest)

Proposed (2.32.0):
- Debian: 2.31.0 (1 version behind)
- Fedora: 2.31.0 (1 version behind)
- Alpine: 2.31.0 (1 version behind)

**Note**: Update ahead of major distributions, monitor for issues.
```

### Implementation Details

```rust
pub async fn check_repology(
    pool: &SqlitePool,
    package_name: &str,
    old_version: &str,
    new_version: &str,
    skip_repology: bool,
) -> Result<RepologyAnalysis> {
    // Query Repology API for package status
    // Get version in each major distribution
    // Compare against proposed version
    // Cache results (24 hour TTL)
}
```

### Caching

Repology data is cached with 24-hour expiry:

```
repology_cache table:
  package_name: "requests"
  cached_data: "{...}"
  cached_at: "2024-05-15T08:00:00Z"
  expires_at: "2024-05-16T08:00:00Z"
```

### When to Trust Results

Repology is informational:
- Use to identify early versions (ahead of other distros)
- Use to find versions blocked in other distros (may indicate issues)
- Don't block updates based solely on Repology
- Other distros may have different release cycles

## Directory Diff

Visually shows all file changes made by the update.

### What It Does

Generates a unified diff showing:
- Which files were modified
- Exact line changes
- Context around modifications

### Configuration

```nix
pr-enhancements.directory-diff = true;  # Enable (default)
```

Disable to reduce PR size for large changes:

```bash
ekapkgs-update run --no-directory-diff
```

### Example PR Body Enhancement

```markdown
## Changes

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
+    hash = "sha256-def456==";
   };

   dependencies = [
```

Reviewers can see exactly what was changed by the updater.

### Implementation Details

```rust
pub async fn generate_directory_diff(
    worktree_path: &Path,
    include_context_lines: usize,
) -> Result<String> {
    // Run: git diff HEAD
    // Return unified diff format
    // Include N lines of context
}
```

### Size Limits

For very large changes (>100 files), consider disabling:

```bash
ekapkgs-update run --no-directory-diff
```

This prevents PR bodies from becoming unwieldy.

## Rebuild Analysis

Estimates how many packages would need to rebuild if this update succeeds.

### What It Does

For each update, determines:
- How many packages depend on this one (reverse dependencies)
- Whether the update changes the hash (triggers rebuild)
- Total rebuild count if update is merged

### Configuration

```nix
pr-enhancements.analyze-rebuilds = true;   # Enable (default)
pr-enhancements.max-rebuilds = 100;        # Skip if > 100 rebuilds
```

Skip updates causing too many rebuilds:

```bash
ekapkgs-update run --analyze-rebuilds --max-rebuilds 50
```

### Example PR Body Enhancement

```markdown
## Rebuild Impact

This update will trigger rebuilds for:
- **Count**: 37 packages
- **Bucket**: Small (11-50)

**High-impact packages**:
- python312: 25 reverse dependencies
- shared-mime-info: 12 reverse dependencies

**Recommendation**: Acceptable rebuild impact, safe to merge.
```

### Implementation Details

```rust
pub async fn analyze_rebuild_count(
    worktree_path: &Path,
    attr_path: &str,
) -> Result<usize> {
    // Build new version
    // Get new output hash
    // Compare with old hash
    // If different, run: nix why-depends
    // Count reverse dependencies
}
```

### Rebuild Buckets

Results are categorized:

| Bucket | Rebuilds | Merge Priority |
|--------|----------|---|
| Small | 0-10 | High - merge quickly |
| Medium | 11-50 | Medium - review carefully |
| Large | 51-100 | Low - coordinate timing |
| Huge | 101+ | Very Low - requires discussion |

### When to Skip

Skip updates with excessive rebuild counts:

```nix
pr-enhancements.max-rebuilds = 20;  # Skip if > 20 rebuilds
```

This prevents cascading rebuilds during maintenance windows.

### Performance Considerations

Rebuild analysis is expensive:
- Requires full build of updated package
- Requires `nix why-depends` query
- Can take 5-30 minutes per package

Can be disabled for speed:

```bash
ekapkgs-update run --no-analyze-rebuilds
```

## Cachix Integration

Pre-builds packages and pushes results to a Cachix cache.

### What It Does

After a successful update:
1. Builds the updated package
2. Extracts all output paths
3. Pushes to configured Cachix cache
4. Reviewers can use `nix build` immediately

### Configuration

```nix
pr-enhancements.skip-cachix = false;
pr-enhancements.cachix-cache = "my-cache";
```

Or via environment variable:

```bash
export CACHIX_CACHE_NAME="my-cache"
ekapkgs-update run --config config.toml
```

### Authentication

Requires Cachix token in environment:

```bash
export CACHIX_AUTH_TOKEN="token_xxx"
ekapkgs-update run
```

### Example Workflow

```bash
# Run with Cachix enabled
ekapkgs-update run \
  --config config.toml \
  --cachix-cache my-cache

# During PR review, builders can use pre-built outputs:
nix build github.com/example/pr#python312Packages.requests
# Uses outputs from Cachix cache instead of rebuilding
```

### Implementation Details

```rust
pub async fn perform_cachix_push(
    worktree_path: &Path,
    attr_path: &str,
    cache_name: &str,
) -> Result<()> {
    // Build package: nix build
    // Extract outputs
    // Push to Cachix: cachix push
    // Log results (not shown in PR)
}
```

### Cost and Storage

Each push has:
- **Time**: 5-30 minutes (includes build)
- **Storage**: 50 MB - 5 GB per package
- **Cost**: If using Cachix paid tier

Disable for cost-sensitive setups:

```bash
ekapkgs-update run --no-cachix
```

## Combining Enhancements

All enhancements work together in the PR body:

```markdown
# Update python312Packages.requests from 2.31.0 to 2.32.0

## Summary
Security release with 2 CVE fixes.

## CVE Analysis
- Resolved: CVE-2024-1234 (Info Disclosure, Medium)
- Introduced: None

## Repology Status
Ahead of Debian (2.31.0) and Fedora (2.31.0)

## Rebuild Impact
37 packages will rebuild (Small category)

## Changes
[27-line unified diff showing exact modifications]

---
Generated by ekapkgs-update. Commit: abc123
```

## Performance Considerations

Enhancement overhead:

| Enhancement | Time | Notes |
|-------------|------|-------|
| CVE Check | 1-5s | Cached, minimal impact |
| Repology | 1-3s | Cached, minimal impact |
| Directory Diff | <1s | Near instant |
| Rebuild Analysis | 5-30m | Most expensive, optional |
| Cachix Push | 5-30m | Most expensive, optional |

For fastest updates, disable expensive checks:

```bash
ekapkgs-update run \
  --no-analyze-rebuilds \
  --no-cachix \
  --no-directory-diff
```

This reduces per-package time to ~5-10 seconds.

## Error Handling

If an enhancement fails:
- **CVE check**: Logs warning, continues without CVE section
- **Repology**: Logs warning, continues without Repology section
- **Directory diff**: Logs warning, continues (usually doesn't fail)
- **Rebuild analysis**: Fails the update if enabled and required
- **Cachix**: Logs warning, doesn't fail the PR creation

The system prioritizes successful PR creation over perfect enhancement data.

## Disabling All Enhancements

For fastest, minimal updates:

```bash
ekapkgs-update run \
  --config config.toml \
  --no-cve-check \
  --no-repology \
  --no-directory-diff \
  --no-analyze-rebuilds \
  --no-cachix
```

This creates minimal PRs with just the version bump.

## Best Practices

1. **Use rebuilds analysis** for high-impact packages (e.g., Python, Rust base libraries)
2. **Use CVE checks** for security-sensitive packages
3. **Use Cachix** if you have the bandwidth and budget
4. **Disable Repology** for highly specialized packages with custom versioning
5. **Batch expensive checks** during off-peak hours
6. **Cache aggressively** - CVE and Repology data is heavily reused

## Related Topics

- [PR Enhancements Configuration](../configuration.md)
- [Version Selection](./version-selection.md) - Affects version change type reporting
- [Backoff Strategy](./backoff.md) - Rebuild analysis affects backoff calculations
