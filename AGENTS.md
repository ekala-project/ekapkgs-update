# AGENTS.md

## Project Overview

ekapkgs-update automates Nix package updates: it discovers upstream releases (GitHub, GitLab, SourceHut, PyPI), rewrites version and hash attributes in `.nix` files, builds, tests, and creates pull requests.

**Workspace members:**

- `ekapkgs-update/` — CLI tool (binary + library)
- `ekapkgs-update-web/` — Axum web dashboard (read-only view of shared SQLite database)

## Rules

- **No commit attribution** — Do not add `Co-Authored-By:` or similar attribution lines to commits.
- **Zero warnings before committing** — Run `cargo clippy --workspace` and resolve all warnings before any commit. All compiler warnings must also be clean.
- **Format before committing** — Run `cargo fmt` and `nix fmt .` before committing to ensure consistent formatting across Rust and Nix files.
- **Keep this file current** — If you alter the project structure (add/remove/rename modules, crates, or significant files), update this file to reflect the change.

## Hard Constraints

- **`unsafe_code = "forbid"`** at workspace level. Do not introduce `unsafe` blocks.
- **MSRV 1.83.0** (see `clippy.toml`).
- **No external mocking frameworks.** Tests use inline assertions and table-driven patterns.
- **CI runs inside `nix develop`** — use `nix develop --command cargo build/test/fmt/clippy`.
- Clippy is strict: `all = warn`, plus selected pedantic lints. `too_many_arguments` and `module_name_repetitions` are explicitly allowed.

## Repository Layout

```
Cargo.toml                          # Workspace root
clippy.toml                         # Clippy thresholds (MSRV, arg limits)
flake.nix                           # Nix flake: dev shell, packages, NixOS module
nix/                                # Nix overlay, package defs, NixOS module

ekapkgs-update/                     # Main CLI crate
  Cargo.toml
  migrations/                       # SQLite migrations (sqlx::migrate!)
  src/
    main.rs                         # Entry: fd limit, tracing, arg parse, dispatch
    lib.rs                          # Public modules (for doctests)
    cli.rs                          # Clap definitions, Commands enum, thin dispatch
    init.rs                         # Process bootstrap (increase_fd_limit)
    paths.rs                        # Centralized XDG cache/data paths
    config.rs                       # Placeholder
    database/                       # SQLite via sqlx: updates, logs, sessions, caches
    commands/
      run/                          # Batch mode: checker + updater services
        config.rs                   #   RunConfig, UpdaterServiceConfig
        checker.rs                  #   Release discovery (Semaphore-limited)
        updater.rs                  #   Update performer (worktree or branch)
        preservation.rs             #   Failed worktree preservation
        types.rs                    #   UpdateRequest, UpdateResult
      update/                       # Single-package update workflow
        config.rs                   #   UpdateConfig, VersionConfig
        errors.rs                   #   UpdateError (structured, LLM-friendly)
        types.rs                    #   UpdatePhase enum
        instrumentation.rs          #   Phase timing/tracking
        mod.rs                      #   update_from_file_path (9-step workflow)
        file_update.rs              #   Version/hash rewriting in .nix files
        build.rs                    #   nix-build invocation, extra args (OnceLock)
        hash_workflows.rs           #   Hash update functions per dep type
        script.rs                   #   nix-update-script support
        flake.rs                    #   Flake package support
        format.rs                   #   nixfmt integration
        git.rs                      #   Git commit creation
        pr.rs                       #   PR body assembly
        variants.rs                 #   mkManyVariants support
      migrate/                      # nixpkgs → ekapkgs paradigm migration
      pr_enhancements.rs            # PrEnhancementsConfig (CVE, Repology, rebuilds, Cachix)
      autofix/                        # LLM-assisted automatic fix pipeline
        config.rs                   #   AutofixConfig
        queue.rs                    #   Queue management (SQLite autofix_queue/autofix_attempts)
        prompt.rs                   #   Prompt construction for small LLMs (with RAG examples)
        retriever.rs                #   RAG: embed errors, retrieve similar successful fixes
        validator.rs                #   Apply changes + nix-build validation
        processor.rs                #   Serial queue processing loop
        dataset.rs                  #   Training dataset export (SFT/DPO JSONL)
      export.rs, apply.rs           # LLM integration (export failure context, apply fixes)
      retry.rs, worktrees.rs        # Failure recovery and worktree management
      log.rs, inspect.rs            # Per-package failure inspection
      query.rs, report.rs, status.rs  # Database query/reporting commands
      prune_maintainers.rs          # Remove deprecated maintainers from .nix files
    llm/                            # OpenAI-compatible LLM client (EKAPKGS_LLM_BASE_URL)
    package/                        # PackageMetadata extraction via nix-instantiate
    vcs_sources/                    # UpstreamSource enum, SemverStrategy, Release matching
    github/, gitlab/, sourcehut/, pypi/  # Platform-specific API clients
    rewrite/                        # Nix file rewriting (regex + rnix AST validation)
    hash_discovery.rs               # Extract correct hash from nix-build stderr
    nix/                            # Nix eval helpers, nix-eval-jobs, eval cache
    git/                            # Git worktree/branch/PR operations
    cve/                            # OSV.dev CVE analysis (24h cache)
    repology/                       # Repology cross-distro version check (72h cache)
    cachix/                         # Binary cache pushing
    directory_diff/                 # Compare build outputs for PR body
    variant_strategy.rs             # mkManyVariants version strategy inference
  tests/
    migrate_tests.rs                # Integration tests with fixture files

ekapkgs-update-web/                 # Web dashboard crate
  Cargo.toml
  src/
    main.rs                         # Axum server setup
    routes/                         # dashboard, sessions, packages, analytics, ws
    state.rs                        # AppState (wraps Database)
    templates.rs                    # Askama template structs
  templates/                        # Askama HTML templates
  static/                           # CSS + HTMX
```

## Architecture: The `run` Command Pipeline

The `run` command is the primary batch-mode entry point. Two concurrent services communicate over an unbounded mpsc channel:

```
nix-eval-jobs (streaming JSON)
        │
        ▼
  Checker Service  ──── mpsc::unbounded_channel ────▶  Updater Service
  (Semaphore-limited)                                  (JoinSet, concurrency-limited)
```

### Checker Service (`commands/run/checker.rs`)

1. Consumes `nix-eval-jobs` output stream
2. For each package: checks database backoff, extracts `PackageMetadata`
3. Reads passthru attributes (`skip`, `semver-strategy`, `include-prereleases`, `version-regex`)
4. Determines `UpstreamSource` from `src.url` or pname
5. Fetches upstream releases, filters by `SemverStrategy`
6. Deduplicates by (pname, version) to avoid updating aliased packages twice
7. Sends `UpdateRequest` on channel

**OOM guard:** Checker concurrency is capped at `max(4, concurrency * 2)` via a `Semaphore`. Each task can spawn nix-instantiate processes, so unbounded concurrency exhausts memory.

### Updater Service (`commands/run/config.rs`)

1. Receives `UpdateRequest` from channel
2. Maintains a `JoinSet` bounded by concurrency (default: `max(1, cpus / 4)`)
3. **Worktree mode** (default): calls `perform_update` — isolated git worktree per package, supports concurrency and PRs
4. **Branch mode**: calls `perform_direct_update` — serializes git operations via `Arc<Mutex<()>>`, no PR support

### Update Workflow (`commands/update/mod.rs`)

The 9-phase workflow for each package:

1. **MetadataExtraction** — Extract `PackageMetadata` via nix-instantiate
2. **SourceDiscovery** — Determine `UpstreamSource` from metadata
3. **VersionSelection** — Fetch best compatible release (SemverStrategy + filters)
4. **SourceHashUpdate** — Update source hash (see Hash Discovery below)
5. **DependencyHashUpdate** — Update dep hashes (cargoHash, vendorHash, npmDepsHash, etc.)
6. **Build** — Build with patch recovery (detect and remove obsolete patches)
7. **Testing** — Run passthru.tests (if configured)
8. **PrCreation** — Commit + push + create PR via GitHub REST API

## Key Patterns

### Hash Discovery (`hash_discovery.rs`, `commands/update/hash_workflows.rs`)

This is intentional, not a bug:

1. Write an **invalid hash** (`sha256-AAAA...A`) into the .nix file
2. Run `nix-build` — it **must fail** with a hash mismatch
3. Parse the **correct hash** from stderr (`got: sha256-<actual>`)
4. Write the correct hash back
5. Build again to verify

Applies to: source hash, cargoHash, vendorHash, npmDepsHash, nugetDepsHash, composerDepsHash. Each has a dedicated `update_*_if_needed` function in `hash_workflows.rs`.

### Nix AST Rewriting (`rewrite/`)

Uses **regex with rnix AST validation**, not pure AST transformation:

1. Parse content with `rnix::Root::parse()` to validate syntax
2. Build a regex to match `attr_name = "value";` patterns
3. Perform text replacement
4. Re-parse to validate the result is still valid Nix

Key functions: `find_and_update_attr()`, `try_update_rev_attr()`, `remove_patch_from_array()`, `replace_maintainers_with_empty()`, `update_variant_attr()`.

Error type: `RewriteError` with an `is_not_found()` method. When `NotFound` is returned for a version attribute, `file_update.rs` falls back to searching sibling files (mkManyVariants pattern).

### mkManyVariants Support

Some packages use a `mkManyVariants` pattern with multiple version variants (e.g., `v0_20`, `v0_23`):

- Detected via `pkg ? variants` Nix evaluation
- `SemverStrategy` inferred from variant name (`v1_2` → Patch, `v1` → Minor)
- 3+ component variants considered pinned (`v1_2_3` → no auto-update)
- Searches sibling files when version isn't found in the primary file

### Database Backoff (`database/mod.rs`)

Failed updates use stepped backoff: **2 days → 4 days → 6 days** (max). Successful updates reset to 2 days.

### CommitStrategy (`cli.rs`)

- **Worktrees** (default): Each update in an isolated git worktree. Full concurrency and PR support.
- **Branch**: Direct commits to current branch. Git serialized via `Arc<Mutex<()>>`. No PRs.

## Important Types

| Type | Location | Purpose |
|------|----------|---------|
| `PackageMetadata` | `package/mod.rs` | Metadata from Nix eval (version, hashes, passthru attrs) |
| `UpstreamSource` | `vcs_sources/mod.rs` | Enum: GitHub, GitLab, SourceHut, PyPI |
| `SemverStrategy` | `vcs_sources/mod.rs` | Enum: Latest, Major, Minor, Patch |
| `Release` | `vcs_sources/mod.rs` | Upstream release with version extraction and matching |
| `UpdatePhase` | `commands/update/types.rs` | 9 phases from MetadataExtraction through PrCreation |
| `UpdateError` | `commands/update/errors.rs` | Structured errors with LLM-friendly serialization |
| `RewriteError` | `rewrite/error.rs` | Parse, NotFound, InvalidResult, Regex, Structural |
| `RunConfig` | `commands/run/config.rs` | Batch mode configuration |
| `UpdateConfig` | `commands/update/config.rs` | Single-update configuration |
| `UpdateRequest` | `commands/run/types.rs` | Message from checker → updater (attr_path, versions) |
| `UpdateResult` | `commands/run/types.rs` | Outcome: Updated, Skipped, DryRun |
| `PrEnhancementsConfig` | `commands/pr_enhancements.rs` | CVE, Repology, rebuild, Cachix, dir diff flags |
| `CommitStrategy` | `cli.rs` | Worktrees vs Branch |
| `NixEvalDrv` | `nix/nix_eval_jobs.rs` | Deserialized nix-eval-jobs output per package |
| `Database` | `database/mod.rs` | SQLite pool wrapper (Clone-safe for async sharing) |
| `FailureArtifacts` | `commands/run/preservation.rs` | Preserved worktree + logs for failed updates |
| `PrConfig` | `git/mod.rs` | Owner/repo/base-branch for PR creation |
| `LlmClient` | `llm/mod.rs` | OpenAI-compatible chat completion client |
| `AutofixQueueItem` | `commands/autofix/queue.rs` | Queue entry for LLM fix attempt |
| `AutofixAttemptRecord` | `commands/autofix/queue.rs` | Single LLM attempt with prompt/response/outcome |

## External Dependencies

Runtime tools (provided by Nix dev shell):

- `nix-instantiate` — Nix expression evaluation
- `nix-build` — Package building
- `nix-eval-jobs` — Parallel package evaluation (streaming JSON)
- `git` — Worktree management, commits, branches
- `nixfmt` — Optional Nix file formatting
- `cachix` — Optional binary cache pushing

API integrations (via reqwest):

- **GitHub REST API** — Release fetching and PR creation (uses `GITHUB_TOKEN`)
- **GitLab API** — Release/tag fetching
- **SourceHut API** — Tag fetching
- **PyPI JSON API** — Python package releases
- **OSV.dev** — CVE vulnerability checking (24h cache)
- **Repology** — Cross-distribution version validation (72h cache, 1 req/sec rate limit)

## Testing

- **Unit tests**: `#[test]` with inline assertions. Located in same file or `tests_*.rs` siblings.
- **Table-driven tests**: `rewrite/tests_attributes.rs`, `tests_maintainers.rs`, `tests_patches.rs`, `tests_rev.rs`.
- **Integration tests**: `tests/migrate_tests.rs` with fixture files in `tests/migrate/`.
- **Web crate tests**: `routes/tests.rs` uses in-memory SQLite and `tower::ServiceExt::oneshot`.
- **Run**: `cargo test --workspace`

## Concurrency and Resource Gotchas

1. **OOM risk** — Checker tasks spawn nix-instantiate subprocesses. Concurrency MUST be limited via Semaphore (`max(4, concurrency * 2)`).
2. **File descriptor limit** — `init::increase_fd_limit()` raises soft RLIMIT_NOFILE at startup. Without this, concurrent worktree operations hit "Too many open files".
3. **Git mutex** — In branch mode, all git operations go through `Arc<Mutex<()>>`. Do not bypass.
4. **Extra nix-build args** — Set once via `OnceLock` in `commands/update/build.rs`. Second call is a no-op.
5. **Database pool** — `Database` is `Clone` (shared pool). Safe to pass across async tasks.

## Extension Recipes

### Adding a New Subcommand

1. Add variant to `Commands` enum in `cli.rs` with clap attributes
2. Create `commands/<name>.rs` (or `commands/<name>/mod.rs` for complex commands)
3. Add `mod <name>` to `commands/mod.rs`
4. Add dispatch arm in `Commands::execute()` in `cli.rs`
5. If it needs database access, accept `database: String` with `default_value = DEFAULT_DATABASE_PATH`

### Adding a New Upstream Source

1. Add variant to `UpstreamSource` enum in `vcs_sources/mod.rs`
2. Create `src/<platform>/mod.rs` with URL parsing and release fetching
3. Add URL pattern matching in `UpstreamSource::from_url()`
4. Implement release fetching in `UpstreamSource::get_releases()`
5. Add module declaration in `lib.rs`

### Adding a New Dependency Hash Type

1. Add hash field to `PackageMetadata` in `package/mod.rs`
2. Add extraction logic in `PackageMetadata::from_attr_path()`
3. Create `update_<hash>_if_needed` in `commands/update/hash_workflows.rs` following existing pattern
4. Call the new function from `update_from_file_path()` in `commands/update/mod.rs`
