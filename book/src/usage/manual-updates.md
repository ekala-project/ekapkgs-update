# Manual Updates

> **Note:** This chapter is under construction.

Manual updates allow you to update specific packages on-demand.

## Basic Usage

```bash
ekapkgs-update update mypackage
```

## Common Options

```bash
# Specify file
ekapkgs-update update --file ./pkgs/default.nix mypackage

# Create commit
ekapkgs-update update mypackage --commit

# Create PR
ekapkgs-update update mypackage --create-pr

# Force update despite skip flag
ekapkgs-update update mypackage --force
```

See [CLI Reference](../cli-reference.md) for all options.
