# CLI Reference

> **Note:** This chapter is under construction.

## Commands

### update

Update a specific package.

```bash
ekapkgs-update update [OPTIONS] <ATTR_PATH>
```

### run

Run daemon mode to continuously update packages.

```bash
ekapkgs-update run [OPTIONS]
```

## Common Options

- `--file <FILE>` - Nix file to evaluate (default: default.nix)
- `--force` - Force update even if package has skip = true
- `--commit` - Create git commit after successful update
- `--create-pr` - Create pull request (implies --commit)
- `--semver <STRATEGY>` - Version strategy: latest, major, minor, patch

## Environment Variables

- `GITHUB_TOKEN` - GitHub API token (recommended for rate limits)
- `GITLAB_TOKEN` - GitLab API token
- `RUST_LOG` - Logging level (debug, info, warn, error)

For complete documentation, run:

```bash
ekapkgs-update --help
ekapkgs-update update --help
ekapkgs-update run --help
```
