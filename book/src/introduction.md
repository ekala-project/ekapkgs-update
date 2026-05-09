# Introduction

`ekapkgs-update` is an automated package update tool for Ekapkgs and Nixpkgs repositories. It streamlines the process of keeping packages up-to-date by automatically detecting new upstream versions, updating package definitions, and optionally creating pull requests.

## Features

- **Automatic Version Detection**: Supports GitHub, GitLab, and PyPI upstream sources
- **Intelligent Update Strategies**: Control update behavior with semver strategies (latest, major, minor, patch)
- **Daemon Mode**: Continuously monitor and update multiple packages
- **Manual Updates**: Update specific packages on-demand
- **Pull Request Automation**: Automatically create PRs with detailed changelogs
- **Per-Package Configuration**: Control update behavior via `passthru.ekapkgs-update` attributes
- **Variant Support**: Special handling for `mkManyVariants` packages
- **CVE Checking**: Automatic vulnerability scanning via OSV database
- **Repology Integration**: Cross-distribution version verification
- **Build Verification**: Test packages before committing updates

## Why ekapkgs-update?

Managing package updates in a large repository is time-consuming and error-prone. `ekapkgs-update` automates:

- **Version monitoring**: Automatically detects when new versions are released
- **Source updates**: Updates `fetchFromGitHub`, `fetchFromGitLab`, `fetchPypi`, etc.
- **Hash updates**: Automatically updates output hashes and dependency hashes
- **Testing**: Optionally runs `passthru.tests` to verify updates
- **Documentation**: Generates detailed PR descriptions with changelogs and CVE information

## Use Cases

### For Package Maintainers

- **Set it and forget it**: Configure update preferences once using `passthru.ekapkgs-update` attributes
- **Conservative updates**: Pin packages to specific version ranges
- **Prerelease tracking**: Track beta/RC versions for cutting-edge packages
- **Custom tag formats**: Handle non-standard version tagging schemes

### For Repository Maintainers

- **Automated bulk updates**: Run daemon mode to keep hundreds of packages up-to-date
- **Quality control**: Built-in CVE checking and build verification
- **Consistent PRs**: Standardized PR format with detailed information
- **Reduced manual work**: Focus on reviewing instead of creating updates

## Quick Example

```nix
# In your package definition
{
  pname = "example-app";
  version = "1.2.3";

  src = fetchFromGitHub {
    owner = "example";
    repo = "app";
    rev = "v${version}";
    hash = "sha256-...";
  };

  # Control update behavior
  passthru.ekapkgs-update = {
    semver-strategy = "minor";  # Only accept minor version updates (1.x.x)
    include-prereleases = false; # Skip beta/RC versions
  };
}
```

Run the updater:

```bash
# Manual update
ekapkgs-update update example-app

# Daemon mode (monitors all packages)
ekapkgs-update run --file ./default.nix
```

## Documentation Structure

- **User Guide**: Learn how to use ekapkgs-update for manual and automated updates
- **Passthru Attributes**: Detailed guide on per-package configuration (EEP-0039)
- **Reference**: CLI commands, configuration options, and troubleshooting
- **Advanced Topics**: Special package types and enhancement features
- **Contributing**: Development setup and architecture overview

## Getting Started

Continue to the [Installation](./installation.md) guide to set up ekapkgs-update, or jump to [Quick Start](./quick-start.md) to see it in action.
