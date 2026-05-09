# Skip Updates

The `skip` attribute disables automatic updates for a package entirely.

## Syntax

```nix
passthru.ekapkgs-update.skip = true;  # or false
```

**Type:** `boolean`
**Default:** `false`
**Implemented:** ✅ Yes

## When to Use

Use `skip = true` for packages that should **never** be automatically updated:

### Pinned Versions

```nix
{
  pname = "nodejs";
  version = "16.20.0";  # LTS version pinned for stability

  passthru.ekapkgs-update.skip = true;
}
```

### Deprecated Packages

```nix
{
  pname = "python27";
  version = "2.7.18";  # End of life, frozen

  passthru.ekapkgs-update.skip = true;
}
```

### Complex Manual Updates

```nix
{
  pname = "chromium";
  # Requires manual testing and security review

  passthru.ekapkgs-update.skip = true;
}
```

### Waiting for Downstream Fixes

```nix
{
  pname = "broken-app";
  version = "1.2.3";
  # Newer versions require patches that aren't ready yet

  passthru.ekapkgs-update.skip = true;
}
```

## Behavior

### Daemon Mode

In daemon mode (`ekapkgs-update run`), packages with `skip = true` are silently ignored:

```bash
$ ekapkgs-update run
# Packages with skip = true won't appear in the update queue
```

Debug log will show:

```
DEBUG myapp: Skipping due to passthru.ekapkgs-update.skip = true
```

### Manual Updates

For manual updates (`ekapkgs-update update`), the skip attribute acts as a safeguard:

```bash
$ ekapkgs-update update myapp
WARNING: Package 'myapp' has passthru.ekapkgs-update.skip = true
Error: Update skipped. Use --force to override this setting.
```

#### Override with --force

```bash
$ ekapkgs-update update myapp --force
WARNING: Package 'myapp' has passthru.ekapkgs-update.skip = true,
         but proceeding due to --force flag
# Update proceeds...
```

## Examples

### Temporarily Freeze Package

```nix
{
  pname = "critical-app";
  version = "2.5.1";

  # Freeze during critical period (e.g., before release)
  passthru.ekapkgs-update.skip = true;

  meta.description = "Critical application - version frozen until testing complete";
}
```

### Variant-Specific Skip

For `mkManyVariants` packages, you can skip specific variants:

```nix
mkManyVariants {
  # ...
  variants = {
    v1_0 = {
      version = "1.0.15";
      # Old LTS branch, no more updates
      passthru.ekapkgs-update.skip = true;
    };
    v2_0 = {
      version = "2.0.5";
      # Current stable, allow updates
      passthru.ekapkgs-update.skip = false;
    };
  };
}
```

### Conditional Skip

You can use Nix expressions to conditionally skip:

```nix
{
  pname = "platform-specific";

  passthru.ekapkgs-update.skip = stdenv.isDarwin;  # Skip on macOS only
}
```

## Best Practices

### 1. Document Why

Always add a comment explaining why updates are skipped:

```nix
passthru.ekapkgs-update.skip = true;  # Pinned for API compatibility with legacy clients
```

### 2. Prefer Specific Strategies Over Skip

Instead of completely skipping updates, consider using a more specific strategy:

```nix
# Instead of this:
passthru.ekapkgs-update.skip = true;

# Consider this:
passthru.ekapkgs-update.semver-strategy = "patch";  # Allow security updates
```

### 3. Use Meta Description

Document the skip reason in package metadata:

```nix
{
  passthru.ekapkgs-update.skip = true;

  meta = {
    description = "Legacy version required for compatibility";
    # ...
  };
}
```

### 4. Review Periodically

Packages with `skip = true` should be reviewed periodically:

```nix
# TODO: Remove skip after version 3.0 is stable (review 2026-06)
passthru.ekapkgs-update.skip = true;
```

## Common Mistakes

### ❌ Skipping to Avoid Update Noise

Don't use skip just to reduce update frequency:

```nix
# Bad: Skipping because updates happen too often
passthru.ekapkgs-update.skip = true;
```

Use a more restrictive semver strategy instead:

```nix
# Good: Only accept major updates
passthru.ekapkgs-update.semver-strategy = "major";
```

### ❌ Using Skip Instead of Version Constraints

Don't use skip when you actually want version constraints:

```nix
# Bad: Skipping to stay on version 1.x
passthru.ekapkgs-update.skip = true;
```

Use semver strategy:

```nix
# Good: Stay within 1.x versions
passthru.ekapkgs-update.semver-strategy = "minor";  # If currently on 1.x
```

## Troubleshooting

### Skip Not Working

If the package still gets updated:

1. **Check attribute path**: Verify it's under `passthru.ekapkgs-update`, not just `passthru`

```bash
nix-instantiate --eval -E 'with import ./default.nix {}; myapp.passthru.ekapkgs-update.skip'
# Should output: true
```

2. **Check for --force flag**: Manual updates may be using `--force`

3. **Check version**: Ensure you're using ekapkgs-update version with skip support

### Force Flag Not Working

If `--force` doesn't override skip:

1. Check that you're using the correct command:
   ```bash
   ekapkgs-update update myapp --force  # Correct
   ```

2. Ensure the package is otherwise updateable (has valid upstream source)

## Related

- [Semver Strategy](./semver-strategy.md) - For version constraints instead of complete skip
- [Include Prereleases](./include-prereleases.md) - For controlling prerelease acceptance
- [CLI Reference](../cli-reference.md) - For `--force` flag documentation
