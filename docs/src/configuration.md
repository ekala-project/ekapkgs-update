# Repository Configuration

Configure packages for automated updates using `passthru.ekapkgs-update` and related attributes.

## Basic Configuration

### Minimal Setup

```nix
{
  mypackage = pkgs.stdenv.mkDerivation rec {
    pname = "mypackage";
    version = "1.0.0";

    src = fetchFromGitHub {
      owner = "example";
      repo = "mypackage";
      rev = "v${version}";
      hash = "sha256-...";
    };

    # Minimal configuration - source detected automatically
    passthru.ekapkgs-update = {
      enable = true;
    };

    meta = with lib; {
      description = "Example package";
      homepage = "https://github.com/example/mypackage";
      license = licenses.mit;
    };
  };
}
```

## Package Schema Requirements

### Required Attributes

For automated updates to work, packages need:

1. **Version attribute**: `version = "1.0.0";`
2. **Source with version reference**: `rev = "v${version}";` or `inherit pname version;`
3. **Supported source type**: `fetchFromGitHub`, `fetchFromGitLab`, `fetchPypi`, etc.

### Supported Source Types

#### GitHub

```nix
src = fetchFromGitHub {
  owner = "example";
  repo = "mypackage";
  rev = "v${version}";       # Must reference ${version}
  hash = "sha256-...";
};

# Automatically detected, no extra config needed
```

#### GitLab

```nix
src = fetchFromGitLab {
  owner = "example";
  repo = "mypackage";
  rev = version;             # Or "v${version}"
  hash = "sha256-...";
};
```

#### PyPI

```nix
src = fetchPypi {
  inherit pname version;
  hash = "sha256-...";
};
```

#### SourceHut

```nix
src = fetchFromSourcehut {
  owner = "~username";
  repo = "mypackage";
  rev = "v${version}";
  hash = "sha256-...";
};
```

## passthru.ekapkgs-update Configuration

### Full Configuration Example

```nix
{
  mypackage = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      # Enable automatic updates (default: auto-detected from src)
      enable = true;

      # Skip this package in automated updates
      skip = false;

      # Reason for skipping (documentation)
      skip-reason = null;

      # Custom regex to extract version from tag
      version-regex = null;  # e.g., "jq-(.*)" for jq-1.6 tags

      # Include pre-release versions
      include-prereleases = false;
    };
  };
}
```

### Skip Updates

```nix
{
  # Skip entirely
  legacy-package = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update.skip = true;
  };

  # Skip with reason (for documentation)
  critical-package = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      skip = true;
      skip-reason = "Requires manual testing and coordination";
    };
  };
}
```

### Custom Version Regex

For non-standard tag formats:

```nix
{
  # Tags like "jq-1.6" instead of "v1.6"
  jq = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      version-regex = "jq-(.*)";
    };
  };

  # Tags like "release-v2.5.1"
  myapp = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      version-regex = "release-v(.*)";
    };
  };

  # Date-based versions "2024.01.15"
  datepkg = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      version-regex = "(\\d{4}\\.\\d{2}\\.\\d{2})";
    };
  };
}
```

### Pre-release Handling

```nix
{
  # Exclude pre-releases (default)
  stable-package = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      include-prereleases = false;
    };
  };

  # Include pre-releases
  bleeding-edge-package = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      include-prereleases = true;
    };
  };
}
```

## Testing Configuration

### passthru.tests

Define tests to run during updates:

```nix
{
  mypackage = pkgs.stdenv.mkDerivation rec {
    pname = "mypackage";
    version = "1.0.0";

    # ... package definition ...

    passthru.tests = {
      # Version check
      version = pkgs.runCommand "${pname}-test-version" {} ''
        ${finalAttrs.finalPackage}/bin/${pname} --version | grep "${version}"
        touch $out
      '';

      # Help text
      help = pkgs.runCommand "${pname}-test-help" {} ''
        ${finalAttrs.finalPackage}/bin/${pname} --help
        touch $out
      '';

      # Basic functionality
      basic = pkgs.runCommand "${pname}-test-basic" {} ''
        echo "test" | ${finalAttrs.finalPackage}/bin/${pname} > output.txt
        grep "expected" output.txt
        touch $out
      '';

      # NixOS integration test
      nixos = pkgs.nixosTest {
        name = "${pname}-nixos-test";
        nodes.machine = { pkgs, ... }: {
          environment.systemPackages = [ pkgs.mypackage ];
        };
        testScript = ''
          machine.succeed("${pname} --version")
        '';
      };
    };
  };
}
```

## Update Scripts

### Custom Update Logic

For complex packages that need custom update logic:

```nix
{
  mypackage = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.updateScript = pkgs.writeShellScript "update.sh" ''
      #!/usr/bin/env nix-shell
      #!nix-shell -i bash -p curl jq common-updater-scripts

      set -euo pipefail

      # Fetch latest version
      VERSION=$(curl -s "https://api.github.com/repos/owner/repo/releases/latest" | \
                jq -r '.tag_name' | sed 's/^v//')

      # Update version in file
      update-source-version mypackage "$VERSION"

      # Custom post-processing
      # ... additional logic ...
    '';
  };
}
```

### Ignore Update Scripts

To use generic update instead of custom script:

```bash
ekapkgs-update update mypackage --ignore-update-script
```

## Multi-Variant Packages

### Using mkManyVariants

```nix
{
  # Define multiple variants of same package
  inherit (pkgs.mkManyVariants {
    name = "elasticsearch";
    inherit pkgs lib;

    baseFn = { version, hash }: pkgs.stdenv.mkDerivation {
      pname = "elasticsearch";
      inherit version;

      src = fetchurl {
        url = "https://artifacts.elastic.co/downloads/elasticsearch/elasticsearch-${version}-linux-x86_64.tar.gz";
        inherit hash;
      };

      # ... rest of package definition ...

      passthru.ekapkgs-update = {
        # Each variant updates independently
        enable = true;
      };
    };

    variants = {
      v7_17 = { version = "7.17.18"; hash = "sha256-..."; };
      v8_11 = { version = "8.11.0"; hash = "sha256-..."; };
      v8_12 = { version = "8.12.0"; hash = "sha256-..."; };
    };
  }) elasticsearch_7_17 elasticsearch_8_11 elasticsearch_8_12;

  # Default to latest
  elasticsearch = elasticsearch_8_12;
}
```

### Updating Variants

```bash
# Update all variants
ekapkgs-update update elasticsearch --all-variants

# Update single variant
ekapkgs-update update elasticsearch --variant v7_17

# Update specific variant package
ekapkgs-update update elasticsearch_7_17
```

## Language-Specific Configurations

### Python Packages

```nix
{
  python312Packages.mypackage = buildPythonPackage rec {
    pname = "mypackage";
    version = "1.0.0";
    format = "pyproject";

    src = fetchPypi {
      inherit pname version;
      hash = "sha256-...";
    };

    nativeBuildInputs = [
      setuptools
      wheel
    ];

    propagatedBuildInputs = [
      requests
      click
    ];

    passthru.tests = {
      imports = pkgs.runCommand "${pname}-test-imports" {
        nativeBuildInputs = [ python312 ];
      } ''
        python -c "import ${pname}; print(${pname}.__version__)"
        touch $out
      '';
    };

    passthru.ekapkgs-update = {
      enable = true;
    };
  };
}
```

### Rust Packages

```nix
{
  myrust = rustPlatform.buildRustPackage rec {
    pname = "myrust";
    version = "1.0.0";

    src = fetchFromGitHub {
      owner = "example";
      repo = "myrust";
      rev = "v${version}";
      hash = "sha256-...";
    };

    cargoHash = "sha256-...";  # Auto-updated by ekapkgs-update

    passthru.tests = {
      version = pkgs.runCommand "${pname}-test" {} ''
        ${finalAttrs.finalPackage}/bin/${pname} --version
        touch $out
      '';
    };

    passthru.ekapkgs-update = {
      enable = true;
    };
  };
}
```

### Go Packages

```nix
{
  mygo = buildGoModule rec {
    pname = "mygo";
    version = "1.0.0";

    src = fetchFromGitHub {
      owner = "example";
      repo = "mygo";
      rev = "v${version}";
      hash = "sha256-...";
    };

    vendorHash = "sha256-...";  # Auto-updated by ekapkgs-update

    passthru.ekapkgs-update = {
      enable = true;
    };
  };
}
```

### Node.js Packages

```nix
{
  mynode = buildNpmPackage rec {
    pname = "mynode";
    version = "1.0.0";

    src = fetchFromGitHub {
      owner = "example";
      repo = "mynode";
      rev = "v${version}";
      hash = "sha256-...";
    };

    npmDepsHash = "sha256-...";  # Auto-updated by ekapkgs-update

    passthru.ekapkgs-update = {
      enable = true;
    };
  };
}
```

## Best Practices

### 1. Always Test After Configuration

```bash
# Test update works
ekapkgs-update update mypackage --dry-run

# Test with actual update
ekapkgs-update update mypackage

# Test with tests enabled
ekapkgs-update update mypackage --run-passthru-tests
```

### 2. Add Meaningful Tests

```nix
# Bad - just checks it runs
passthru.tests.version = pkgs.runCommand "test" {} ''
  ${pkg}/bin/pkg --version
  touch $out
'';

# Good - validates version correctness
passthru.tests.version = pkgs.runCommand "test" {} ''
  ${pkg}/bin/pkg --version | grep "${version}"
  touch $out
'';

# Better - tests actual functionality
passthru.tests.functionality = pkgs.runCommand "test" {} ''
  echo "input" | ${pkg}/bin/pkg > output.txt
  grep "expected output" output.txt
  touch $out
'';
```

### 3. Document Skip Reasons

```nix
passthru.ekapkgs-update = {
  skip = true;
  skip-reason = "Requires manual coordination with module changes";
};
```

### 4. Use Version References

```nix
# Good - version is templated
src = fetchFromGitHub {
  owner = "example";
  repo = "pkg";
  rev = "v${version}";
  hash = "...";
};

# Bad - version is hardcoded
src = fetchFromGitHub {
  owner = "example";
  repo = "pkg";
  rev = "v1.0.0";  # Won't auto-update
  hash = "...";
};
```

## Migrating Existing Packages

Use the migrate command:

```bash
# Migrate single package
ekapkgs-update migrate mypackage

# Review changes
git diff

# Test update
ekapkgs-update update mypackage --dry-run

# Commit if satisfied
git add pkgs/mypackage/default.nix
git commit -m "mypackage: migrate to ekapkgs"
```

## See Also

- [migrate command](./cli/migrate.md) - Auto-migrate packages
- [Package Schema Reference](./reference/package-schema.md) - Detailed schema
- [Single Package Updates](./use-cases/single-package.md) - Update examples
