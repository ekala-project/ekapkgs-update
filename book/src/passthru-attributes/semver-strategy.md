# Semver Strategy

The `semver-strategy` attribute controls which version updates are acceptable based on semantic versioning constraints.

## Syntax

```nix
passthru.ekapkgs-update.semver-strategy = "latest";  # or "major" | "minor" | "patch"
```

**Type:** `string` (enum)
**Default:** `"latest"`
**Implemented:** ✅ Yes

## Strategies

### `latest` (Default)

Accept any newer non-prerelease version:

```nix
passthru.ekapkgs-update.semver-strategy = "latest";
```

**Example:**
- Current: `1.2.3`
- Accepts: `1.2.4`, `1.3.0`, `2.0.0`, `3.0.0`
- Rejects: `1.2.3-beta`, `2.0.0-rc1` (unless `include-prereleases = true`)

**Use case:** Most packages that follow semver and want to stay current

### `major`

Same as `latest` - allows major version updates:

```nix
passthru.ekapkgs-update.semver-strategy = "major";
```

This is an alias for `latest` provided for clarity.

### `minor`

Only update to latest minor version within the same major version:

```nix
passthru.ekapkgs-update.semver-strategy = "minor";
```

**Example:**
- Current: `1.2.3`
- Accepts: `1.2.4`, `1.3.0`, `1.99.0`
- Rejects: `2.0.0`, `3.0.0`

**Use case:** Packages where major version changes introduce breaking changes

### `patch`

Only update to latest patch version within the same major.minor version:

```nix
passthru.ekapkgs-update.semver-strategy = "patch";
```

**Example:**
- Current: `1.2.3`
- Accepts: `1.2.4`, `1.2.5`, `1.2.99`
- Rejects: `1.3.0`, `2.0.0`

**Use case:** Critical packages that need maximum stability

## When to Use Each Strategy

### Use `latest` / `major` for:

✅ Libraries and tools that follow semver strictly
✅ Packages with good backwards compatibility
✅ Development tools where you want latest features
✅ Packages you actively maintain and can quickly fix

```nix
{
  pname = "ripgrep";  # Well-maintained, follows semver
  passthru.ekapkgs-update.semver-strategy = "latest";
}
```

### Use `minor` for:

✅ Packages with potentially breaking major versions
✅ Runtime dependencies of critical applications
✅ Packages with large API surfaces
✅ Languages and compilers

```nix
{
  pname = "nodejs";  # Major versions can break compatibility
  passthru.ekapkgs-update.semver-strategy = "minor";
}
```

### Use `patch` for:

✅ Critical system components
✅ Packages used in production
✅ Stable/LTS versions
✅ Packages with strict compatibility requirements

```nix
{
  pname = "openssl";  # Critical security component
  passthru.ekapkgs-update.semver-strategy = "patch";
}
```

## Examples

### Conservative Library Update

```nix
{
  pname = "critical-lib";
  version = "2.5.3";

  passthru.ekapkgs-update = {
    semver-strategy = "patch";  # Only 2.5.x updates
  };
}
```

### Language Runtime

```nix
{
  pname = "python3";
  version = "3.11.5";

  passthru.ekapkgs-update = {
    semver-strategy = "minor";  # Stay within 3.11.x
  };

  # Allows: 3.11.6, 3.11.7
  # Blocks: 3.12.0, 4.0.0
}
```

### Development Tool

```nix
{
  pname = "rust-analyzer";
  version = "2023-09-04";

  passthru.ekapkgs-update = {
    semver-strategy = "latest";  # Always get latest features
    include-prereleases = true;   # Include nightly builds
  };
}
```

### mkManyVariants with Different Strategies

```nix
mkManyVariants {
  baseName = "python3";
  variants = {
    v3_11 = {
      version = "3.11.5";
      # Patch updates only for stable release
      passthru.ekapkgs-update.semver-strategy = "patch";
    };
    v3_12 = {
      version = "3.12.0";
      # Minor updates for newer release
      passthru.ekapkgs-update.semver-strategy = "minor";
    };
  };
}
```

## Strategy Behavior Details

### Semver Parsing

The tool attempts to parse versions as semver (MAJOR.MINOR.PATCH):

```
1.2.3     → major=1, minor=2, patch=3
v2.0.0    → major=2, minor=0, patch=0
0.15.2    → major=0, minor=15, patch=2
```

For non-semver versions (e.g., `20231015`), the tool uses string comparison:
- `latest`/`major`: Accepts any newer string
- `minor`/`patch`: May not work as expected

### Non-Semver Packages

For packages that don't follow semver:

```nix
{
  pname = "calver-package";
  version = "2023.10.15";  # Calendar versioning

  # Use 'latest' for non-semver
  passthru.ekapkgs-update.semver-strategy = "latest";
}
```

### Combined with include-prereleases

Strategies work together with prerelease filtering:

```nix
passthru.ekapkgs-update = {
  semver-strategy = "minor";
  include-prereleases = true;
};

# Current: 1.2.3
# Accepts: 1.2.4-beta.1, 1.3.0-rc1
# Rejects: 2.0.0-beta.1 (major version change)
```

## Variant-Specific Behavior

For `mkManyVariants` packages, the strategy can be **inferred** from variant names:

```nix
mkManyVariants {
  variants = {
    v1 = { };      # Inferred strategy: minor (1.x.x)
    v1_2 = { };    # Inferred strategy: patch (1.2.x)
    v1_2_3 = { };  # Pinned (no updates)
  };
}
```

Override inference with explicit strategy:

```nix
v1 = {
  passthru.ekapkgs-update.semver-strategy = "latest";  # Override inference
};
```

## Troubleshooting

### Strategy Not Respected

If updates aren't following your strategy:

1. **Check the version format**:
   ```bash
   # Must be parseable as semver
   echo "1.2.3" | grep -P '^\d+\.\d+\.\d+'
   ```

2. **Verify the attribute**:
   ```bash
   nix-instantiate --eval -E 'with import ./default.nix {}; myapp.passthru.ekapkgs-update.semver-strategy'
   # Should output: "minor"
   ```

3. **Check for typos**:
   - ✅ `"minor"` (correct, lowercase, quoted)
   - ❌ `minor` (missing quotes)
   - ❌ `"Minor"` (wrong case)
   - ❌ `"minur"` (typo)

4. **Enable debug logging**:
   ```bash
   RUST_LOG=debug ekapkgs-update update myapp 2>&1 | grep strategy
   # Should show: "Using semver strategy: minor"
   ```

### Unexpected Versions Accepted

If a version is accepted that shouldn't be:

1. **Check prerelease filtering**: Prereleases are excluded by default unless `include-prereleases = true`

2. **Verify current version**: The strategy is relative to the current version
   ```bash
   nix-instantiate --eval -E 'with import ./default.nix {}; myapp.version'
   ```

3. **Check for manual `--version` override**: Explicit `--version` flag bypasses strategy

### No Updates Found

If no compatible updates are found:

1. **Check upstream tags**: Verify there are newer versions with correct format
   ```bash
   gh api repos/owner/repo/tags | jq '.[].name'
   ```

2. **Try a less restrictive strategy**: Temporarily use `latest` to see all available versions

3. **Check version-regex**: If using custom regex, it may be filtering out versions

## Best Practices

### 1. Match Package Stability

```nix
# Stable, well-tested package
passthru.ekapkgs-update.semver-strategy = "latest";

# Beta/experimental package
passthru.ekapkgs-update.semver-strategy = "patch";  # Be conservative
```

### 2. Consider Downstream Impact

```nix
# Widely-used library
passthru.ekapkgs-update.semver-strategy = "minor";  # Avoid surprise breakage

# Leaf package (no dependents)
passthru.ekapkgs-update.semver-strategy = "latest";  # Can update freely
```

### 3. Document Strategy Choice

```nix
passthru.ekapkgs-update = {
  # Staying on v1.x for API stability
  semver-strategy = "minor";
};
```

### 4. Review Strategy Periodically

```nix
passthru.ekapkgs-update = {
  # TODO: Switch to 'latest' after migrating to v2 API (2026-Q2)
  semver-strategy = "minor";
};
```

## Related

- [Include Prereleases](./include-prereleases.md) - Control prerelease acceptance
- [Version Regex](./version-regex.md) - Custom tag parsing
- [Skip](./skip.md) - Disable updates entirely
