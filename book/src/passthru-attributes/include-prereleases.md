# Include Prereleases

The `include-prereleases` attribute controls whether pre-release versions (alpha, beta, RC, etc.) are considered valid update candidates.

## Syntax

```nix
passthru.ekapkgs-update.include-prereleases = true;  # or false
```

**Type:** `boolean`
**Default:** `false` (exclude prereleases)
**Implemented:** ✅ Yes

## Default Behavior

By default, pre-release versions are **excluded**:

```nix
# Default behavior (no attribute needed)
passthru.ekapkgs-update.include-prereleases = false;

# Current version: 1.2.3
# Available tags: 1.2.4, 1.3.0-beta.1, 1.3.0-rc1, 1.3.0
# Accepts: 1.2.4, 1.3.0
# Rejects: 1.3.0-beta.1, 1.3.0-rc1
```

## When to Use

Set `include-prereleases = true` to track development versions:

### Nightly/Beta Channels

```nix
{
  pname = "rust-analyzer";
  version = "2023-09-04";

  passthru.ekapkgs-update = {
    semver-strategy = "latest";
    include-prereleases = true;  # Track nightly builds
  };
}
```

### Early Adopters

```nix
{
  pname = "experimental-tool";

  passthru.ekapkgs-update = {
    include-prereleases = true;  # Get beta features early
  };
}
```

### Projects with Stable Prereleases

Some projects use "beta" or "rc" tags for actually-stable versions:

```nix
{
  pname = "special-project";

  # This project's RC releases are production-ready
  passthru.ekapkgs-update.include-prereleases = true;
}
```

## How Prereleases are Detected

### GitHub Releases

Uses the `prerelease` field from GitHub API:

```json
{
  "tag_name": "v1.3.0-beta.1",
  "prerelease": true
}
```

### GitLab Releases

Uses the `upcoming_release` field from GitLab API:

```json
{
  "tag_name": "v1.3.0-rc1",
  "upcoming_release": true
}
```

### Git Tags

When using tags (no releases), tags are **assumed stable**:

```bash
# Tags without release objects
v1.2.3        # Treated as stable
v1.3.0-beta   # Treated as stable (no metadata available)
```

> **Note:** If a project uses tags only (not releases), all versions are treated as stable. Use `version-regex` to filter if needed.

### PyPI

Uses the `yanked` status as prerelease indicator:

```json
{
  "version": "1.3.0",
  "yanked": true  # Treated as prerelease
}
```

## Interaction with Semver Strategy

Prereleases are filtered **after** semver strategy:

```nix
passthru.ekapkgs-update = {
  semver-strategy = "minor";
  include-prereleases = true;
};

# Current: 1.2.3
# Available: 1.2.4, 1.3.0-beta.1, 2.0.0-beta.1
# Strategy filters to: 1.2.4, 1.3.0-beta.1 (rejects 2.0.0-beta.1)
# Prerelease filter accepts: 1.2.4, 1.3.0-beta.1
# Winner: 1.3.0-beta.1 (newest)
```

## Examples

### Rust Nightly

```nix
{
  pname = "rust";
  version = "1.72.0";

  passthru.ekapkgs-update = {
    semver-strategy = "latest";
    include-prereleases = true;
  };

  # Accepts: 1.73.0-beta.1, 1.73.0-nightly
}
```

### Node.js LTS with Prereleases

```nix
{
  pname = "nodejs";
  version = "18.17.0";

  passthru.ekapkgs-update = {
    semver-strategy = "minor";      # Stay on 18.x
    include-prereleases = true;     # Accept RC versions
  };

  # Accepts: 18.17.1, 18.18.0-rc1
  # Rejects: 20.0.0-beta.1 (major version change)
}
```

### Electron with Stable Betas

```nix
{
  pname = "electron";

  passthru.ekapkgs-update = {
    include-prereleases = true;  # Beta versions are actually stable
  };
}
```

### Mixed Variants

Different prerelease policies for different variants:

```nix
mkManyVariants {
  baseName = "myapp";
  variants = {
    stable = {
      version = "1.0.5";
      passthru.ekapkgs-update.include-prereleases = false;  # Stable only
    };
    beta = {
      version = "1.1.0-beta.2";
      passthru.ekapkgs-update.include-prereleases = true;   # Accept betas
    };
  };
}
```

## Best Practices

### 1. Be Explicit

Even when using the default, consider being explicit for clarity:

```nix
passthru.ekapkgs-update = {
  semver-strategy = "patch";
  include-prereleases = false;  # Explicit: only stable patches
};
```

### 2. Document Prerelease Rationale

```nix
passthru.ekapkgs-update = {
  # This project's RC releases go through extensive testing
  # and are effectively stable. We track them for early bug fixes.
  include-prereleases = true;
};
```

### 3. Combine with Strategy

Always specify a semver strategy when including prereleases:

```nix
# Good: Clear intent
passthru.ekapkgs-update = {
  semver-strategy = "minor";
  include-prereleases = true;
};

# Less clear: What range of prereleases?
passthru.ekapkgs-update.include-prereleases = true;
```

### 4. Test Before Enabling

Before enabling prereleases, check what versions would be selected:

```bash
# Dry run to see what version would be selected
ekapkgs-update update myapp --dry-run
```

## Common Use Cases

### Development Environment

```nix
# devShell.nix
{
  # Use cutting-edge tools in development
  passthru.ekapkgs-update = {
    semver-strategy = "latest";
    include-prereleases = true;
  };
}
```

### Production Environment

```nix
# production.nix
{
  # Only stable versions in production
  passthru.ekapkgs-update = {
    semver-strategy = "patch";
    include-prereleases = false;
  };
}
```

## Troubleshooting

### Prereleases Still Excluded

If prereleases aren't being accepted:

1. **Verify the attribute**:
   ```bash
   nix-instantiate --eval -E 'with import ./default.nix {}; myapp.passthru.ekapkgs-update.include-prereleases'
   # Should output: true
   ```

2. **Check source type**:
   - GitHub/GitLab releases: Prerelease metadata available ✅
   - Git tags only: No metadata, all treated as stable ⚠️
   - PyPI: Uses yanked status ✅

3. **Verify upstream has prerelease flag**:
   ```bash
   gh api repos/owner/repo/releases | jq '.[] | {tag: .tag_name, pre: .prerelease}'
   ```

### Too Many Prereleases

If you're getting unstable versions you don't want:

1. **Use version-regex to filter**:
   ```nix
   passthru.ekapkgs-update = {
     include-prereleases = true;
     version-regex = ".*-rc.*";  # Only RC versions, not alpha/beta
   };
   ```

2. **Disable prereleases**:
   ```nix
   passthru.ekapkgs-update.include-prereleases = false;
   ```

### Git Tags Treated as Stable

If using a repository that tags prereleases but doesn't create releases:

**Problem:** Tags have no prerelease metadata, so `1.0.0-beta` is treated as stable.

**Solutions:**

1. **Use version-regex to exclude**:
   ```nix
   passthru.ekapkgs-update.version-regex = "^[0-9]+\\.[0-9]+\\.[0-9]+$";  # Semver only
   ```

2. **Ask upstream to create releases**: GitHub/GitLab releases include prerelease flags

## Debugging

Enable debug logging to see prerelease filtering:

```bash
RUST_LOG=debug ekapkgs-update update myapp 2>&1 | grep -i prerelease
```

You should see:
```
DEBUG myapp: Including prerelease versions
DEBUG myapp: Skipping release v1.3.0-beta.1 - is_prerelease=true
```

## Related

- [Semver Strategy](./semver-strategy.md) - Control version constraints
- [Version Regex](./version-regex.md) - Filter versions by pattern
- [Skip](./skip.md) - Disable updates entirely
