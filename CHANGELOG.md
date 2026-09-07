# Changelog

## Unreleased

### Batch Updates (`run`)

- Evaluates all packages via `nix-eval-jobs`, checks upstream sources, and updates in parallel
- Worktree isolation by default — each update gets its own git worktree for safe concurrency
- Branch mode alternative — serialized commits directly to the working tree
- Automatic PR creation with rich bodies: changelogs, rebuild impact, CVE analysis, directory diffs, audit results
- Semver-aware version selection: latest, major, minor, or patch strategies (per-package via passthru)
- Repology cross-distribution version validation
- CVE analysis via OSV.dev with 24h caching
- Rebuild impact analysis with configurable thresholds
- Cachix binary cache push after successful builds
- Package audit integration — checks built outputs for correctness issues
- `--interactive` mode for manual PR review before submission
- `--dry-run` to preview updates without modifying anything
- `--preserve-failures` to keep worktrees for post-mortem inspection
- `--claude-fix` to invoke Claude Code CLI for automated build failure repair
- Database-backed backoff: failed packages wait 2/4/6 days before retry
- Deduplication of aliased packages (e.g., python312Packages.foo and python313Packages.foo)

### Single Package Update (`update`)

- Update any package by attribute path with full hash discovery workflow
- Supports all dependency hash types: cargoHash, vendorHash, npmDepsHash, nugetDepsHash, composerDepsHash
- Automatic obsolete patch removal with build recovery
- Platform-specific source hash discovery for multi-platform packages
- Flake package support (`--flake`)
- passthru.updateScript integration (skippable with `--ignore-update-script`)
- `--version` to pin to a specific upstream release
- `--version-regex` for custom tag-to-version extraction

### mkManyVariants Support

- Automatic detection and per-variant updates with strategy inference from naming convention
- `--all-variants` updates every variant and discovers new upstream version series
- New variants automatically added to `variants.nix` when upstream releases a new series
- Platform-hash aware — handles both single-hash and multi-platform-hash variants
- 3+ component variants treated as pinned (no auto-update)
- Default Minor strategy in batch mode keeps variants within their major series
- Tries `src-hash`, `hash`, and `sha256` attribute names for hash discovery/update

### Package Audit (`audit`)

- Standalone or run-integrated auditing of built Nix package outputs
- Checks: broken symlinks, missing shared libs, bad RPATHs, FHS shebangs, layout issues, pkg-config validation
- Security scanning: embedded credentials, crypto miners, download-and-execute patterns, data exfiltration, high-entropy strings
- Output formats: human-readable, JSON, markdown

### Autofix Pipeline (`autofix`)

- LLM-assisted automatic repair of failed builds using RAG-retrieved similar fixes
- Queue-based processing with configurable attempt limits
- Training dataset export (SFT/DPO JSONL) from historical fix attempts

### Failure Investigation

- `log` / `inspect` — view failure details by package or derivation path
- `query` — filter failures by error type, phase, status, date range
- `report` — generate categorized markdown failure reports
- `export` — extract failure context as JSON/markdown for LLM analysis
- `apply` — apply LLM-generated patches to preserved worktrees and validate
- `retry` — resume failed updates from a specific phase
- `worktrees` — list, inspect, and clean preserved failure artifacts

### Package Migration (`migrate`)

- Migrate packages from nixpkgs conventions to ekapkgs patterns

### Maintenance (`prune-maintainers`)

- Remove deprecated maintainer handles and empty team references from .nix files
- `--check` mode for CI integration

### Upstream Sources

- GitHub, GitLab (multi-instance), SourceHut, PyPI, HTTP directory listings
- `mirror://github/` URLs automatically normalized to GitHub API
- `mirror://gnu/` and `mirror://sourceforge/` URLs scraped via directory listing
- Custom GitLab instances: freedesktop.org, GNOME, KDE, Alpine, Arch, Debian
- Explicit override via `passthru.ekapkgs-update.github-repo = "owner/repo"` for packages with non-parseable URLs
- Release and tag fetching with authentication token support
- Prerelease filtering (API flags + version string heuristics)

### Platform-Specific Hash Discovery

- `passthru.ekapkgs-update.platform-hashes` lists systems with distinct source archives
- After version bump, evaluates `src.url` with `--system` per platform
- Prefetches each URL via `nix store prefetch-file --json` to compute SRI hash
- Gracefully skips platforms that can't be evaluated (e.g., darwin on linux-only repos)

### Web Dashboard (`ekapkgs-update-web`)

- Read-only Axum web UI over the shared SQLite database
- Session, package, and analytics views
