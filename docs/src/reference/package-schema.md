# Package Schema

ekapkgs-update expects packages to follow a specific schema to enable automatic updates. This page documents the required and optional attributes.

## Basic Package Structure

### Minimal Package

The minimum required structure for a package to be updatable:

```nix
{
  mypackage = {
    pname = "mypackage";
    version = "1.2.3";

    src = fetchurl {
      url = "https://example.com/releases/${version}/mypackage-${version}.tar.gz";
      sha256 = "...";
    };
  };
}
```

### Complete Package

A fully-featured package with all optional attributes:

```nix
{
  mypackage = rec {
    pname = "mypackage";
    version = "1.2.3";

    src = fetchFromGitHub {
      owner = "example";
      repo = pname;
      rev = "v${version}";
      sha256 = "sha256-...";
    };

    # Dependencies with auto-hash updates
    npmDeps = fetchNpmDeps {
      inherit src;
      hash = "sha256-...";
    };

    # Testing
    passthru = {
      tests = {
        basic = runCommand "test-${pname}" {} ''
          ${mypackage}/bin/mypackage --version
          touch $out
        '';
      };

      # Update control
      ekapkgs-update = {
        skip = false;
        semver = "latest";  # or "major", "minor", "patch"
      };

      # Custom update script
      updateScript = writeShellScript "update.sh" ''
        #!/usr/bin/env bash
        # Custom update logic
      '';
    };
  };
}
```

## Required Attributes

### `pname`
- **Type**: String
- **Required**: Yes
- **Description**: Package name without version

```nix
pname = "hello";
```

### `version`
- **Type**: String
- **Required**: Yes
- **Description**: Current package version

```nix
version = "2.12.1";
```

### `src`
- **Type**: Derivation
- **Required**: Yes
- **Description**: Source fetcher (fetchurl, fetchFromGitHub, etc.)

Must contain a URL that includes `${version}` or `${pname}` for automatic detection:

```nix
# Good - includes version
src = fetchurl {
  url = "https://example.com/${version}/pkg.tar.gz";
  sha256 = "...";
};

# Good - fetchFromGitHub with rev
src = fetchFromGitHub {
  owner = "user";
  repo = "repo";
  rev = "v${version}";
  sha256 = "...";
};

# Bad - hardcoded URL
src = fetchurl {
  url = "https://example.com/pkg-1.2.3.tar.gz";
  sha256 = "...";
};
```

## Optional Attributes

### `passthru.tests`
- **Type**: Attrset of derivations
- **Description**: Tests to run before accepting update

```nix
passthru.tests = {
  basic = runCommand "test-${pname}" {} ''
    ${pkg}/bin/binary --version | grep -q "${version}"
    touch $out
  '';

  smoke = nixosTest {
    name = "${pname}-smoke";
    nodes.machine = { pkgs, ... }: {
      environment.systemPackages = [ pkg ];
    };
    testScript = ''
      machine.succeed("binary --help")
    '';
  };
};
```

When `--run-passthru-tests` is enabled, all tests must pass for the update to be accepted.

### `passthru.ekapkgs-update`
- **Type**: Attrset
- **Description**: Update behavior control

```nix
passthru.ekapkgs-update = {
  # Skip this package entirely
  skip = false;  # or true to skip

  # Version selection strategy
  semver = "latest";  # "major", "minor", "patch", or "latest"
};
```

### `passthru.updateScript`
- **Type**: Derivation (script)
- **Description**: Custom update script

If present and `--ignore-update-script` is not used, this script will be executed instead of the generic update method.

```nix
passthru.updateScript = writeShellScript "update-${pname}.sh" ''
  #!/usr/bin/env bash
  set -euo pipefail

  # Custom update logic
  NEW_VERSION=$(curl -s https://api.example.com/latest)

  # Update version in file
  sed -i "s/version = \".*\"/version = \"$NEW_VERSION\"/" ${toString ./.}/default.nix

  # Update hash
  nix-prefetch-url "https://example.com/$NEW_VERSION/pkg.tar.gz"
'';
```

### Dependency Hashes

ekapkgs-update automatically updates hashes for common dependency fetchers:

#### `npmDeps`
```nix
npmDeps = fetchNpmDeps {
  inherit src;
  hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
};
```

#### `nugetDeps`
```nix
nugetDeps = fetchNuGetDeps {
  inherit pname version;
  hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
};
```

#### `composerDeps`
```nix
composerDeps = fetchComposerDeps {
  inherit src;
  hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
};
```

#### `cargoDeps`
```nix
cargoDeps = rustPlatform.fetchCargoTarball {
  inherit src;
  hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
};
```

Use `--src-only` to skip updating dependency hashes.

## Source Fetchers

### fetchurl

```nix
src = fetchurl {
  url = "https://example.com/releases/${version}/pkg-${version}.tar.gz";
  sha256 = "sha256-...";
};
```

### fetchFromGitHub

```nix
src = fetchFromGitHub {
  owner = "owner";
  repo = "repo";
  rev = "v${version}";  # or rev = version
  sha256 = "sha256-...";
};
```

Supported tag patterns:
- `v${version}` - "v1.2.3"
- `${version}` - "1.2.3"
- `${pname}-${version}` - "myapp-1.2.3"
- Custom via `--version-regex`

### fetchFromGitLab

```nix
src = fetchFromGitLab {
  owner = "owner";
  repo = "repo";
  rev = version;
  sha256 = "sha256-...";
};
```

### fetchFromSourcehut

```nix
src = fetchFromSourcehut {
  owner = "~user";
  repo = "repo";
  rev = version;
  sha256 = "sha256-...";
};
```

### fetchPypi

```nix
src = fetchPypi {
  inherit pname version;
  sha256 = "sha256-...";
};
```

PyPI packages are automatically detected and the latest version is queried from the PyPI API.

## Multi-Variant Packages

For packages with multiple versions maintained simultaneously, use `mkManyVariants`:

```nix
{
  python3 = mkManyVariants {
    inherit (sources) python3;

    variants = {
      v3_11 = {
        version = "3.11.9";
        sha256 = "...";
      };
      v3_12 = {
        version = "3.12.4";
        sha256 = "...";
      };
      v3_13 = {
        version = "3.13.0";
        sha256 = "...";
      };
    };

    builder = variant: buildPythonInterpreter {
      inherit (variant) version;
      src = fetchurl {
        url = "https://www.python.org/ftp/python/${variant.version}/Python-${variant.version}.tar.xz";
        inherit (variant) sha256;
      };
    };
  };
}
```

Update specific variant:
```bash
ekapkgs-update update python3 --variant v3_12
```

Update all variants:
```bash
ekapkgs-update update python3 --all-variants
```

## Metadata

### `meta.position`

ekapkgs-update uses `meta.position` to determine which file to update. This is automatically set by Nix when evaluating packages.

To override:
```bash
ekapkgs-update update mypackage --override-filename /path/to/file.nix
```

### `meta.homepage`

While not required for updates, `meta.homepage` helps with source discovery:

```nix
meta = {
  homepage = "https://github.com/owner/repo";
  description = "Package description";
};
```

## Flake Packages

For flake-based packages:

```bash
ekapkgs-update update mypackage \
  --flake \
  --flake-output "packages.x86_64-linux"
```

Flake packages should follow the same schema but within the flake's `packages` output:

```nix
{
  outputs = { self, nixpkgs }: {
    packages.x86_64-linux.mypackage = {
      pname = "mypackage";
      version = "1.2.3";
      src = ...;
    };
  };
}
```

## Best Practices

### 1. Use `rec` for Self-References

```nix
mypackage = rec {
  pname = "mypackage";
  version = "1.2.3";
  src = fetchurl {
    url = "https://example.com/${pname}-${version}.tar.gz";
    sha256 = "...";
  };
};
```

### 2. Template URLs Properly

```nix
# Good - dynamic
url = "https://example.com/releases/${version}/pkg.tar.gz";

# Bad - hardcoded
url = "https://example.com/releases/1.2.3/pkg.tar.gz";
```

### 3. Use Standard Rev Patterns

```nix
# Good - standard patterns
rev = "v${version}";
rev = version;
rev = "${pname}-${version}";

# Avoid - non-standard patterns may require --version-regex
rev = "release_${version}";
```

### 4. Include Tests

```nix
passthru.tests.version-check = runCommand "test" {} ''
  ${pkg}/bin/binary --version | grep -q "${version}"
  touch $out
'';
```

### 5. Pin Stable Packages

```nix
passthru.ekapkgs-update.skip = true;  # For critical packages
```

## Examples

### Simple CLI Tool

```nix
{
  jq = rec {
    pname = "jq";
    version = "1.7";

    src = fetchFromGitHub {
      owner = "jqlang";
      repo = "jq";
      rev = "jq-${version}";
      sha256 = "sha256-...";
    };

    passthru.tests.basic = runCommand "test-jq" {} ''
      echo '{"test": 123}' | ${jq}/bin/jq .test | grep -q 123
      touch $out
    '';
  };
}
```

### Python Package with Dependencies

```nix
{
  python312Packages.requests = rec {
    pname = "requests";
    version = "2.31.0";

    src = fetchPypi {
      inherit pname version;
      sha256 = "sha256-...";
    };

    passthru = {
      tests.import = python312.pkgs.callPackage ./tests.nix { };

      ekapkgs-update.semver = "major";  # Only update major versions
    };
  };
}
```

### Node.js Package with npm Dependencies

```nix
{
  vscode = rec {
    pname = "vscode";
    version = "1.85.0";

    src = fetchFromGitHub {
      owner = "microsoft";
      repo = "vscode";
      rev = version;
      sha256 = "sha256-...";
    };

    npmDeps = fetchNpmDeps {
      inherit src;
      hash = "sha256-...";
    };

    passthru.ekapkgs-update.skip = false;
  };
}
```

## Troubleshooting

### Package Not Detected

Ensure:
- `pname` and `version` are set
- `src` contains a fetcher
- URL includes version placeholder
- File is evaluated by `nix-eval-jobs`

### Hash Update Failed

- Check that src URL is templated correctly
- Verify network connectivity
- Try `--src-only` to skip dependency hashes

### Custom Version Format

Use `--version-regex`:
```bash
ekapkgs-update update jq --version-regex 'jq-(.*)'
```

## Next Steps

- [update command](../cli/update.md) - Update single packages
- [run command](../cli/run.md) - Batch update workflows
- [Configuration](../configuration.md) - Repository setup
