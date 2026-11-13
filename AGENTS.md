# AGENTS.md

## Project Overview

ekapkgs-update automates Nix package updates: it discovers upstream releases (GitHub, GitLab, SourceHut, PyPI), rewrites version and hash attributes in `.nix` files, builds, tests, and creates pull requests.

**Workspace members:**

- `ekapkgs-update/` — CLI tool (binary + library)
- `ekapkgs-update-web/` — Axum JSON API + Elm single-page dashboard (read-only view of shared SQLite database)

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
  package-web-frontend.nix          # Offline Elm build of ekapkgs-update-web/frontend → elm.js
  web-frontend/                     # Pinned Elm deps (elm-srcs.nix, registry.dat); update.sh regenerates them

ekapkgs-update/                     # Main CLI crate
  Cargo.toml
  migrations/                       # SQLite migrations (sqlx::migrate!)
  src/
    main.rs                         # Entry: fd limit, tracing, arg parse, dispatch
    lib.rs                          # Public modules (for doctests)
    cli.rs                          # Clap definitions, Commands enum, thin dispatch
    init.rs                         # Process bootstrap (increase_fd_limit)
    paths.rs                        # Centralized XDG cache/data paths
    config.rs                       # TOML config file (LLM, autofix, nix, watch settings)
    database/                       # SQLite via sqlx (shared with the web dashboard)
      mod.rs                        #   Database pool, rebuild stats, upstream_index
      types.rs                      #   UpdateStatus, UpdateRecord, PackageInfo, FailedUpdate, UpdateLog
      updates.rs                    #   Repo-scoped `updates` state machine + backoff, update_logs
      sessions.rs                   #   update_sessions (run/watch) + update_phases
      watch.rs                      #   watched_repos, watch_cycles, upstream_sources
    commands/
      run/                          # Batch mode: checker + updater services
        config.rs                   #   RunConfig, UpdaterServiceConfig
        checker.rs                  #   Release discovery (Semaphore-limited)
        updater.rs                  #   Update performer (worktree or branch)
        branch_git.rs               #   Branch-mode staging/revert of all paths an update dirtied
        claude_fix.rs               #   Claude Code CLI subagent for build fix (--claude-fix)
        preservation.rs             #   Failed worktree preservation
        types.rs                    #   UpdateRequest, UpdateResult
      update/                       # Single-package update workflow
        config.rs                   #   UpdateConfig, VersionConfig
        errors.rs                   #   UpdateError (structured, LLM-friendly)
        types.rs                    #   UpdatePhase enum
        instrumentation.rs          #   PhaseTracker: per-phase update_phases rows
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
      audit/                        # Package correctness auditing
        mod.rs                      #   Standalone command entry + target resolution
        types.rs                    #   Severity, CheckCategory, AuditFinding, AuditReport, AuditConfig
        checks.rs                   #   Check implementations (symlinks, layout, RPATH, shebangs, .pc files, etc.)
        report.rs                   #   Terminal (ANSI), markdown (PR body), and JSON formatters
      review.rs                     # PR review: checkout, build, test, agent review, merge
      migrate/                      # nixpkgs → ekapkgs paradigm migration
      pr_enhancements.rs            # PrEnhancementsConfig (CVE, Repology, rebuilds, Cachix, audit)
      autofix/                        # LLM-assisted automatic fix pipeline
        config.rs                   #   AutofixConfig
        queue.rs                    #   Queue management (SQLite autofix_queue/autofix_attempts)
        prompt.rs                   #   Prompt construction for small LLMs (with RAG examples)
        retriever.rs                #   RAG: embed errors, retrieve similar successful fixes
        validator.rs                #   Apply changes + nix-build validation
        processor.rs                #   Serial queue processing loop
        dataset.rs                  #   Training dataset export (SFT/DPO JSONL)
      watch/                          # RSS/event-driven passive update mode (multi-repo)
        mod.rs                      #   WatchConfig, ResolvedRepo, repo provisioning (clone/fork)
        sync.rs                     #   Fast-forward-only checkout sync before each index
        index.rs                    #   Per-repo upstream index builder (nix-eval-jobs → upstream_index)
        feeds.rs                    #   Feed polling (RSS/Atom/API), UpstreamSource reconstruction
        listener.rs                 #   Event loop: poll cycles, observe-only bootstrap, dispatch decisions
      export.rs                     # Export failure context for LLM analysis (JSON/Markdown)
      apply.rs                      # Apply LLM-generated fixes to preserved worktrees
      retry.rs, worktrees.rs        # Failure recovery and worktree management
      log.rs, inspect.rs            # Per-package failure inspection
      query.rs, report.rs, status.rs  # Database query/reporting commands
      prune_maintainers.rs          # Remove deprecated maintainers and teams from .nix files
    llm/                            # OpenAI-compatible LLM client (EKAPKGS_LLM_BASE_URL)
    package/                        # PackageMetadata extraction via nix-instantiate
    vcs_sources/                    # UpstreamSource enum, SemverStrategy, Release matching
    github/, gitlab/, sourcehut/, pypi/  # Platform-specific API clients (GitLab supports custom instances)
    rewrite/                        # Nix file rewriting (regex + rnix AST validation)
    hash_discovery.rs               # Extract correct hash from nix-build stderr
    nix/                            # Nix eval helpers, nix-eval-jobs, eval cache
    git/                            # Git worktree/branch/PR operations
    cve/                            # OSV.dev CVE analysis (24h cache)
    repology/                       # Repology cross-distro version check (72h cache)
    cachix/                         # Binary cache pushing
    directory_diff/                 # Compare build outputs for PR body
    variant_strategy.rs             # Semver strategy inference (effective_strategy, mkManyVariants)
  tests/
    migrate_tests.rs                # Integration tests with fixture files
    watch_tests.rs                  # Watch index/feed/dispatch integration tests

ekapkgs-update-web/                 # Web dashboard crate (read-only JSON API + Elm SPA)
  Cargo.toml
  build.rs                          # `elm make --optimize` frontend/ → OUT_DIR/elm.js (or copies $EKAPKGS_WEB_ELM_JS; stub if no elm)
  frontend/                         # Elm 0.19 application
    elm.json
    src/
      Main.elm                      #   Browser.application: route → GET /api/… → page; ports liveStatus/liveUpdate
      Route.elm                     #   Path URLs ↔ Route, Route → API URL
      Api.elm                       #   Shared JSON decoders, HTTP error handling
      Format.elm                    #   Times, durations, numbers, badges, URLs (safeUrl)
      View.elm                      #   Shared view helpers (chips, pager, bars, filters, logs)
      Ctx.elm, Msg.elm              #   View context and messages
      Page/                         #   One module per page (Data, decoder, view); Session/Item.elm shared row logic
  src/
    main.rs                         # CLI options, Database::new, router + CORS/trace layers
    state.rs                        # AppState (wraps Database)
    error.rs                        # AppError → JSON 404/500
    params.rs                       # Query-parameter helpers (repo labels, pages, ratios)
    assets.rs                       # App shell (index.html), embedded static files, ETag/Cache-Control
    queries/                        # Raw sqlx SELECTs with local row structs; live.rs = WS fingerprint
    routes/                         # mod.rs (page paths → shell, /api, /ws, /static), api.rs, ws.rs; tests.rs
  static/                           # css/styles.css, js/app.js (WebSocket → Elm ports); compiled into the binary
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
5. Resolves the strategy via `variant_strategy::effective_strategy` (explicit → attr-path suffix, pinned = skip → mkManyVariants = Minor → Latest) and registers the package (`upsert_package_info`)
6. Fetches upstream releases, filters by `SemverStrategy`; records the outcome (`record_no_update` / `record_update_available`)
7. Deduplicates by (pname, version) to avoid updating aliased packages twice
8. Sends `UpdateRequest` on channel

Progress (`packages_evaluated`, `packages_backoff`) is written to the session every 250 packages.

**OOM guard:** Checker concurrency is capped at `max(4, concurrency * 2)` via a `Semaphore`. Each task can spawn nix-instantiate processes, so unbounded concurrency exhausts memory.

### Updater Service (`commands/run/config.rs`)

1. Receives `UpdateRequest` from channel
2. Maintains a `JoinSet` bounded by concurrency (default: `max(1, cpus / 4)`)
3. **Worktree mode** (default): calls `perform_update` — isolated git worktree per package (named `<repo>-<attr>` in multi-repo watch), supports concurrency and PRs
4. **Branch mode**: calls `perform_direct_update` — serializes git operations via `Arc<Mutex<()>>`, no PR support
5. Each attempt gets a `PhaseTracker` (phases → `update_phases`) and ends as `UpdateResult::Updated`, `DryRun`, `Skipped` (not attempted, e.g. worktree setup failed or rebuild limit) or `Failed` (recorded via `record_failed_update`, sets failure backoff)
6. Session counters (`packages_attempted/succeeded/failed/skipped`) are incremented live; `run_service` returns `UpdaterStats`

### Update Workflow (`commands/update/mod.rs`)

The 9-phase workflow for each package:

1. **MetadataExtraction** — Extract `PackageMetadata` via nix-instantiate
2. **SourceDiscovery** — Determine `UpstreamSource` from metadata
3. **VersionSelection** — Fetch best compatible release (SemverStrategy + filters)
4. **SourceHashUpdate** — Update source hash (see Hash Discovery below)
5. **DependencyHashUpdate** — Update dep hashes (cargoHash, vendorHash, npmDepsHash, etc.)
6. **Build** — Build with patch recovery (detect and remove obsolete patches)
7. **Testing** — Run passthru.tests (if configured)
8. **AgentFix** — *(run/watch with `--claude-fix`, on failure)* Claude Code fix attempt
9. **PrCreation** — Commit + push + create PR via GitHub REST API

Every phase is wrapped with `PhaseTracker::run` / `start` (`commands/update/instrumentation.rs`). The standalone `update` command uses `PhaseTracker::disabled()`. A phase failure is classified into an `UpdateError` (`UpdateError::classify`), and `tracker.last_failure()` gives the caller the structured cause.

## Architecture: The `watch` Command (Passive Mode)

The `watch` command is an event-driven alternative to `run`. Instead of evaluating all packages on every cycle, it builds a reverse index and polls upstream feeds. Supports **multi-repo mode** for poly-repo package sets:

```
Config: [[repos]] (name, upstream, entrypoint, path)
        │
        ▼
  Repo provisioning ──── auto-clone if missing, auto-fork via GitHub API
        │
        ▼
  Per-repo nix-eval-jobs ──── periodic re-index (default: 24h)
        │
        ▼
  upstream_index table
  (repo_name, upstream_type, upstream_key → attr_path)
        │
        ▼
  Feed Poller ────────────── RSS/Atom (GitHub, PyPI) + API (GitLab, SourceHut)
  (Semaphore-limited, deduplicated across repos)
        │ new version detected
        ▼
  mpsc::unbounded_channel ──── UpdateRequest carries per-repo routing
        │
        ▼
  UpdaterServiceConfig::run_service ──── uses per-request overrides for
                                         eval_entry_point, pr_config, fork
```

**Repo provisioning** (`commands/watch/mod.rs`): On startup, clones repos that don't exist and creates GitHub forks via API if needed. Resolves each `RepoConfig` into a `ResolvedRepo` with absolute paths and `PrConfig` (`managed = true`; legacy single-repo mode is `managed = false`, name `''`). Then it closes out state left by a killed process (`recover_interrupted_watch_state`), registers repos in `watched_repos` and forgets repos removed from the config (`retain_watched_repos`).

**Sync** (`commands/watch/sync.rs`): Before every index build, managed checkouts are fetched from the remote matching the upstream, then fast-forwarded (`merge --ff-only`) to `<remote>/<base>`. Divergence, or a non-base branch checked out, is logged and recorded as `diverged`, and the stale tree is kept. History is never rewritten. Worktrees are created from `HEAD`, so this is what keeps updates current.

**Index builder** (`commands/watch/index.rs`): Scans packages via `nix-eval-jobs` per-repo (tasks in a `JoinSet`, Semaphore(8)). It extracts `PackageMetadata`, determines `UpstreamSource` and the effective strategy, and stores them in `upstream_index` with `version_regex` / `include_prereleases`. It also registers the package in `updates`. Stale rows are pruned unless the evaluation looks incomplete.

**Feed poller** (`commands/watch/feeds.rs`): Uses RSS/Atom feeds for GitHub and PyPI (cheaper than API), falls back to existing `UpstreamSource::fetch_all_releases()` for GitLab/SourceHut/DirectoryListing. GitHub tags come from the `/releases/tag/<tag>` link, not the entry title (the title is the release name).

**Event loop** (`commands/watch/listener.rs`):

- The first poll runs immediately after the startup index. After that, polls run every `poll_interval`, and sync + re-index runs every `index_refresh`.
- Each poll is a `watch_cycles` row. Each source's health is recorded in `upstream_sources`, and each package's observation in `updates` (`record_observation`).
- **Observe-only bootstrap:** the first observation of a package records `baseline_version` and dispatches nothing. Only releases newer than the baseline are dispatched; the backlog is left to `run`.
- `dispatch_decision` (pure, unit-tested) evaluates, in order: Baseline, UpToDate, Backlog, AlreadyProposed, Backoff (same failed version only), Dispatch.
- `resolve_siblings` collapses aliases (same repo, current and target version) to the shortest attr path.
- **Lazy sessions:** an update session (`SessionMode::Watch`) is created only when a cycle dispatches at least one update.

**NixOS deployment**: Third systemd service (`ekapkgs-update-watch.service`) alongside the main daemon and web portal. Same user/group, shared database, `Restart=always`. `watch.repos` option configures multi-repo mode.

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

Key functions: `find_and_update_attr()`, `try_update_rev_attr()`, `remove_patch_from_array()`, `replace_maintainers_with_empty()`, `replace_teams_with_empty()`, `update_variant_attr()`, `add_variant_entry()`.

Error type: `RewriteError` with an `is_not_found()` method. When `NotFound` is returned for a version attribute, `file_update.rs` falls back to searching sibling files (mkManyVariants pattern).

### mkManyVariants Support

Some packages use a `mkManyVariants` pattern with multiple version variants (e.g., `v0_20`, `v0_23`):

- Detected via `pkg ? variants` Nix evaluation
- `SemverStrategy` inferred from variant name (`v1_2` → Patch, `v1` → Minor)
- 3+ component variants considered pinned (`v1_2_3` → no auto-update)
- Searches sibling files when version isn't found in the primary file
- **New variant discovery** (`--all-variants`): After updating existing variants, fetches all upstream releases, groups by version series, and adds new variant entries to `variants.nix` for series newer than the highest existing variant. Handles both single-hash and platform-hash packages. Each new variant is auto-committed individually when `--commit` is passed.
- **`run` command default**: mkManyVariants packages default to `SemverStrategy::Minor` in the batch checker, constraining updates to the same major version series.
- **Hash attribute flexibility**: Variant hash discovery tries `src-hash`, `hash`, and `sha256` attribute names, supporting different naming conventions across packages.

### Platform-Specific Hashes

Packages with `passthru.ekapkgs-update.platform-hashes` (a list of system strings like `["x86_64-linux" "aarch64-linux"]`) get per-platform hash discovery. After the version is bumped, the tool evaluates `src.url` with `--system <platform>` to get each platform's download URL, then prefetches it with `nix store prefetch-file --json` to compute the SRI hash. This avoids needing to build for foreign platforms.

### Explicit Upstream Source

When `src.url` can't be parsed (mirror:// schemes, custom domains), packages can set `passthru.ekapkgs-update.github-repo = "owner/repo"` to explicitly specify the GitHub repository for release discovery. This takes priority over URL-based detection.

### Database State and Backoff (`database/updates.rs`)

`updates` is keyed by `(repo_name, attr_path)`. `repo_name = ''` is used by `run` and legacy watch; multi-repo watch uses the config name (`default` is reserved — the dashboard labels `''` with it; rejected by `ConfigFile::validate` and a NixOS module assertion). All `updates` methods take `repo_name` first. `autofix_queue` / `autofix_attempts` are repo-scoped the same way (via `FailureArtifacts.repo_name`; preserved failures live under `failed/<session>/[<repo>-]<attr>/`).

`status` values:

- `pending`: indexed, not yet compared
- `up_to_date`
- `outdated`: newer compatible release, not proposed
- `proposed`: built and proposed; becomes `up_to_date` when `current_version` catches up
- `failed`
- `upstream_error`

Two kinds of backoff:

- **Check backoff** (`record_no_update`): 2 → 4 → 6 days, by time since the last check.
- **Failure backoff** (`record_failed_update`): 2 → 4 → 6 days, by `failure_count`. Also sets `failed_version` and `last_error`, and writes `update_logs`; the key is the drv path, or `<repo>:<attr>@<version>` when there is none.

Successful updates (`record_successful_update`) reset `failure_count` and back off 2 days. Watch ignores the check backoff and honours the failure backoff only for the same `failed_version`.

The web dashboard (`ekapkgs-update-web`) reads these tables directly. Keep the migration comments in `migrations/20261111_dashboard_data.sql` in sync with the column semantics.

### CommitStrategy (`cli.rs`)

- **Worktrees** (default): Each update in an isolated git worktree. Full concurrency and PR support.
- **Branch**: Direct commits to current branch. Git serialized via `Arc<Mutex<()>>`. No PRs.

### PR Review (`commands/review.rs`)

The `review` command automates PR review by checking out the PR branch, building changed packages, and optionally running tests and a best-practices agent review:

```
ekapkgs-update review <PR_NUMBER> --repo owner/repo [--tests] [--best-practices]
```

**Workflow:**

1. Fetch PR details and changed files from GitHub API
2. Create an isolated worktree from the current repo
3. Checkout the PR branch, merge latest base branch (must succeed — merge conflicts fail the review)
4. Determine affected attr paths from changed `.nix` files (heuristic: `pkgs/by-name/<shard>/<pname>/` or parent dir name, with dynamic scope discovery for scoped packages — e.g. `postgresql/pkgs/<name>/` auto-resolves to `postgresqlPackages.<name>` via `nix-instantiate` eval)
5. Validate each candidate attr path exists in the entry point
6. Build each package with `nix-build`
7. *(if `--tests`)* Build `passthru.tests` for each successfully built package
8. *(if `--best-practices`)* Invoke an agent CLI (default: `claude`) with a structured prompt checking for common Nix packaging issues (build input placement, metadata, deprecated patterns, hash format, etc.)
9. If all checks pass: post APPROVE review and merge the PR
10. If any checks fail: post REQUEST_CHANGES review with failure details and exit non-zero

**Agent review:** The `--best-practices` flag spawns a Claude Code (or other agent specified via `--best-practices-agent`) process with `--print --dangerously-skip-permissions`. The agent must end its output with `VERDICT: PASS` or `VERDICT: FAIL` followed by bullet-pointed issues.

**`--repo` format:** Accepts `owner/repo` (assumes GitHub) or full URL `github.com/owner/repo`. Requires `GITHUB_TOKEN` environment variable.

### Package Audit (`commands/audit/`)

The `audit` command inspects built Nix package outputs for correctness issues. It has two surfaces:

1. **Standalone command** (`ekapkgs-update audit <target>`) — accepts a `.drv` path, store path, or attr path. Builds if needed, walks outputs, runs all checks, and reports findings. Exits with code 1 if any errors are found (CI-friendly).

2. **Run integration** (`ekapkgs-update run --audit`) — after each successful build during batch updates, audits the package and includes findings in the PR body.

#### Checks performed (`commands/audit/checks.rs`)

| Check | Category | Severity | Description |
|-------|----------|----------|-------------|
| `broken_symlink` | BrokenReferences | Error | Symlinks that don't resolve |
| `missing_store_reference` | BrokenReferences | Warning | Hardcoded `/nix/store/...` paths that don't exist (scans first 8KB) |
| `missing_shared_lib` | BrokenReferences | Error | `ldd` reports "not found" for shared libraries |
| `pkg_config_bad_path` | BrokenReferences | Error | `.pc` file path fields or `-L`/`-I` flags reference non-existent dirs |
| `empty_output` | LayoutIssues | Error | Output directory contains no entries |
| `no_regular_files` | LayoutIssues | Warning | Output has directories but no regular files |
| `unexpected_top_level` | LayoutIssues | Warning | Top-level entries outside `bin/`, `lib/`, `share/`, `etc/`, `include/`, etc. |
| `misplaced_executable` | LayoutIssues | Info | Executable files outside `bin/`, `sbin/`, `libexec/` |
| `build_dir_in_rpath` | BinaryIssues | Error | RPATH contains `/build/`, `/tmp/`, or `/homeless-shelter/` |
| `fhs_rpath` | BinaryIssues | Warning | RPATH contains FHS paths like `/usr/lib` |
| `fhs_shebang` | ScriptIssues | Error | Shebang points to `/usr/bin/`, `/bin/` (except `/bin/sh`) |
| `bin_sh_shebang` | ScriptIssues | Info | Uses `/bin/sh` (works on NixOS but not fully pure) |
| `fhs_path_reference` | ScriptIssues | Warning | Scripts contain hardcoded `/usr/`, `/opt/`, `/etc/` paths |
| `missing_description` | MetadataIssues | Info | `meta.description` not set |
| `missing_license` | MetadataIssues | Warning | `meta.license` not set |
| `missing_homepage` | MetadataIssues | Info | `meta.homepage` not set |
| `world_writable_file` | SecurityIssues | Warning | File is world-writable (mode o+w) |
| `hidden_files_in_output` | SecurityIssues | Info | Sensitive dotfiles (`.env`, `.ssh`, `.npmrc`, etc.) in output |
| `suspicious_binary_in_share` | SecurityIssues | Warning | ELF binary in unexpected location (`share/`, `etc/`, `include/`) |
| `embedded_credentials` | SecurityIssues | Warning | Hardcoded API keys, tokens, or passwords (AWS, GitHub, GitLab, OpenAI/Stripe) |
| `suspicious_network_in_output` | SecurityIssues | Warning | References to pastebin/ngrok/tunnel hosts, IP-literal URLs, or non-HTTPS URLs |
| `shell_download_pattern` | SecurityIssues | Warning | Download-and-execute patterns (`curl\|sh`, `wget\|bash`, etc.) |
| `crypto_miner_indicators` | SecurityIssues | Warning | Crypto mining pool URLs or miner binary names |
| `data_exfiltration_pattern` | SecurityIssues | Warning | Script reads sensitive files and has network send capability |
| `eval_of_remote_content` | SecurityIssues | Warning | Dynamic code execution of fetched content (`eval "$(curl ...)"`) |
| `high_entropy_strings` | SecurityIssues | Info | High-entropy strings (≥40 chars, >4.5 bits/char) that may be secrets or obfuscated code |

Checks that depend on external tools (`ldd`, `patchelf`) gracefully skip if the tool is unavailable.

The pkg-config check parses `.pc` variable definitions, resolves `${var}` substitutions, and validates that path-valued fields (`prefix`, `libdir`, `includedir`, etc.) and `-L`/`-I` flags in `Libs`/`Cflags` lines point to existing directories.

Security checks are enabled by default. Use `--no-security` to disable them on both the standalone `audit` command and the `run --audit` command.

#### Integration with `run` command

Controlled by `PrEnhancementsConfig.run_audit`. When enabled, `perform_worktree_audit()` builds the package in the worktree, runs all checks, and returns markdown for the PR body. Audit findings are informational — they do not block the update. Security checks can be disabled via `PrEnhancementsConfig.skip_security` (CLI: `--no-security`).

### Export (`commands/export.rs`)

The `export` command extracts failure context from preserved worktrees into LLM-friendly formats for analysis or feeding to the autofix pipeline.

```
ekapkgs-update export <attr_path> [--format json|markdown] [-o file]
```

Collects from preserved failure artifacts:
- **Error context** — Structured JSON from the failed update phase
- **Git diff** — Changes made before failure (truncated to 10KB in markdown)
- **Build log** — Nix build stderr (last 5KB in markdown)
- **Test output** — passthru.tests output if applicable
- **Package metadata** — Nix evaluation metadata
- **Previous attempts** — Historical phase/status/duration from database

Output formats:
- **JSON** (`--format json`) — Complete `LlmContext` struct serialized with serde
- **Markdown** (`--format markdown`) — Sections with fenced code blocks, suitable for pasting into LLM conversations

Requires `--preserve-failures` to have been set during the `run` that produced the failure.

## Important Types

| Type | Location | Purpose |
|------|----------|---------|
| `PackageMetadata` | `package/mod.rs` | Metadata from Nix eval (version, hashes, passthru attrs) |
| `UpstreamSource` | `vcs_sources/mod.rs` | Enum: GitHub, GitLab (multi-instance), SourceHut, PyPI, DirectoryListing |
| `SemverStrategy` | `vcs_sources/mod.rs` | Enum: Latest, Major, Minor, Patch |
| `Release` | `vcs_sources/mod.rs` | Upstream release with version extraction and matching |
| `UpdatePhase` | `commands/update/types.rs` | Workflow phases (MetadataExtraction … PatchRecovery, Testing, AgentFix, PrCreation); snake_case `as_str()` stored in the DB |
| `UpdateError` | `commands/update/errors.rs` | Structured errors with LLM-friendly serialization |
| `RewriteError` | `rewrite/error.rs` | Parse, NotFound, InvalidResult, Regex, Structural |
| `RunConfig` | `commands/run/config.rs` | Batch mode configuration |
| `UpdateConfig` | `commands/update/config.rs` | Single-update configuration |
| `UpdateRequest` | `commands/run/types.rs` | Message from checker → updater (attr_path, versions, per-repo routing) |
| `UpdateResult` | `commands/run/types.rs` | Outcome: Updated, Skipped (not attempted), Failed (attempted, recorded), DryRun |
| `UpdaterStats` | `commands/run/config.rs` | Per-`run_service` outcome counts |
| `PhaseTracker` | `commands/update/instrumentation.rs` | Records `update_phases` rows for one attempt (no-op when disabled) |
| `UpdateStatus` / `UpdateRecord` | `database/types.rs` | `updates` row state (status, versions, failures, backoff) |
| `PackageInfo` | `database/types.rs` | Cached pname/description/homepage/upstream/strategy for the dashboard |
| `FailedUpdate` | `database/types.rs` | Input to `record_failed_update` (phase, error, session) |
| `SessionMode` / `SessionCounter` | `database/sessions.rs` | Session kind (run/watch) and live counters |
| `WatchCycleStats` / `IndexOutcome` | `database/watch.rs` | Poll/index cycle results |
| `PrEnhancementsConfig` | `commands/pr_enhancements.rs` | CVE, Repology, rebuild, Cachix, dir diff flags |
| `CommitStrategy` | `cli.rs` | Worktrees vs Branch |
| `NixEvalDrv` | `nix/nix_eval_jobs.rs` | Deserialized nix-eval-jobs output per package |
| `Database` | `database/mod.rs` | SQLite pool wrapper (Clone-safe for async sharing) |
| `FailureArtifacts` | `commands/run/preservation.rs` | Preserved worktree + logs for failed updates |
| `PrConfig` | `git/mod.rs` | Owner/repo/base-branch for PR creation |
| `LlmClient` | `llm/mod.rs` | OpenAI-compatible chat completion client |
| `AutofixQueueItem` | `commands/autofix/queue.rs` | Queue entry for LLM fix attempt |
| `AutofixAttemptRecord` | `commands/autofix/queue.rs` | Single LLM attempt with prompt/response/outcome |
| `AuditReport` | `commands/audit/types.rs` | Complete audit result (findings, timing, store paths) |
| `AuditFinding` | `commands/audit/types.rs` | Single finding (severity, category, check name, file path) |
| `AuditConfig` | `commands/audit/types.rs` | Controls which checks to run and minimum severity |
| `WatchConfig` | `commands/watch/mod.rs` | Watch (passive/RSS-driven) mode configuration |
| `ResolvedRepo` | `commands/watch/mod.rs` | Runtime-resolved repo with paths, PrConfig, fork remote |
| `RepoConfig` | `config.rs` | TOML config for a watched repo (name, upstream, entrypoint, path) |
| `UpstreamIndexEntry` | `database/mod.rs` | Row from upstream_index table (repo_name + source → attr_path, strategy/regex/prereleases) |
| `ReviewConfig` | `commands/review.rs` | PR review configuration (repo, PR#, tests, best-practices flags) |
| `RepoCoords` | `commands/review.rs` | Parsed repository owner/repo coordinates |

## External Dependencies

Runtime tools (provided by Nix dev shell):

- `nix-instantiate` — Nix expression evaluation
- `nix-build` — Package building (supports `--system` for cross-platform eval)
- `nix store prefetch-file` — URL hash prefetching for platform-hashes
- `nix-eval-jobs` — Parallel package evaluation (streaming JSON)
- `git` — Worktree management, commits, branches
- `nixfmt` — Optional Nix file formatting
- `cachix` — Optional binary cache pushing
- `patchelf` — Optional RPATH inspection (used by audit)
- `ldd` — Optional shared library checking (used by audit)

API integrations (via reqwest):

- **GitHub REST API** — Release fetching and PR creation (uses `GITHUB_TOKEN`)
- **GitLab API** — Release/tag fetching (supports gitlab.com, freedesktop.org, GNOME, KDE, Alpine, Arch, Debian instances)
- **SourceHut API** — Tag fetching
- **PyPI JSON API** — Python package releases
- **HTTP directory listings** — Version scraping from GNU FTP / SourceForge directory pages
- **OSV.dev** — CVE vulnerability checking (24h cache)
- **Repology** — Cross-distribution version validation (72h cache, 1 req/sec rate limit)

## Testing

- **Unit tests**: `#[test]` with inline assertions. Located in same file or `tests_*.rs` siblings.
- **Table-driven tests**: `rewrite/tests_attributes.rs`, `tests_maintainers.rs`, `tests_patches.rs`, `tests_rev.rs`.
- **Integration tests**: `tests/migrate_tests.rs` with fixture files in `tests/migrate/`.
- **Web crate tests**: `routes/tests.rs` drives every API endpoint, the app shell and static assets via `tower::ServiceExt::oneshot` against in-memory SQLite with the real migrations (a fixture covering every table, plus an empty database). The Elm frontend has no unit tests; `elm make` type-checks it on every build.
- **Web crate frontend**: `build.rs` compiles `frontend/` with `elm`, which the dev shell provides on x86_64-linux only. `EKAPKGS_WEB_ELM_JS` can point at a prebuilt `elm.js` instead; a missing file there is an error. Without either, the build warns and embeds a stub `elm.js` (a "frontend not built" notice), so `cargo build/test --workspace` work without `elm`. An Elm compile error always fails the build. `EKAPKGS_WEB_REQUIRE_ELM=1` makes a missing `elm` fatal too. After changing `frontend/elm.json`, run `nix/web-frontend/update.sh`.
- **Run**: `cargo test --workspace`

## Concurrency and Resource Gotchas

1. **OOM risk** — Checker tasks spawn nix-instantiate subprocesses. Concurrency MUST be limited via Semaphore (`max(4, concurrency * 2)`).
2. **File descriptor limit** — `init::increase_fd_limit()` raises soft RLIMIT_NOFILE at startup. Without this, concurrent worktree operations hit "Too many open files".
3. **Git mutex** — In branch mode, all git operations go through `Arc<Mutex<()>>`. Do not bypass.
4. **Extra nix-build args** — Set once via `OnceLock` in `commands/update/build.rs`. Second call is a no-op.
5. **Database pool** — `Database` is `Clone` (shared pool). Safe to pass across async tasks.
6. **Never `set_current_dir` in run/watch code** — the cwd is process-wide and updates run concurrently. Pass the directory to each subprocess (`Command::current_dir`) instead. `build_and_get_outputs(work_dir, …)` runs `nix-build` in `work_dir` and writes its `result*` out-links to a private temporary directory, so concurrent builds never share them. Only single-shot CLI commands (`retry`, `apply`) change the cwd.

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
