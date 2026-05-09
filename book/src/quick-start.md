# Quick Start

> **Note:** This chapter is under construction.

## Update a Single Package

```bash
ekapkgs-update update --file ./default.nix mypackage
```

## Run Daemon Mode

```bash
ekapkgs-update run --file ./default.nix
```

## Common Options

```bash
# Create a pull request
ekapkgs-update update mypackage --create-pr

# Use a specific semver strategy
ekapkgs-update update mypackage --semver minor

# Force update a skipped package
ekapkgs-update update mypackage --force
```

For more details, see the [Usage Guide](./usage.md) and [Passthru Attributes](./passthru-attributes.md).
