# update - Single Package Updates

The `update` command updates a single package to a new version. It's ideal for manual updates, testing, and precise version control.

## Synopsis

```bash
ekapkgs-update update [OPTIONS] <ATTR_PATH>
```

## Description

The `update` command performs a surgical update of one package:

1. Evaluates the package at the given attribute path
2. Determines the upstream source (GitHub, GitLab, PyPI, etc.)
3. Fetches available versions according to semver strategy
4. Updates the Nix file with the new version
5. Recomputes all necessary hashes (source, cargo, npm, vendor, etc.)
6. Optionally builds, tests, commits, and creates a PR

## Arguments

### `<ATTR_PATH>`
Attribute path of the package to update.

```bash
# Top-level package
ekapkgs-update update hello

# Nested package
ekapkgs-update update python312Packages.requests

# Deep nesting
ekapkgs-update update haskellPackages.pandoc
```

## Options

### File Configuration

#### `--file <FILE>` (short: `-f`)
Nix file to evaluate.

**Default:** `default.nix`

```bash
ekapkgs-update update --file pkgs/default.nix mypackage
```

### Version Selection

#### `--semver <STRATEGY>`
Version selection strategy for determining which updates to accept.

**Default:** `latest`

**Strategies:**
- `latest` - Accept any newer version (including major bumps)
- `major` - Latest version, allowing major version changes (same as `latest`)
- `minor` - Latest minor version within same major (e.g., `1.2.x` -> `1.3.y`)
- `patch` - Latest patch version within same major.minor (e.g., `1.2.3` -> `1.2.5`)

```bash
# Update to latest version (major version changes allowed)
ekapkgs-update update terraform --semver latest

# Conservative minor update (1.6.x -> 1.7.y, but not 2.0.0)
ekapkgs-update update terraform --semver minor

# Only patch updates (1.6.3 -> 1.6.5, but not 1.7.0)
ekapkgs-update update terraform --semver patch
```

**Example version selection:**

Current version: `2.5.3`

| Strategy | Accepts | Example |
|----------|---------|---------|
| `latest` | Any newer | `2.5.4`, `2.6.0`, `3.0.0` |
| `major`  | Any newer | `2.5.4`, `2.6.0`, `3.0.0` |
| `minor`  | Same major | `2.5.4`, `2.6.0` (NOT `3.0.0`) |
| `patch`  | Same major.minor | `2.5.4`, `2.5.5` (NOT `2.6.0`) |

#### `--version <VERSION>`
Explicit version to update to (overrides `--semver`).

```bash
# Update to specific version
ekapkgs-update update nodejs --version 20.11.0

# Update using tag name
ekapkgs-update update ripgrep --version 14.1.0
```

Use cases:
- Pin to specific version
- Downgrade to older version
- Update to pre-release
- Override auto-detection

#### `--version-regex <REGEX>`
Custom regex to extract version from tags.

```bash
# Tag format: jq-1.6
ekapkgs-update update jq --version-regex 'jq-(.*)'

# Tag format: release-v2.5.1
ekapkgs-update update myapp --version-regex 'release-v(.*)'

# Tag format: 2024.01.15
ekapkgs-update update myapp --version-regex '(\d{4}\.\d{2}\.\d{2})'
```

The regex must have exactly one capture group that extracts the version string.

**Per-package configuration:**
Prefer setting this in the package definition:
```nix
{
  mypackage = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      version-regex = "jq-(.*)";
    };
  };
}
```

### Update Behavior

#### `--ignore-update-script`
Ignore package's `passthru.updateScript` and use generic update method.

```bash
ekapkgs-update update firefox --ignore-update-script
```

Some packages define custom update scripts. This option bypasses them and uses the generic version/hash update logic.

#### `--force`
Force update even if package has `passthru.ekapkgs-update.skip = true`.

```bash
ekapkgs-update update my-package --force
```

Useful for:
- Temporarily overriding skip configuration
- Testing updates on normally-skipped packages
- One-off manual updates

#### `--src-only`
Only update source hash, skip dependency hashes.

```bash
ekapkgs-update update python312Packages.requests --src-only
```

Skips:
- `cargoHash` (Rust)
- `vendorHash` (Go)
- `npmDepsHash` (Node.js)
- `nugetDeps` (.NET)
- `composerDepsHash` (PHP)

Use when:
- Dependencies haven't changed
- Debugging hash issues
- Faster iteration during testing

### Multi-Variant Packages

#### `--variant <VARIANT>`
For `mkManyVariants` packages: update only this specific variant.

```bash
# Only update Python 3.12 variant
ekapkgs-update update mypackage --variant python312

# Only update specific version variant
ekapkgs-update update elasticsearch --variant v7_17
```

#### `--all-variants`
Explicitly update all variants (this is the default behavior).

```bash
ekapkgs-update update mypackage --all-variants
```

**Multi-variant package example:**
```nix
{
  # Defines elasticsearch_7_17, elasticsearch_8_0, etc.
  inherit (pkgs.mkManyVariants {
    name = "elasticsearch";
    variants = {
      v7_17 = { version = "7.17.0"; sha256 = "..."; };
      v8_0 = { version = "8.0.0"; sha256 = "..."; };
    };
  }) elasticsearch_7_17 elasticsearch_8_0;
}
```

### Flake Support

#### `--flake`
Enable flake mode: update a package exposed by a flake.

```bash
ekapkgs-update update --flake my-package
```

#### `--flake-output <OUTPUT>`
Flake output prefix (e.g., 'packages.x86_64-linux').

**Default:** Auto-detected from system

```bash
ekapkgs-update update --flake --flake-output packages.x86_64-linux my-package

# Cross-platform update
ekapkgs-update update --flake --flake-output packages.aarch64-darwin my-package
```

### Testing and Validation

#### `--run-passthru-tests`
Run `passthru.tests` if available before considering update successful.

```bash
ekapkgs-update update gcc --run-passthru-tests
```

Behavior:
- Executes all derivations in `passthru.tests`
- Update fails if any test fails
- Test output is logged for debugging

### Git Integration

#### `--commit`
Create a git commit after successful update.

```bash
ekapkgs-update update nodejs --commit
```

Commit message format:
```
nodejs: 20.10.0 -> 20.11.0
```

#### `--create-pr`
Create a pull request after successful update (implies `--commit`).

```bash
ekapkgs-update update terraform --create-pr
```

**Prerequisites:**
- Git repository with configured remotes
- GitHub token in `GITHUB_TOKEN` environment variable
- Push access to `--fork` remote

**PR creation workflow:**
1. Creates commit
2. Creates branch: `update/<attr-path>-<version>`
3. Pushes branch to fork
4. Creates PR against upstream

#### `--upstream <REMOTE>`
Upstream git remote for pull requests.

**Default:** Auto-detected

```bash
ekapkgs-update update terraform --create-pr --upstream nixpkgs
```

#### `--fork <REMOTE>`
Remote repository to push branches to.

**Default:** `origin`

```bash
ekapkgs-update update terraform --create-pr --fork my-fork
```

### Build and Format

#### `--format`
Format updated files using nixfmt.

```bash
ekapkgs-update update mypackage --format
```

Ensures consistent formatting after version string replacements.

#### `--override-filename <PATH>`
Override the filename to update.

```bash
ekapkgs-update update mypackage --override-filename pkgs/mypackage/default.nix
```

Useful when:
- `meta.position` points to wrong file
- Package is imported from elsewhere
- Manual path specification needed

#### `--system <SYSTEM>`
System to use for evaluation.

**Default:** Current system

```bash
# Evaluate for Linux
ekapkgs-update update --system x86_64-linux mypackage

# Evaluate for macOS
ekapkgs-update update --system aarch64-darwin mypackage
```

### PR Enhancements

#### `--skip-directory-diff`
Skip directory structure diff in PR body.

```bash
ekapkgs-update update terraform --create-pr --skip-directory-diff
```

Speeds up PR creation by skipping directory listing comparison.

## Examples

### Basic Update

```bash
# Simple update to latest version
ekapkgs-update update hello

# Update with commit
ekapkgs-update update hello --commit
```

### Version Control

```bash
# Conservative minor update
ekapkgs-update update terraform --semver minor

# Only patch updates
ekapkgs-update update kubernetes --semver patch

# Update to specific version
ekapkgs-update update nodejs --version 20.11.0
```

### Full Workflow

```bash
# Update, test, format, commit, and create PR
ekapkgs-update update python312Packages.requests \
  --run-passthru-tests \
  --format \
  --create-pr
```

### Flake Package

```bash
# Update flake package
ekapkgs-update update --flake my-package --create-pr
```

### Multi-Variant

```bash
# Update single variant
ekapkgs-update update elasticsearch --variant v7_17 --commit

# Update all variants
ekapkgs-update update elasticsearch --all-variants --commit
```

### Custom Source

```bash
# Custom version extraction
ekapkgs-update update jq \
  --version-regex 'jq-(.*)' \
  --commit
```

### Quick Iteration

```bash
# Fast update without tests or formatting
ekapkgs-update update mypackage --src-only

# Add tests after confirming it works
ekapkgs-update update mypackage --run-passthru-tests --format --commit
```

### Override Defaults

```bash
# Force update of skipped package
ekapkgs-update update legacy-package --force

# Ignore custom update script
ekapkgs-update update firefox --ignore-update-script
```

## Workflow

The `update` command follows this workflow:

1. **Evaluate**: Get package metadata from Nix evaluation
2. **Source Detection**: Identify upstream source (GitHub, GitLab, PyPI, etc.)
3. **Version Fetch**: Query available versions from upstream
4. **Version Selection**: Apply semver strategy or explicit version
5. **File Update**: Rewrite version string in Nix file
6. **Hash Update**: Update source hash (fetchurl, fetchFromGitHub, etc.)
7. **Dependency Hashes**: Update cargo/vendor/npm/nuget hashes (unless `--src-only`)
8. **Build**: Build package to verify correctness
9. **Patch Recovery**: Auto-remove outdated patches that fail to apply
10. **Tests** (optional): Run `passthru.tests`
11. **Format** (optional): Run nixfmt
12. **Commit** (optional): Create git commit
13. **PR** (optional): Push branch and create pull request

## Hash Updates

The following hashes are automatically updated (unless `--src-only`):

| Hash Type | Build System | Example Attribute |
|-----------|--------------|-------------------|
| Source hash | All | `sha256`, `hash` in `fetchurl`, `fetchFromGitHub` |
| `cargoHash` | Rust | `buildRustPackage` |
| `vendorHash` | Go | `buildGoModule` |
| `npmDepsHash` | Node.js | `buildNpmPackage` |
| `nugetDeps` | .NET | `buildDotnetModule` |
| `composerDepsHash` | PHP | `buildComposerPackage` |

## Exit Codes

- `0` - Update succeeded
- `1` - Update failed (check output for error details)
- `2` - Invalid arguments

## See Also

- [run](./run.md) - Batch update multiple packages
- [retry](./retry.md) - Retry failed updates
- [migrate](./migrate.md) - Migrate packages to ekapkgs patterns
- [Single Package Updates Use Case](../use-cases/single-package.md)
- [Version Selection](../advanced/version-selection.md)
