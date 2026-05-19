# Quick Start

This guide will help you update your first package with ekapkgs-update in under 5 minutes.

## Prerequisites

- Nix installed with flakes enabled
- A Nix package repository (or use our examples)
- Git repository initialized

## Update Your First Package

### 1. Install ekapkgs-update

```bash
# Quick install via nix profile
nix profile install github:ekapkgs/ekapkgs-update

# Or run directly without installing
alias ekapkgs-update="nix run github:ekapkgs/ekapkgs-update --"
```

### 2. Update a Single Package

Let's update a simple package. If you don't have a repository ready, create a test file:

```bash
# Create a simple test package
cat > test-package.nix <<'EOF'
{
  hello = rec {
    pname = "hello";
    version = "2.10";
    src = builtins.fetchurl {
      url = "https://ftp.gnu.org/gnu/hello/hello-${version}.tar.gz";
      sha256 = "0ssi1wpaf7plaswqqjwigppsg5fyh99vdlb9kzl7c9lng89ndq1i";
    };
  };
}
EOF

# Initialize git repository
git init
git add test-package.nix
git commit -m "Initial commit"
```

Now update the package:

```bash
# Update hello to latest version
ekapkgs-update update hello --file test-package.nix

# Check what changed
git diff
```

You should see the version and hash updated automatically!

### 3. Run with Commit and PR Creation

For production use, you'll want automatic commits and PRs:

```bash
# Update with automatic commit
ekapkgs-update update hello \
  --file test-package.nix \
  --commit

# Update with commit AND PR creation (requires GitHub repo)
ekapkgs-update update hello \
  --file test-package.nix \
  --commit \
  --create-pr
```

### 4. Batch Update Multiple Packages

For repositories with many packages, use `run` mode:

```bash
# Dry-run to see what would be updated
ekapkgs-update run \
  --file ./default.nix \
  --dry-run \
  --max-rebuilds 10

# Actually perform updates
ekapkgs-update run \
  --file ./default.nix \
  --max-rebuilds 50 \
  --concurrent-updates 4
```

The `run` command will:
- Evaluate all packages in your file
- Check for available updates
- Filter based on rebuild count
- Update packages concurrently
- Track results in SQLite database

### 5. Check Update Status

After running updates, view the results:

```bash
# View session status
ekapkgs-update status

# Query recent failures
ekapkgs-update query --since-days 1

# Inspect a specific failure
ekapkgs-update inspect python312Packages.somepackage
```

## Common Workflows

### Test Before Updating

Always test packages after updates:

```bash
# Update and run passthru.tests
ekapkgs-update update gcc --run-passthru-tests

# Or in batch mode
ekapkgs-update run --run-passthru-tests --max-rebuilds 10
```

### Control Version Updates

Use semver strategies to control how versions are selected:

```bash
# Only update to latest patch version (2.1.0 -> 2.1.3, not 2.2.0)
ekapkgs-update update mypackage --semver patch

# Only update to latest minor version (2.1.0 -> 2.3.0, not 3.0.0)
ekapkgs-update update mypackage --semver minor

# Update to latest major version (default)
ekapkgs-update update mypackage --semver latest
```

### Update to Specific Version

```bash
# Update to exact version
ekapkgs-update update python312 --version 3.12.5

# With custom version regex for unusual tag formats
ekapkgs-update update jq --version-regex 'jq-(.*)'
```

### Debugging Failed Updates

When an update fails:

```bash
# 1. Check the failure log
ekapkgs-update log python312Packages.tensorflow

# 2. Inspect detailed failure info
ekapkgs-update inspect python312Packages.tensorflow

# 3. If --preserve-failures was used, inspect worktree
ekapkgs-update worktrees show python312Packages.tensorflow

# 4. Retry after manual fixes
ekapkgs-update retry python312Packages.tensorflow
```

## Environment Variables

Set these for enhanced functionality:

```bash
# GitHub token for PR creation and higher API rate limits
export GITHUB_TOKEN="ghp_your_token_here"

# Cachix for build caching
export CACHIX_AUTH_TOKEN="your_cachix_token"
export CACHIX_CACHE_NAME="your-cache-name"

# GitLab/SourceHut tokens for those platforms
export GITLAB_TOKEN="your_gitlab_token"
export SOURCEHUT_TOKEN="your_sourcehut_token"
```

## Configuration File

For repeated use, create a shell alias or wrapper script:

```bash
# Add to ~/.bashrc or ~/.zshrc
alias ekapkgs-run='ekapkgs-update run \
  --file /path/to/my/packages.nix \
  --database ~/.local/state/ekapkgs/updates.db \
  --max-rebuilds 100 \
  --skip-unstable \
  --concurrent-updates 4'

# Now just run:
ekapkgs-run
```

Or create a configuration script:

```bash
#!/usr/bin/env bash
# update-packages.sh

export GITHUB_TOKEN="$(cat ~/.secrets/github-token)"
export CACHIX_AUTH_TOKEN="$(cat ~/.secrets/cachix-token)"

ekapkgs-update run \
  --file "$PWD/packages/default.nix" \
  --database "$PWD/.ekapkgs/updates.db" \
  --max-rebuilds 200 \
  --concurrent-updates 6 \
  --preserve-failures \
  --analyze-rebuilds \
  "$@"
```

## Next Steps

Now that you understand the basics:

- [Command-Line Interface](./cli/README.md) - Detailed documentation for all commands
- [Configuration](./configuration.md) - Set up repository-specific configuration
- [NixOS Module](./nixos-module.md) - Deploy as an automated service
- [Use Cases](./use-cases/single-package.md) - Learn specific workflows

## Getting Help

```bash
# General help
ekapkgs-update --help

# Command-specific help
ekapkgs-update update --help
ekapkgs-update run --help

# List all commands
ekapkgs-update --help | grep -A 100 "Commands:"
```
