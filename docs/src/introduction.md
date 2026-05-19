# Introduction

**ekapkgs-update** is an automated package updater for Nix package repositories. It discovers new releases, updates package definitions, builds them, runs tests, and optionally creates pull requests—all with minimal manual intervention.

## What is ekapkgs-update?

ekapkgs-update automates the tedious process of keeping Nix packages up-to-date. Instead of manually:

- Checking for new releases on GitHub/GitLab/SourceHut
- Updating version strings and hashes
- Running builds and tests
- Creating PRs with proper formatting

...ekapkgs-update does all of this automatically, tracking successes and failures in a SQLite database with intelligent backoff for problematic packages.

## Key Features

### 🤖 Fully Automated Updates
- **Automatic release detection** from GitHub, GitLab, SourceHut, PyPI, and more
- **Smart version selection** with semver strategy support (latest, major, minor, patch)
- **Hash updates** for source and dependencies (npm, nuget, composer, cargo, etc.)
- **Build validation** with optional `passthru.tests` execution
- **PR creation** with detailed changelogs and metadata

### 📊 Comprehensive Tracking
- **SQLite database** tracking all update attempts, successes, and failures
- **Session management** with statistics (packages attempted, succeeded, failed, skipped)
- **Phase tracking** for detailed failure analysis
- **Backoff mechanism** prevents retry spam for persistently failing packages

### 🔍 Rich Failure Analysis
- **Failure preservation** keeps worktrees and build logs for inspection
- **Query interface** to search and analyze failure patterns
- **LLM integration** via export/apply commands for AI-assisted debugging
- **Retry mechanism** with selective phase resumption

### 🚀 Production Ready
- **NixOS module** for systemd service deployment
- **Web dashboard** for monitoring update status and history
- **Concurrent updates** with configurable worker pools
- **Interactive mode** for manual PR review before submission
- **CI/CD friendly** with dry-run mode and exit codes

### 🛡️ Safety & Control
- **Dry-run mode** to preview changes without modifying anything
- **Rebuild limits** to avoid high-impact updates
- **CVE checking** to flag security vulnerabilities
- **Repology integration** for cross-distribution version validation
- **Directory diff** analysis in PR descriptions

## How It Works

### Architecture

ekapkgs-update uses a **two-service architecture**:

1. **Release Checker Service**: Evaluates packages, detects available updates, applies backoff filtering
2. **Updater Service**: Processes update requests concurrently, performs builds, creates PRs

```
┌─────────────────────────────────────────────────────────────┐
│  ekapkgs-update run                                         │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────────────┐      ┌──────────────────────┐    │
│  │ Release Checker     │─────▶│  Updater Service     │    │
│  │                     │ chan │  (worker pool)       │    │
│  │ - nix-eval-jobs     │ nel  │                      │    │
│  │ - VCS source lookup │      │  - Hash updates      │    │
│  │ - Backoff filtering │      │  - Build & test      │    │
│  │ - Repology check    │      │  - PR creation       │    │
│  └─────────────────────┘      └──────────────────────┘    │
│           │                             │                   │
│           └─────────────┬───────────────┘                   │
│                         ▼                                   │
│                  ┌──────────────┐                          │
│                  │   Database   │                          │
│                  │  (SQLite)    │                          │
│                  └──────────────┘                          │
└─────────────────────────────────────────────────────────────┘
```

### Update Workflow

1. **Discovery**: `nix-eval-jobs` evaluates your package set
2. **Filtering**: Backoff mechanism skips recently-failed packages
3. **Version Check**: Query upstream sources for new releases
4. **Update**: Rewrite package file with new version and hashes
5. **Build**: Validate the update with `nix-build`
6. **Test**: Run `passthru.tests` if enabled
7. **Commit**: Create git commit with changelog
8. **PR**: Push to fork and create pull request (optional)
9. **Record**: Log results to database with full context

## Use Cases

### Single Package Updates
```bash
# Update a single package to latest version
ekapkgs-update update hello

# Update with commit and PR creation
ekapkgs-update update --commit --create-pr python312Packages.requests

# Test before finalizing
ekapkgs-update update --run-passthru-tests gcc
```

### Automated Batch Updates
```bash
# Update entire repository with conservative settings
ekapkgs-update run \
  --file ./default.nix \
  --max-rebuilds 100 \
  --skip-cve \
  --concurrent-updates 4

# Dry-run to see what would be updated
ekapkgs-update run --dry-run --max-rebuilds 10
```

### Failure Analysis & Debugging
```bash
# Query recent failures
ekapkgs-update query --since-days 7 --group-by-error

# Inspect specific failure
ekapkgs-update inspect python312Packages.tensorflow

# Retry after manual fix
ekapkgs-update retry python312Packages.tensorflow
```

### NixOS Service Deployment
```nix
{
  services.ekapkgs-update = {
    enable = true;
    packagesFile = ./packages/default.nix;
    environmentFile = config.sops.secrets.ekapkgs-tokens.path;
    extraArgs = [
      "--max-rebuilds" "200"
      "--skip-unstable"
    ];
  };
}
```

## Getting Started

Ready to automate your package updates? Continue to:

- [Installation](./installation.md) - Install ekapkgs-update on your system
- [Quick Start](./quick-start.md) - Update your first package in 5 minutes
- [Configuration](./configuration.md) - Configure for your repository

## Community & Support

- **Issues**: [GitHub Issues](https://github.com/ekapkgs/ekapkgs-update/issues)
- **Documentation**: This guide
- **Examples**: See [Use Cases](./use-cases/single-package.md) section
