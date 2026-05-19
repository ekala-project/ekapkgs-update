# migrate - Package Migration Tool

The `migrate` command helps convert packages from nixpkgs patterns to ekapkgs paradigms, ensuring compatibility with automated updates.

## Synopsis

```bash
ekapkgs-update migrate [OPTIONS] <TARGET>
```

## Description

Migrates packages to follow ekapkgs conventions:
- Adds `passthru.ekapkgs-update` metadata
- Structures package definitions for automated updates
- Adds update scripts if appropriate
- Configures version extraction patterns
- Sets up test infrastructure

## Arguments

### `<TARGET>`
Attribute path or file path to migrate.

```bash
# Migrate by attribute path
ekapkgs-update migrate python312Packages.mypackage

# Migrate by file path
ekapkgs-update migrate pkgs/python-modules/mypackage/default.nix
```

## Options

#### `--file <FILE>` (short: `-f`)
Nix file to evaluate (for attribute paths).

**Default:** `default.nix`

```bash
ekapkgs-update migrate --file pkgs/default.nix mypackage
```

## What Gets Migrated

### 1. Basic Metadata

```nix
# Before
{ lib, stdenv, fetchFromGitHub }:

stdenv.mkDerivation rec {
  pname = "mypackage";
  version = "1.0.0";

  src = fetchFromGitHub {
    owner = "foo";
    repo = "mypackage";
    rev = "v${version}";
    sha256 = "...";
  };
}

# After
{ lib, stdenv, fetchFromGitHub }:

stdenv.mkDerivation rec {
  pname = "mypackage";
  version = "1.0.0";

  src = fetchFromGitHub {
    owner = "foo";
    repo = "mypackage";
    rev = "v${version}";
    sha256 = "...";
  };

  passthru.ekapkgs-update = {
    # Auto-detected from src
    enable = true;
  };

  meta = with lib; {
    # ... existing meta ...
    # Maintainers removed if they don't match current user
  };
}
```

### 2. Version Regex

For non-standard version tags:

```nix
# Tag format: jq-1.6
passthru.ekapkgs-update = {
  version-regex = "jq-(.*)";
};

# Tag format: release-v2.5.1
passthru.ekapkgs-update = {
  version-regex = "release-v(.*)";
};
```

### 3. Skip Configuration

For packages that shouldn't auto-update:

```nix
# Don't auto-update
passthru.ekapkgs-update = {
  skip = true;
};

# Or with reason
passthru.ekapkgs-update = {
  skip = true;
  skip-reason = "Requires manual testing";
};
```

### 4. Test Infrastructure

```nix
# Add basic tests
passthru.tests = {
  version = pkgs.runCommand "mypackage-test-version" {} ''
    ${mypackage}/bin/mypackage --version | grep "${version}"
    touch $out
  '';
};
```

### 5. Update Scripts

For complex packages:

```nix
passthru.updateScript = writeShellScript "update.sh" ''
  #!/usr/bin/env nix-shell
  #!nix-shell -i bash -p curl jq nix-prefetch

  # Custom update logic
  VERSION=$(curl -s https://api.github.com/repos/foo/mypackage/releases/latest | jq -r .tag_name)
  # ... update files ...
'';
```

## Examples

### Basic Migration

```bash
# Migrate single package
ekapkgs-update migrate mypackage

# Verify changes
git diff pkgs/mypackage/default.nix
```

### Batch Migration

```bash
# Migrate all Python packages
cd pkgs/python-modules
for dir in */; do
  pkg=$(basename "$dir")
  ekapkgs-update migrate "python312Packages.$pkg"
done
```

### File-Based Migration

```bash
# Migrate by file path
ekapkgs-update migrate pkgs/tools/mypackage/default.nix

# Verify
git diff pkgs/tools/mypackage/default.nix
```

### Review Before Committing

```bash
# Migrate
ekapkgs-update migrate mypackage

# Review changes
git diff

# Test update
ekapkgs-update update mypackage --dry-run

# Commit if satisfied
git add pkgs/mypackage/default.nix
git commit -m "mypackage: migrate to ekapkgs"
```

## Migration Checklist

After running `migrate`, verify:

- [ ] `passthru.ekapkgs-update` is present
- [ ] `version-regex` is correct (if needed)
- [ ] Package builds: `nix-build -A mypackage`
- [ ] Update works: `ekapkgs-update update mypackage --dry-run`
- [ ] Tests pass: `nix-build -A mypackage.tests` (if added)
- [ ] Metadata is correct (homepage, description, license)

## Common Migration Scenarios

### GitHub Package

```nix
# Automatically detected from fetchFromGitHub
src = fetchFromGitHub {
  owner = "foo";
  repo = "mypackage";
  rev = "v${version}";
  sha256 = "...";
};

# Migration adds:
passthru.ekapkgs-update = {
  enable = true;
  # version-regex = "v(.*)";  # If needed
};
```

### GitLab Package

```nix
src = fetchFromGitLab {
  owner = "foo";
  repo = "mypackage";
  rev = version;
  sha256 = "...";
};

# Migration adds:
passthru.ekapkgs-update = {
  enable = true;
};
```

### PyPI Package

```nix
src = fetchPypi {
  inherit pname version;
  sha256 = "...";
};

# Migration adds:
passthru.ekapkgs-update = {
  enable = true;
};
```

### Custom Tag Format

```nix
# Tags: release-2024.01.15
src = fetchFromGitHub {
  owner = "foo";
  repo = "mypackage";
  rev = "release-${version}";
  sha256 = "...";
};

# Migration adds:
passthru.ekapkgs-update = {
  enable = true;
  version-regex = "release-(.*)";
};
```

### Packages with Tests

```nix
# Adds test infrastructure
passthru.tests = {
  version = pkgs.runCommand "${pname}-test-version" {} ''
    ${finalAttrs.finalPackage}/bin/${pname} --version | grep "${version}"
    touch $out
  '';

  simple = pkgs.runCommand "${pname}-test-simple" {} ''
    ${finalAttrs.finalPackage}/bin/${pname} --help > /dev/null
    touch $out
  '';
};
```

## Manual Adjustments

After migration, you may want to manually:

### Add Skip Configuration

```nix
passthru.ekapkgs-update = {
  skip = true;
  skip-reason = "Requires coordinated update with dependent packages";
};
```

### Configure Pre-release Handling

```nix
passthru.ekapkgs-update = {
  include-prereleases = false;  # Default: false
};
```

### Add Custom Tests

```nix
passthru.tests = {
  # Auto-generated basic test
  version = ...;

  # Add your custom tests
  integration = pkgs.nixosTest {
    name = "${pname}-integration";
    nodes.machine = { pkgs, ... }: {
      environment.systemPackages = [ pkgs.mypackage ];
    };
    testScript = ''
      machine.succeed("mypackage --version")
    '';
  };
};
```

### Configure Update Script

```nix
passthru.updateScript = writeShellScript "update.sh" ''
  #!/usr/bin/env nix-shell
  #!nix-shell -i bash -p curl jq common-updater-scripts

  set -euo pipefail

  # Fetch latest version
  version=$(curl -s "https://api.github.com/repos/foo/mypackage/releases/latest" | jq -r '.tag_name' | sed 's/^v//')

  # Update version in file
  update-source-version mypackage "$version"
'';
```

## Integration with Update Workflow

After migration:

```bash
# 1. Migrate
ekapkgs-update migrate mypackage

# 2. Test dry-run
ekapkgs-update update mypackage --dry-run

# 3. Test actual update
ekapkgs-update update mypackage

# 4. Test with commit
ekapkgs-update update mypackage --commit

# 5. Add to batch updates
ekapkgs-update run --file default.nix
```

## Troubleshooting

### Migration Fails

```bash
$ ekapkgs-update migrate mypackage
Error: Could not determine upstream source

# Solution: Package might not be supported
# Check source type:
nix eval -f . mypackage.src.type
# If not GitHub/GitLab/PyPI, manual migration required
```

### Invalid Version Regex

```bash
# After migration, test update
ekapkgs-update update mypackage --dry-run
# Error: Could not extract version from tag

# Solution: Adjust version-regex manually
passthru.ekapkgs-update = {
  version-regex = "custom-pattern-(.*)";
};
```

### Package Won't Build

```bash
# After migration, package fails to build
nix-build -A mypackage
# Error: attribute 'ekapkgs-update' missing

# Solution: Check Nix version (requires passthru support)
nix --version
# Ensure proper passthru syntax
```

## Exit Codes

- `0` - Migration succeeded
- `1` - Migration failed (check error message)
- `2` - Invalid arguments

## See Also

- [update](./update.md) - Test migrated packages
- [Configuration](../configuration.md) - Package configuration guide
- [Package Schema](../reference/package-schema.md) - Detailed schema reference
