# Daemon Mode

> **Note:** This chapter is under construction.

Daemon mode continuously monitors packages and creates automated updates.

## Basic Usage

```bash
ekapkgs-update run --file ./default.nix
```

## Common Options

```bash
# With PR creation
ekapkgs-update run --file ./default.nix --create-pr

# Skip unstable versions
ekapkgs-update run --skip-unstable

# Limit concurrent updates
ekapkgs-update run --concurrent-updates 4
```

## Database

Daemon mode uses a SQLite database to track update history:

```bash
# Specify database location
ekapkgs-update run --database ./updates.db
```

See [CLI Reference](../cli-reference.md) for all options.
