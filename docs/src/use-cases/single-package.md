# Updating Single Packages

This guide covers common scenarios for updating individual packages with the `update` command.

## Basic Update

### Simple Version Bump

```bash
# Update to latest version
ekapkgs-update update hello

# Check what would be updated (dry-run)
ekapkgs-update update hello --dry-run
```

**What happens:**
1. Evaluates package to get current version
2. Fetches latest version from upstream (GitHub/GitLab/PyPI)
3. Updates version in Nix file
4. Recomputes source hash
5. Updates dependency hashes (cargo, npm, vendor, etc.)
6. Builds package to verify correctness

### Update with Commit

```bash
# Update and create git commit
ekapkgs-update update terraform --commit
```

**Commit message format:**
```
terraform: 1.6.0 -> 1.7.0
```

### Update with PR Creation

```bash
# Update, commit, and create pull request
ekapkgs-update update nodejs --create-pr
```

**Requires:**
- `GITHUB_TOKEN` environment variable
- Git repository with configured remotes
- Push access to fork

**Creates:**
- Branch: `update/nodejs-20.11.0`
- Commit with version bump
- Pull request against upstream

## Version Control

### Conservative Updates (Semver)

```bash
# Only minor version updates (1.6.x -> 1.7.y, not 2.0.0)
ekapkgs-update update terraform --semver minor

# Only patch updates (1.6.3 -> 1.6.5, not 1.7.0)
ekapkgs-update update kubernetes --semver patch

# Any version (major updates allowed) - this is the default
ekapkgs-update update mypackage --semver latest
```

**Semver strategies:**

| Current | `latest`/`major` | `minor` | `patch` |
|---------|------------------|---------|---------|
| 1.2.3   | Any newer        | 1.x.x   | 1.2.x   |
| 2.5.1   | Any newer        | 2.x.x   | 2.5.x   |

### Explicit Version

```bash
# Update to specific version
ekapkgs-update update nodejs --version 20.11.0

# Update to tagged version
ekapkgs-update update ripgrep --version 14.1.0

# Downgrade to older version
ekapkgs-update update terraform --version 1.6.0
```

Use cases:
- Pin to known-good version
- Test specific version
- Downgrade after problematic update
- Update to pre-release

## Testing Updates

### Run Package Tests

```bash
# Update and run passthru.tests
ekapkgs-update update gcc --run-passthru-tests
```

**What happens:**
1. Updates package
2. Builds successfully
3. Runs all tests in `passthru.tests`
4. Only succeeds if all tests pass

**Package with tests:**
```nix
{
  mypackage = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.tests = {
      version = pkgs.runCommand "test-version" {} ''
        ${mypackage}/bin/mypackage --version | grep "${version}"
        touch $out
      '';
      simple = pkgs.runCommand "test-simple" {} ''
        ${mypackage}/bin/mypackage --help
        touch $out
      '';
    };
  };
}
```

### Format After Update

```bash
# Update and format with nixfmt
ekapkgs-update update python312Packages.requests --format
```

Ensures consistent code formatting after version string replacements.

## Advanced Scenarios

### Custom Version Extraction

For packages with non-standard tag formats:

```bash
# Tag format: jq-1.6
ekapkgs-update update jq --version-regex 'jq-(.*)'

# Tag format: release-v2.5.1
ekapkgs-update update myapp --version-regex 'release-v(.*)'

# Tag format: 2024.01.15
ekapkgs-update update datepkg --version-regex '(\d{4}\.\d{2}\.\d{2})'
```

**Better approach:** Configure in package:
```nix
{
  jq = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      version-regex = "jq-(.*)";
    };
  };
}
```

### Source-Only Updates

Skip dependency hash updates (faster iteration):

```bash
# Only update source hash, skip cargo/npm/vendor hashes
ekapkgs-update update python312Packages.requests --src-only
```

Use when:
- Dependencies haven't changed
- Debugging hash issues
- Quick testing

### Override File Location

```bash
# Specify exact file to update
ekapkgs-update update mypackage \
  --override-filename pkgs/tools/mypackage/default.nix
```

Useful when `meta.position` points to wrong file.

### Force Update Skipped Packages

```bash
# Update package marked with skip = true
ekapkgs-update update legacy-package --force
```

## Flake Packages

### Basic Flake Update

```bash
# Update flake package
ekapkgs-update update --flake my-package --commit
```

### Specify Flake Output

```bash
# Update for specific system
ekapkgs-update update --flake --flake-output packages.x86_64-linux my-package

# Cross-platform update
ekapkgs-update update --flake --flake-output packages.aarch64-darwin my-package
```

## Multi-Variant Packages

For packages using `mkManyVariants`:

```bash
# Update all variants (default)
ekapkgs-update update elasticsearch --commit

# Update single variant
ekapkgs-update update elasticsearch --variant v7_17 --commit

# Explicitly update all
ekapkgs-update update elasticsearch --all-variants --commit
```

**Multi-variant example:**
```nix
{
  inherit (pkgs.mkManyVariants {
    name = "elasticsearch";
    variants = {
      v7_17 = { version = "7.17.0"; sha256 = "..."; };
      v8_0 = { version = "8.0.0"; sha256 = "..."; };
    };
  }) elasticsearch_7_17 elasticsearch_8_0;
}
```

## Complete Workflows

### Standard Update Flow

```bash
# 1. Dry-run to preview
ekapkgs-update update mypackage --dry-run

# 2. Actual update
ekapkgs-update update mypackage

# 3. Test manually
nix-build -A mypackage
./result/bin/mypackage --version

# 4. Commit if satisfied
ekapkgs-update update mypackage --commit
```

### Update with Full Validation

```bash
# Update with tests, formatting, and PR
ekapkgs-update update python312Packages.requests \
  --run-passthru-tests \
  --format \
  --create-pr
```

### Quick Iteration During Development

```bash
# Fast updates without extra checks
ekapkgs-update update mypackage --src-only

# Full update when ready
ekapkgs-update update mypackage \
  --run-passthru-tests \
  --format \
  --commit
```

### Cross-System Update

```bash
# Update for Linux
ekapkgs-update update mypackage --system x86_64-linux --commit

# Update for macOS
ekapkgs-update update mypackage --system aarch64-darwin --commit
```

## Troubleshooting

### Update Fails: Hash Mismatch

```bash
$ ekapkgs-update update mypackage
Error: hash mismatch in fixed-output derivation

# Try different version
ekapkgs-update update mypackage --version 2.5.0

# Or check upstream for tarball changes
curl -L https://github.com/owner/repo/archive/v2.6.0.tar.gz | sha256sum
```

### Update Fails: Build Error

```bash
$ ekapkgs-update update mypackage
Error: builder failed with exit code 2

# Preserve failure for debugging
ekapkgs-update update mypackage --preserve-failures

# Inspect failure
ekapkgs-update log mypackage
ekapkgs-update inspect mypackage
```

### Update Fails: Patch Application

```bash
$ ekapkgs-update update mypackage
Error: patch application failed

# Package may have outdated patches
# Manually remove in worktree and retry
cd /tmp/ekapkgs-update-worktrees/mypackage
# Edit default.nix to remove patch
ekapkgs-update retry mypackage
```

### No Compatible Version Found

```bash
$ ekapkgs-update update mypackage --semver minor
Error: No compatible version found

# Try different strategy
ekapkgs-update update mypackage --semver latest

# Or specify explicit version
ekapkgs-update update mypackage --version 2.6.0
```

## Best Practices

### Before Updating

- Check if package has custom update script
- Review package's passthru.ekapkgs-update configuration
- Check recent upstream changes/release notes

### During Update

- Use `--dry-run` first for unfamiliar packages
- Use `--run-passthru-tests` for critical packages
- Use `--format` to maintain code style

### After Update

- Build and test package manually
- Check for warnings in build log
- Review git diff before committing
- Test package functionality, not just `--version`

## See Also

- [update command](../cli/update.md) - Full command reference
- [Batch Updates](./batch-updates.md) - Multiple packages at once
- [Testing](./testing.md) - Testing strategies
- [Debugging](./debugging.md) - Troubleshooting failures
