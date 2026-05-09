# Quick Start

This guide will get you up and running with ekapkgs-update in minutes. We'll cover the most common workflows and use cases.

## Prerequisites

Before you begin, ensure you have:

- ekapkgs-update installed (see [Installation](./installation.md))
- A Nix file with packages to update (e.g., `default.nix`)
- Optionally, a GitHub/GitLab API token configured

## Your First Update

### Basic Package Update

Update a single package to its latest version:

```bash
ekapkgs-update update mypackage
```

This will:
1. Evaluate the package from `default.nix` (the default file)
2. Fetch the latest upstream version
3. Update the version and hash in the Nix file
4. Verify the update by attempting to build

### Specify a Different File

If your packages are in a different file:

```bash
ekapkgs-update update --file ./pkgs/mypackage.nix mypackage
```

### Dry Run Mode

Preview what would be updated without making changes:

```bash
ekapkgs-update run --dry-run
```

This is useful for:
- Testing your configuration
- Seeing which packages have updates available
- Validating API tokens and connectivity

## Common Workflows

### Workflow 1: Manual Single Package Update

Update a specific package and create a Git commit:

```bash
# Update with automatic commit
ekapkgs-update update mypackage --commit

# Inspect the changes
git show

# Push if satisfied
git push
```

### Workflow 2: Create Pull Request

Update a package and automatically create a pull request:

```bash
ekapkgs-update update mypackage \
  --create-pr \
  --upstream nixpkgs \
  --fork origin
```

This will:
1. Update the package
2. Create a Git commit with a descriptive message
3. Push to your fork
4. Create a pull request on GitHub

**Note:** Requires `gh` CLI to be installed and authenticated.

### Workflow 3: Continuous Updates (Daemon Mode)

Automatically monitor and update all packages:

```bash
ekapkgs-update run \
  --file ./default.nix \
  --create-pr \
  --upstream nixpkgs \
  --fork origin
```

Daemon mode will:
- Check all packages for updates
- Create separate PRs for each updated package
- Track update history in a SQLite database
- Respect per-package configuration via passthru attributes

**Best for:** Maintaining a large package set with automated updates.

### Workflow 4: Version-Specific Update

Update to a specific version instead of the latest:

```bash
# Update to a specific version
ekapkgs-update update mypackage --version 2.5.1

# Update using a version regex
ekapkgs-update update mypackage --version-regex 'v(.*)'
```

### Workflow 5: Conservative Updates (SemVer)

Only update within semantic versioning constraints:

```bash
# Only update to latest patch version (1.2.x)
ekapkgs-update update mypackage --semver patch

# Only update to latest minor version (1.x.y)
ekapkgs-update update mypackage --semver minor

# Only update to latest major version (same as latest)
ekapkgs-update update mypackage --semver major
```

This is useful for:
- Critical packages where breaking changes are risky
- Testing updates incrementally
- Maintaining stability in production environments

## Working with Different Package Types

### GitHub Releases

Most packages using `fetchFromGitHub` work automatically:

```bash
ekapkgs-update update mypackage
```

ekapkgs-update will:
- Fetch the latest GitHub release
- Update `version`, `rev`, and `hash` fields
- Handle both `sha256` and modern `hash` formats

### PyPI Packages

Python packages using `fetchPypi`:

```bash
ekapkgs-update update python3Packages.mypackage
```

ekapkgs-update will:
- Query PyPI for the latest version
- Update version and hash
- Respect prerelease settings

### GitLab Projects

Packages from GitLab:

```bash
ekapkgs-update update myglabpackage
```

Works similarly to GitHub, using GitLab's API for releases.

### Flake Packages

Update packages exposed by Nix flakes:

```bash
ekapkgs-update update --flake .#mypackage
```

**Note:** Flake packages don't currently support passthru attributes.

### mkManyVariants Packages

Update packages with multiple variants:

```bash
# Update all variants (default)
ekapkgs-update update mypackage --all-variants

# Update only a specific variant
ekapkgs-update update mypackage --variant v1_2
```

## Handling Special Cases

### Packages with Skip Flag

If a package has `passthru.ekapkgs-update.skip = true`, force the update:

```bash
ekapkgs-update update mypackage --force
```

You'll see a warning but the update will proceed:

```
WARN Package 'mypackage' has skip=true, but proceeding due to --force flag
```

### Packages with Custom Version Regex

Some packages use non-standard tag formats. Override with:

```bash
# Example: Tags like "jq-1.6" instead of "v1.6"
ekapkgs-update update jq --version-regex 'jq-(.*)'
```

The regex must have exactly one capture group for the version.

### Source-Only Updates

Update only the source hash without updating dependencies:

```bash
ekapkgs-update update mypackage --src-only
```

Useful when:
- Dependency updates are failing
- You want to update incrementally
- Testing if source changes are causing issues

### Packages with Update Scripts

By default, ekapkgs-update runs `passthru.updateScript` if available. To skip:

```bash
ekapkgs-update update mypackage --ignore-update-script
```

## Debugging and Troubleshooting

### Enable Debug Logging

See detailed information about what's happening:

```bash
RUST_LOG=debug ekapkgs-update update mypackage
```

### Check Update Failure Logs

View logs for packages that failed to update:

```bash
ekapkgs-update log mypackage
```

### Verify Package Metadata

Check what ekapkgs-update sees for a package:

```bash
# Run with debug logging to see metadata extraction
RUST_LOG=debug ekapkgs-update update mypackage --dry-run
```

Look for lines like:

```
DEBUG mypackage: Using semver strategy: Latest
DEBUG mypackage: Include prereleases: false
```

## Configuration via Passthru Attributes

For frequently updated packages, configure behavior in the Nix file itself:

```nix
mypackage = stdenv.mkDerivation {
  pname = "mypackage";
  version = "1.2.3";

  src = fetchFromGitHub {
    owner = "example";
    repo = "mypackage";
    rev = "v${version}";
    hash = "sha256-...";
  };

  passthru.ekapkgs-update = {
    # Only update to patch versions
    semver-strategy = "patch";

    # Don't include prereleases
    include-prereleases = false;

    # Custom tag format
    version-regex = "release-(.*)";
  };
};
```

See [Passthru Attributes](./passthru-attributes.md) for complete documentation.

## Common Options Reference

### Update Command

```bash
ekapkgs-update update [OPTIONS] <ATTR_PATH>

Common options:
  -f, --file <FILE>          Nix file to update [default: default.nix]
  --semver <STRATEGY>        Version strategy: latest, major, minor, patch
  --version <VERSION>        Update to specific version
  --version-regex <REGEX>    Custom regex for version extraction
  --force                    Force update even if skip = true
  --commit                   Create git commit
  --create-pr                Create pull request (implies --commit)
  --upstream <REMOTE>        Upstream git remote [default: auto-detect]
  --fork <REMOTE>            Fork remote for PRs [default: origin]
  --dry-run                  Preview without changes (via run command)
```

### Run Command (Daemon Mode)

```bash
ekapkgs-update run [OPTIONS]

Common options:
  -f, --file <FILE>              Nix file to evaluate [default: default.nix]
  -d, --database <PATH>          SQLite database for tracking
  --dry-run                      Check updates without applying
  --concurrent-updates <N>       Max concurrent updates
  --skip-unstable               Skip packages with 'unstable' in version
  --skip-cve                    Skip CVE checking
  --skip-repology               Skip Repology checking
  --upstream <REMOTE>           Upstream git remote
  --fork <REMOTE>               Fork remote [default: origin]
```

## Next Steps

Now that you're familiar with basic usage:

- **[CLI Reference](./cli-reference.md)** - Complete command-line documentation
- **[Passthru Attributes](./passthru-attributes.md)** - Per-package configuration
- **[Usage Guide](./usage.md)** - In-depth usage patterns
- **[Troubleshooting](./troubleshooting.md)** - Solutions to common problems

## Examples Repository

For more real-world examples, see the [ekapkgs repository](https://github.com/ekala-project/ekapkgs), which uses ekapkgs-update to maintain hundreds of packages.
