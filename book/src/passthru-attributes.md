# Passthru Attributes (EEP-0039)

The `passthru.ekapkgs-update` attribute set provides a standardized way for package maintainers to communicate update policies directly in package definitions. This eliminates the need for external configuration files and makes update behavior self-documenting.

## Overview

All update preferences are specified under the `passthru.ekapkgs-update` attribute:

```nix
{
  pname = "myapp";
  version = "1.2.3";

  # ... package definition ...

  passthru.ekapkgs-update = {
    skip = false;                    # Allow updates (default)
    semver-strategy = "minor";       # Only accept minor version updates
    include-prereleases = false;     # Exclude beta/RC versions (default)
    version-regex = "v(.*)";         # Custom tag version extraction
    github-repo = "owner/repo";     # Explicit upstream repo (when URL isn't auto-detected)
    keep = "patch";                  # Retain each patch version as its own entry (mkManyVariants)
    update-script = ./list-versions.sh;  # Custom script that lists upstream versions
  };
}
```

## Why Passthru Attributes?

**Before**: Update configuration was scattered across external files, CLI arguments, and tribal knowledge.

**After**: Everything is declared in the package definition where it belongs:

```nix
# Self-documenting: Anyone reading this knows the update policy
passthru.ekapkgs-update = {
  semver-strategy = "patch";  # This package needs conservative updates
};
```

## Available Attributes

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `skip` | boolean | `false` | Skip all automatic updates |
| `semver-strategy` | string | `"latest"` | Version constraint strategy |
| `include-prereleases` | boolean | `false` | Accept prerelease versions |
| `version-regex` | string | `null` | Custom regex for tag extraction |
| `github-repo` | string | `null` | Explicit GitHub repo (`"owner/repo"`) |
| `keep` | string | `null` | Version retention level for mkManyVariants (`"patch"`, `"minor"`, `"major"`) |
| `update-script` | derivation | `null` | Script that lists available versions (one per line to stdout) |

## Quick Examples

### Conservative Updates

```nix
passthru.ekapkgs-update.semver-strategy = "patch";
# Version 1.2.3 will update to 1.2.4, but not 1.3.0
```

### Prerelease Tracking

```nix
passthru.ekapkgs-update = {
  semver-strategy = "minor";
  include-prereleases = true;
};
# Accepts 1.3.0-beta.1 but not 2.0.0-beta.1
```

### Custom Tag Format

```nix
passthru.ekapkgs-update.version-regex = "jq-(.*)";
# Extracts "1.6" from tag "jq-1.6"
```

### Custom Version Source

```nix
passthru.ekapkgs-update.update-script = writeScript "list-releases" ''
  curl -s https://cmake.org/files/ | pcregrep -o1 'v([0-9.]+)/' | sort -uV
'';
# Lists available versions; ekapkgs-update handles the rest
```

### Pinned Package

```nix
passthru.ekapkgs-update.skip = true;
# Never update automatically
```

## Attribute Details

Each attribute has its own detailed documentation:

- [Skip Updates](./passthru-attributes/skip.md) - Disable automatic updates
- [Semver Strategy](./passthru-attributes/semver-strategy.md) - Control version constraints
- [Include Prereleases](./passthru-attributes/include-prereleases.md) - Track beta/RC versions
- [Version Regex](./passthru-attributes/version-regex.md) - Handle custom tag formats
- [Keep](./passthru-attributes/keep.md) - Retain each version as its own variant entry
- [Update Script](./passthru-attributes/update-script.md) - Custom version-listing for non-standard hosts

## Precedence and Overrides

### Daemon Mode

In daemon mode (`ekapkgs-update run`), passthru attributes are always respected:

```nix
passthru.ekapkgs-update.skip = true;  # Package will be skipped
```

### Manual Updates

For manual updates (`ekapkgs-update update`), passthru attributes can be overridden:

```nix
passthru.ekapkgs-update.skip = true;
```

```bash
# This will fail with a warning
ekapkgs-update update myapp

# This will proceed despite skip = true
ekapkgs-update update myapp --force
```

### CLI Argument Precedence

For `version-regex`, passthru takes precedence over CLI:

```nix
passthru.ekapkgs-update.version-regex = "v(.*)";  # Preferred
```

```bash
ekapkgs-update update myapp --version-regex "release-(.*)"  # Ignored if passthru is set
```

## Implementation Status

This feature is defined in [EEP-0039](https://github.com/ekala-project/eeps/blob/master/eeps/0039-ekapkgs-update-passthru.md).

**Implemented:**
- ✅ `skip` - Fully implemented
- ✅ `semver-strategy` - Fully implemented
- ✅ `include-prereleases` - Fully implemented
- ✅ `version-regex` - Fully implemented
- ✅ `github-repo` - Fully implemented
- ✅ `keep` - Fully implemented
- ✅ `update-script` - Fully implemented

**Future Work:**
- 📋 `follow-branch` - Track specific Git branches (deferred)

## Best Practices

### 1. Start Conservative

For critical packages, start with conservative settings:

```nix
passthru.ekapkgs-update = {
  semver-strategy = "patch";
  include-prereleases = false;
};
```

### 2. Document Non-Standard Choices

If using unusual settings, add a comment:

```nix
passthru.ekapkgs-update = {
  # This package uses CalVer, so we track all updates
  semver-strategy = "latest";
  # Beta versions are actually stable for this project
  include-prereleases = true;
};
```

### 3. Use Skip Sparingly

Only use `skip = true` for packages that truly shouldn't be updated:

```nix
passthru.ekapkgs-update.skip = true;  # Pinned to specific version for compatibility
```

For temporary holds, consider using a more specific strategy instead.

### 4. Test Your Regex

If using `version-regex`, test it against actual tags:

```bash
# Test regex extraction
echo "jq-1.6" | grep -oP 'jq-(.*)'
```

## Migration from External Config

If you previously configured updates via external tools or config files, you can migrate to passthru attributes:

**Old (external config):**
```toml
[packages.myapp]
strategy = "minor"
skip_prereleases = true
```

**New (passthru):**
```nix
passthru.ekapkgs-update = {
  semver-strategy = "minor";
  include-prereleases = false;
};
```

## Troubleshooting

### Updates Not Working as Expected

1. **Check the attribute exists**: `nix-instantiate --eval -E 'with import ./default.nix {}; myapp.passthru.ekapkgs-update.semver-strategy'`

2. **Verify the value**: Make sure strings are properly quoted and boolean values are lowercase

3. **Enable debug logging**: `RUST_LOG=debug ekapkgs-update update myapp`

### Passthru Attributes Ignored

If attributes seem to be ignored:

- Ensure you're using a recent version of ekapkgs-update
- Check that attribute names are exact (case-sensitive, use hyphens not underscores)
- Verify the attribute is under `passthru.ekapkgs-update`, not just `passthru`

## Next Steps

- Learn about each attribute in detail in the sub-chapters
- See [Usage Guide](./usage.md) for practical examples
- Check [CLI Reference](./cli-reference.md) for command-line options
