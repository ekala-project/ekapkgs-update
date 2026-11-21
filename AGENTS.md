# AGENTS.md

## Project Overview

ekapkgs-update automates Nix package updates: it discovers upstream releases (GitHub, GitLab, SourceHut, PyPI), rewrites version and hash attributes in `.nix` files, builds, tests, and creates pull requests.

**Workspace members:**

- `ekapkgs-update/` — CLI tool (binary + library)
- `ekapkgs-update-web/` — Axum JSON API + Elm single-page dashboard (read-only view of shared SQLite database)

## Rules

- **No commit attribution** — Do not add `Co-Authored-By:` or similar attribution lines to commits.
- **Zero warnings before committing** — Run `cargo clippy --workspace` and resolve all warnings before any commit. All compiler warnings must also be clean.
- **Format before committing** — Run `cargo fmt` and `nix fmt .` before committing to ensure consistent formatting across Rust, Nix and Elm files. `nix fmt .` runs rustfmt, nixfmt and `elm-format` (over every `*.elm` file, including `ekapkgs-update-web/frontend/`); `elm-format` is also in the dev shell.
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
  elm-format.nix                    # elm-format built from source (not in ekapkgs); dev shell + nix fmt
  iets.nix                          # iets built from source (flake input iets-src; not in ekapkgs); provides `iets eval-jobs`
  package-web-frontend.nix          # Offline Elm build of ekapkgs-update-web/frontend → elm.js (uglify-js minified)
  web-frontend/                     # Pinned Elm deps incl. test deps (elm-srcs.nix, registry.dat); update.sh regenerates them
  web-frontend-tests.nix            # Sandbox check running the Elm unit tests (flake checks.web-frontend-tests)

ekapkgs-update/                     # Main CLI crate
  Cargo.toml
  migrations/                       # SQLite migrations (sqlx::migrate!)
  src/
    main.rs                         # Entry: fd limit, tracing, arg parse, dispatch
    lib.rs                          # Public modules (for doctests)
    cli.rs                          # Clap definitions, Commands enum, thin dispatch
    init.rs                         # Process bootstrap (increase_fd_limit)
    paths.rs                        # Centralized XDG cache/data paths
    config.rs                       # TOML config file (LLM, autofix, nix, watch, github settings); values become CLI defaults
    database/                       # SQLite via sqlx (shared with the web dashboard)
      mod.rs                        #   Database pool, rebuild stats, upstream_index
      types.rs                      #   UpdateStatus, UpdateRecord, PackageInfo, FailedUpdate, UpdateLog
      updates.rs                    #   Repo-scoped `updates` state machine + backoff, update_logs
      sessions.rs                   #   update_sessions (run/watch) + update_phases
      watch.rs                      #   watched_repos, watch_cycles, upstream_sources
      groups.rs                     #   updates.group_parent/group_root: replace_groups, group_roots, group_members
    commands/
      run/                          # Batch mode: checker + updater services
        config.rs                   #   RunConfig, UpdaterServiceConfig
        checker.rs                  #   Release discovery (Semaphore-limited)
        updater.rs                  #   Update performer (worktree or branch)
        branch_git.rs               #   Branch-mode staging/revert of all paths an update dirtied; GroupCheckpoint (group commits + undo)
        grouping.rs                 #   Requests → UpdateJobs (group_jobs), run-mode group bookkeeping (RunGroups)
        group_update.rs             #   Group executors (worktree: group PR; branch: commits on the current branch); shared member loop/verification
        claude_fix.rs               #   Claude Code CLI subagent for build fix (--claude-fix)
        preservation.rs             #   Failed worktree preservation (branch mode: changes replayed into a detached worktree)
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
        fod_refresh.rs              #   Closure diff: refresh hashes of every FOD the bump changed (by value)
        fod_platforms.rs            #   Per-system hash attrsets: prefetch the other systems' hashes
        components.rs               #   passthru.ekapkgs-update.components: bump independently pinned sources
        pins.rs                     #   Component pins read from upstream files (meson .wrap, Cargo.lock, go.mod, regex)
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
      migrate/                      # nixpkgs → ekapkgs paradigm migration (arguments.rs, final_attrs.rs, transforms.rs)
      pr_enhancements.rs            # PrEnhancementsConfig (CVE, Repology, rebuilds, Cachix, audit)
      autofix/                        # LLM-assisted automatic fix pipeline
        config.rs                   #   AutofixConfig
        queue.rs                    #   Queue management (SQLite autofix_queue/autofix_attempts)
        prompt.rs                   #   Prompt construction for small LLMs (with RAG examples)
        retriever.rs                #   RAG: embed errors, retrieve similar successful fixes
        validator.rs                #   Apply changes + nix-build validation against the worktree's own entry point
        processor.rs                #   Serial queue processing loop
        dataset.rs                  #   Training dataset export (SFT/DPO JSONL)
      watch/                          # RSS/event-driven passive update mode (multi-repo)
        mod.rs                      #   WatchConfig, ResolvedRepo, repo provisioning (clone/fork)
        sync.rs                     #   Fast-forward-only checkout sync (startup, re-index, before update batches); ls-remote upstream head
        refresh.rs                  #   Index builds (indexed_commit) + head watcher: re-index when the upstream branch moves (merged PRs)
        index.rs                    #   Per-repo upstream index: one `iets eval-jobs` pass with --apply → upstream_index
        feeds.rs                    #   Feed polling (RSS/Atom/API), per-entry version policy, UpstreamSource reconstruction, source health (moved/not found)
        listener.rs                 #   Event loop: poll cycles, observe-only bootstrap, dispatch decisions
        webhook.rs                  #   GitHub webhook endpoint (axum, --webhook-listen): verify, dedupe, trigger head checks / withdraw closed PRs
      export.rs                     # Export failure context for LLM analysis (JSON/Markdown)
      apply.rs                      # Apply LLM-generated fixes to preserved worktrees
      retry.rs, worktrees.rs        # Failure recovery (copy preserved worktree, apply a patch, rebuild; no cwd change) and worktree management
      reset_failures.rs             # Clear failure backoff of failed updates (filters: repo/attr/session/time/error text)
      log.rs, inspect.rs            # Per-package failure inspection (`--repo`; `log` takes an attr path, or a store path / `.drv`)
      query.rs, report.rs, status.rs  # Database query/reporting commands (`--repo`; `report` lists still-failing packages per (repo, attr))
      lint.rs, tests_lint.rs        # `lint`: check .nix files (incl. `format`); exits non-zero on findings or file errors
      prune_maintainers.rs          # Remove deprecated maintainers and teams from .nix files
      cve_scan.rs                   # `cve-scan`: scan tracked packages for known vulnerabilities (OSV.dev)
    llm/                            # OpenAI-compatible LLM client (EKAPKGS_LLM_BASE_URL); types.rs = request/response types
    package/                        # PackageMetadata extraction via nix-instantiate
    vcs_sources/                    # UpstreamSource enum, SemverStrategy, Release matching (tests.rs)
      version_order.rs              #   compare_version_components: the one version order (numeric, prereleases below release, dates)
      http_status.rs                #   HttpStatusError: typed non-success status from release endpoints (404/410 = not found)
    github/app.rs                   # GitHub App: RS256 JWT (openssl), installation tokens (cached), bot identity
    github/auth.rs                  # Credential choice: App installation tokens else $GITHUB_TOKEN; git env (bot identity + token helper)
    github/webhook.rs               # Pure: X-Hub-Signature-256 verification, push/pull_request payloads → WebhookEvent
    github/, gitlab/, sourcehut/, pypi/  # Platform-specific API clients (GitLab: custom instances, nested groups, fetchFromGitLab `api/v4/projects/<encoded>` URLs; SourceHut: GraphQL `user { repository(name:) }`, needs SOURCEHUT_TOKEN)
    github/existing_prs.rs          # Pre-PR scan of the repo's open PRs by the PR author (`--pr-author`) for the same package
    pypi/sdist.rs                   # Pure sdist/wheel filename matching (PEP 503 names, wheel tags) used after PyPI version bumps
    rewrite/                        # Nix file rewriting (regex + rnix AST validation): attributes, rev_update, rev_fixup, patches,
                                    #   maintainers, teams, variants, fetch_pypi, error.rs; table tests in tests_*.rs
    hash_discovery.rs               # Extract correct hash from nix-build stderr
    nix/                            # Nix eval helpers, `iets eval-jobs` (run_eval.rs: --impure, --select/--apply, exit status; nix_eval_jobs.rs: its nix-eval-jobs-format JSON), eval_cache.rs, flake.rs (flake packages), rebuild_count.rs
    git/                            # Git worktree/branch/PR operations
    cve/                            # OSV.dev CVE analysis: PR-body diff (24h cache) + package scans
      osv.rs, cache.rs, ecosystem.rs, types.rs  #   OSV client/CVSS v3 scoring, 24h cache, ecosystem mapping, shared types
      cvss4.rs                      #   CVSS v4.0 scoring (port of the FIRST reference calculator: MacroVector lookup + interpolation)
      scan.rs                       #   Package → OSV query (GIT repo URL / PyPI), querybatch, record fetch, alias merge, severity
      store.rs                      #   osv_vulnerabilities cache, package_vulnerabilities, per-package scan state, due targets
    repology/                       # Repology cross-distro version check (72h cache; api.rs, cache.rs, types.rs)
    cachix/                         # Binary cache pushing (push.rs, types.rs)
    directory_diff/                 # Compare build outputs for PR body (compare.rs, format.rs, types.rs)
    variant_strategy.rs             # Semver strategy inference (effective_strategy, mkManyVariants)
    groups.rs                       # Update groups (pure): parent resolution, roots, dependency order
  tests/
    migrate_tests.rs                # Integration tests with fixture files
    watch_tests.rs                  # Watch index/feed/dispatch integration tests (in-memory SQLite)

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
    tests/                          #   elm-explorations/test suites (*Test.elm expose `suite`); Table.elm = table-driven helper
    test-runner/                    #   run.sh (generates TestMain, elm make), TestRunner.elm (ports), driver.js (QuickJS)
  src/
    main.rs                         # CLI options, Database::new, router + CORS/trace layers
    state.rs                        # AppState (wraps Database)
    error.rs                        # AppError → JSON 404/500
    params.rs                       # Query-parameter helpers (repo labels, pages, ratios)
    assets.rs                       # App shell (index.html), embedded static files, ETag/Cache-Control
    queries/                        # Raw sqlx SELECTs with local row structs (analytics, failures, packages, sessions, stats, watch); live.rs = WS fingerprint
    routes/                         # mod.rs (page paths → shell, /api, /ws, /static; gzip CompressionLayer), api.rs, ws.rs; tests.rs
  static/                           # css/styles.css, js/app.js (WebSocket → Elm ports); compiled into the binary
```

## Architecture: The `run` Command Pipeline

The `run` command is the primary batch-mode entry point. Two concurrent services communicate over an unbounded mpsc channel:

```
iets eval-jobs (streaming JSON)
        │
        ▼
  Checker Service  ── UpdateRequest ──▶  Grouper  ── UpdateJob ──▶  Updater Service
  (Semaphore-limited)                   (holds back                 (JoinSet, concurrency-limited)
                                         group members)
```

The grouper (in `RunConfig::execute`) passes requests through as `UpdateJob::Single`, except those that may belong to an update group (`RunGroups::should_hold`), which wait until the checker finished and are then dispatched as `UpdateJob::Group` (see Update Groups below).

### Checker Service (`commands/run/checker.rs`)

1. Consumes `iets eval-jobs` output stream
2. For each package: checks database backoff, extracts `PackageMetadata`
3. Reads passthru attributes (`skip`, `semver-strategy`, `include-prereleases`, `version-regex`; `components` is read later by the update workflow)
4. Determines `UpstreamSource` from `src.url` or pname
5. Resolves the strategy via `variant_strategy::effective_strategy` (explicit → attr-path suffix, pinned = skip → mkManyVariants = Minor → Latest) and registers the package (`upsert_package_info`)
6. Fetches upstream releases, filters by `SemverStrategy`; records the outcome (`record_no_update` / `record_update_available`)
7. Deduplicates by (pname, version) to avoid updating aliased packages twice: within a run via a shared set, across runs via `Database::proposed_alias` (an alias already proposing the same target, now in its success backoff and not checked, still occupies the slot; so does a package skipped as already proposed); records `passthru.ekapkgs-update.parent` (`RunGroups::note_declaration`)
8. Sends `UpdateRequest` on channel

Progress (`packages_evaluated`, `packages_backoff`) is written to the session every 250 packages.

**Session lifecycle:** `RunConfig::execute` always closes its session. A panicked checker/updater task still finalizes it (`failed`; `close_session` keeps the last progress when the checker totals are lost) and `run` exits with an error. SIGINT/SIGTERM (`shutdown_signal`) aborts the checker, grouper and updater (in-flight updates are dropped) and closes the session as `cancelled` (`supervise_services`, which takes the shutdown future so tests inject it); the end-of-run CVE scan is skipped on a signal too. Sessions record their owner (`owner_host`, `owner_pid`; migration `20261122_session_owner.sql`). Before creating its session, `run` cancels `run` sessions still `running` whose owner is gone (`recover_stale_run_sessions` / `is_stale_run_session`: same host and the PID no longer exists or is our own; sessions without an owner only after 24h without a heartbeat). Sessions of another host or a live PID are never touched, so concurrent runs on one database are safe. Watch keeps closing every interrupted watch session at startup (`recover_interrupted_watch_state`).

**OOM guard:** Checker concurrency is capped at `max(4, concurrency * 2)` via a `Semaphore`. Each task can spawn nix-instantiate processes, so unbounded concurrency exhausts memory.

### Updater Service (`commands/run/config.rs`)

1. Receives `UpdateJob`s from the channel (a group job runs `group_update::perform_group_update` in worktree mode or `perform_branch_group_update` in branch mode, and counts one attempt per member; a panicked job counts one failure per member). With `--dry-run` every job is reported as `DryRun` before any mode-specific work, in branch and worktree mode alike (`dry_run_outputs`)
2. Maintains a `JoinSet` bounded by concurrency (default: `max(1, cpus / 4)`)
3. **Worktree mode** (default): calls `perform_update` — isolated git worktree per package (named `<repo>+<attr>` in multi-repo watch, via `preservation::repo_scoped_name`), supports concurrency and PRs
4. **Branch mode**: calls `perform_direct_update`. It always runs one update at a time (`updater_concurrency`, with a warning if `--concurrent-updates` asked for more), serializes git operations via `Arc<Mutex<()>>`, and has no PR support (update groups are committed on the branch, see Update Groups)
5. Each attempt gets a `PhaseTracker` (phases → `update_phases`) and ends as `UpdateResult::Updated`, `DryRun`, `Skipped` (not attempted, e.g. worktree creation or branch-mode working-tree snapshot failed, or rebuild limit; the `updates` row is left untouched so the next poll/run retries; or the tree already has the target version, see below) or `Failed` (a lasting setup error, a package file or entry point outside the repository, is a `Failed` `fail_setup` so watch backs off instead of re-dispatching it every poll) (recorded via `record_failed_update`, sets failure backoff). A worktree update that built but has no PR (no PR configuration, or declined in `--interactive`) is also `Skipped` (`not_proposed_without_pr_config`): nothing durable is left once the worktree is removed, so it is not recorded as `proposed` and the package stays `outdated`. `run` warns at startup when PR configuration detection fails in worktree mode.
6. Session counters (`packages_attempted/succeeded/failed/skipped`) are incremented live; `run_service` returns `UpdaterStats`. `DryRun` counts as skipped in the session and is listed separately (not as succeeded) in the run summary

### Update Workflow (`commands/update/mod.rs`)

The 9-phase workflow for each package:

1. **MetadataExtraction** — Extract `PackageMetadata` via nix-instantiate
2. **SourceDiscovery** — Determine `UpstreamSource` from metadata
3. **VersionSelection** — Fetch best compatible release (SemverStrategy + filters)
4. **SourceHashUpdate** — Update source hash (see Hash Discovery below) and its other systems' hashes, refresh other changed sources, bump `components`, refresh again (see Secondary FODs below)
5. **DependencyHashUpdate** — Refresh other changed FODs, update dep hashes (cargoHash, vendorHash, npmDepsHash, etc.; their FODs are deferred to this step), then refresh FODs built from them (skipped with `--src-only`)
6. **Build** — Build with patch recovery (detect and remove obsolete patches)
7. **Testing** — Run passthru.tests (if configured)
8. **AgentFix** — *(run/watch with `--claude-fix`, on failure)* Claude Code fix attempt (`claude` runs in its own process group with `kill_on_drop`; on timeout the whole group is killed and reaped)
9. **PrCreation** — Commit + push + create PR via GitHub REST API

Requests carry the current version from the index or checker, which can be stale (a proposal merged since). The updater evaluates the package's version together with its `meta.position` (`get_package_location`) and replaces the request's `current_version` with it (`PackageLocation::corrected_request`), so titles, commits and records name the version the update actually starts from.

With an explicit target version (always the case in `run`/`watch`), the workflow stops after MetadataExtraction with `AlreadyAtVersion` when the evaluated version has already reached it (`version_at_least`). This happens in branch mode when several attributes share one file (`arro3-core`/`-io`/`-compute`): the first update bumps the file, and the rest find it done. The updater records those as an up-to-date observation (`record_no_update`) and returns `Skipped`, not a failure.

Every phase is wrapped with `PhaseTracker::run` / `start` (`commands/update/instrumentation.rs`). The standalone `update` command uses `PhaseTracker::disabled()`. A phase failure is classified into an `UpdateError` (`UpdateError::classify`), and `tracker.last_failure()` gives the caller the structured cause. The final `nixfmt` step is tracked under `PrCreation` (an `InfrastructureError` with component `nixfmt`). Network classification only treats a status code as HTTP when it appears in an HTTP context (`http_status_code`: `HTTP 404`, `status: 404`, `404 Not Found`), so a bare ` 404` in a build log is not a network error.

## Architecture: The `watch` Command (Passive Mode)

The `watch` command is an event-driven alternative to `run`. Instead of evaluating all packages on every cycle, it builds a reverse index and polls upstream feeds. Supports **multi-repo mode** for poly-repo package sets:

```
Config: [[repos]] (name, upstream, entrypoint, select, eval_entrypoint, fork, path)
        │
        ▼
  Repo provisioning ──── auto-clone if missing, auto-fork via GitHub API,
        │                 sync managed checkout to origin/<base>
        ▼
  Per-repo iets eval-jobs ─── one pass: --select (enumeration) + --apply (metadata);
        │                      re-sync + re-index periodically (default: 24h)
        ▼
  upstream_index table (atomically replaced per repo)
  (repo_name, upstream_type, upstream_key → attr_path, version, drv_path, version policy)
        │
        ▼
  Feed Poller ────────────── RSS/Atom (GitHub releases → tags, PyPI) + API (GitLab, SourceHut, directories)
  (Semaphore-limited)         per-entry VersionPolicy, backoff, open-PR guard, alias dedup
        │ new version detected
        ▼
  dispatched per source as polled: on first dispatch create session + spawn run_service;
  sync each repo before its first update; requests streamed ──── per-request overrides for
                                                              eval_entry_point, pr_config, fork
```

**Entry points: enumeration vs evaluation.** `entrypoint` (+ optional `select`, an `iets eval-jobs` `--select` function such as `x: x.buildable`) is only used to *enumerate* packages. Dotted keys in a selected flat set (`"cmake.v4"`) are split into attr paths (`NixEvalItem::flatten_dotted_attrs`). `eval_entrypoint` (defaults to `entrypoint`, required with `select`) is what attr paths are evaluated, located (`meta.position`) and built against — e.g. the repo's `default.nix`. Never evaluate per-package queries through an eager set like corepkgs' `buildable`: it forces every `drvPath`, so each query becomes a full CI eval. The `run` command has the same split via `--select`/`--eval-file`.

**Repo provisioning** (`commands/watch/mod.rs`): On startup, clones repos that don't exist and creates GitHub forks via API if needed. Resolves each `RepoConfig` into a `ResolvedRepo` with absolute paths and `PrConfig` (`managed = true`; legacy single-repo mode is `managed = false`, name `''`, the user's own working copy). Then it closes out state left by a killed process (`recover_interrupted_watch_state`), registers repos in `watched_repos` and forgets repos removed from the config (`retain_watched_repos`, which also drops their `upstream_index` rows).

**Sync** (`commands/watch/sync.rs`): At startup, before every index build and before a repo's first update in each poll cycle, managed checkouts are fetched from the remote matching the upstream, then fast-forwarded (`merge --ff-only`) to `<remote>/<base>`. Divergence, or a non-base branch checked out, is logged and recorded as `diverged`, and the stale tree is kept. History is never rewritten, and unmanaged checkouts are never fetched or merged. Worktrees are created from `HEAD`, so this is what keeps updates current.

**Index builder** (`commands/watch/index.rs`): One `iets eval-jobs` pass per repo with an `--apply` function (`INDEX_APPLY_EXPR`) that extracts version, pname, description, homepage, source URLs, `meta.position`, variants/Python flags and `passthru.ekapkgs-update` settings — no per-package `nix-instantiate` (that took hours). Skips packages defined outside the repo (`meta.position` in the Nix store), pinned variants, opt-outs and packages without a recognizable upstream (the pname→PyPI guess is only allowed for Python packages). The strategy comes from `infer_update_strategy(...).resolve(has_variants)`. Entries (with `drv_path` and the `VersionPolicy`) are written in one transaction by `replace_upstream_index`, which also prunes the repo's stale rows unless the evaluation looks incomplete (stream error, nothing evaluated, or under half the previous count; `prune_decision`). Every indexed package is registered in `updates` (`upsert_package_info`).

**Feed poller** (`commands/watch/feeds.rs`): Uses RSS/Atom feeds for GitHub (`releases.atom`, then `tags.atom` for tags-only projects) and PyPI (cheaper than API), falls back to `UpstreamSource::fetch_all_releases()` for GitLab/SourceHut/DirectoryListing (GitHub and GitLab fall back from an empty or failed releases list to tags: `releases_or_tags`). `poll_source` takes the first feed with an entry some package of the source can use (`releases_usable`: passes its prerelease/regex filters and is its current version or a newer one its strategy accepts); otherwise it tries the next feed, then the API, and uses the first non-empty feed when the API fails or lists nothing (`poll_with`, testable without network). An API 429 or 403 trips the cycle's `ApiGate`: later sources make no API requests (`ApiSkipped`: nothing recorded, they stay due). Feeds carry no prerelease/yanked flags, so before dispatching a feed release to a package that excludes prereleases, `observe` asks the platform once (`release_flagged_prerelease`: GitHub `releases/tags/<tag>` `prerelease`, PyPI `info.yanked`); a flagged release is marked as a prerelease and the target picked again (also keeping it out of the source's recorded latest). Only dispatch candidates are checked, and a failed check dispatches. GitHub tags come from the `/releases/tag/<tag>` link, not the entry title (the title is the release name). `update_version_for_entry` applies the entry's stored `VersionPolicy`. GitHub's `releases.atom` also lists bare tags, so watch can dispatch a version whose release isn't published yet; `get_compatible_release` therefore looks an explicit target up among the tags (`fetch_tag_releases`, GitHub/GitLab) when the releases endpoint doesn't have it.

**Source health** (`upstream_sources.health`, `SourceHealth`): `ok`, `error` (transient: network, rate limit, 5xx), `not_found` (a release endpoint returned 404/410 — `HttpStatusError::is_not_found`, found via `failure_health`: deleted, private, or a wrong owner/repo in the package) or `moved` (polled fine, but a GitHub feed redirected to another repo; `relocation` compares the final feed URL, and `moved_to` holds the new `owner/repo`). Only the feed shows the new name: the GitHub API redirects to `/repositories/<id>`. Release fetchers must return `HttpStatusError` for non-success statuses, not a formatted `bail!`, or 404s are classified as transient. Sources no `upstream_index` row references any more are deleted when an index build prunes (`replace_upstream_index`), so corrected URL parses don't leave dead rows on the dashboard. The dashboard's watch page lists them as "Problem sources" (`problem_sources`).

**Event loop** (`commands/watch/listener.rs`):

- The first poll runs immediately after the startup index. After that, polls run every `poll_interval`, and sync + re-index runs every `index_refresh`.
- **Merged PRs are picked up within minutes** (`refresh.rs`): each index records the commit it was built from (`watched_repos.indexed_commit`, kept by a failed build). A background task checks every `--head-check-interval` minutes (default 5, `0` = off; NixOS `watch.headCheckMinutes`): `git ls-remote` of the managed repo's base branch (no API quota, no fetch); when it moved, sync; when `HEAD` then differs from `indexed_commit`, re-index the repo (a full index takes seconds). Re-indexing registers the merged version as `current_version`, which marks the proposal merged (`upsert_package_info`). Poll cycles reload the sources from the database. With `--webhook-listen`, webhook deliveries trigger the same check for their repo immediately (see GitHub App and Webhooks).
- **Restarts resume from the database.** At startup every repo is synced, but a repo's index is reused when its last build succeeded within `index_refresh`, indexed something, was built at the current `HEAD` (`indexed_commit`), and has the same `watched_repos.index_config` (`index_config`: binary path, entry points, `select`; a new deploy or config change re-indexes; `index_reuse_remaining`). Each repo is re-indexed when its own index expires (`index_deadlines`; repos due within 5 minutes of each other go together), not all repos at the earliest expiry. Each poll cycle only polls sources that are due by `upstream_sources.last_polled_at` (`source_due`: half a poll interval of slack; `not_found` sources daily; `error` sources back off by `consecutive_failures`, `failure_backoff`: the interval doubles per failure after the first, up to 24h); a cycle with nothing due records no `watch_cycles` row. Sources that cannot be reconstructed (`upstream_source_from_index` → `None`) are recorded as failed polls, so they back off too.
- **Current version**: a poll uses the newer of the indexed version and `updates.current_version` (`effective_current_version`, also applied inside `record_observation`/`record_upstream_error`). An update that found the tree already at its target (a merged PR) records the new version, and the stale index must not write the old one back and re-dispatch it until the next re-index.
- **Shutdown**: SIGINT/SIGTERM (`run::shutdown_signal`, broadcast over a `watch` channel) stops dispatching, aborts the cycle's poll tasks and updater service, and closes its session as `cancelled` (`stop_service`); the cycle row records the interruption (`failed`, with the dispatched count and session). A signal wins over polls finishing at the same time (`biased` select), so nothing is dispatched after it. `run_watch_loop` takes the shutdown future and `run_poll_cycle` the source poller (`Poller`, `source_poller` in production), so tests drive a cycle without signals or network. `recover_interrupted_watch_state` remains the fallback for a killed process.
- Each poll is a `watch_cycles` row. Each source's health is recorded in `upstream_sources`, and each package's observation in `updates` (`record_observation`). A source that cannot be observed (poll failed, no releases/tags, not pollable) marks its packages `upstream_error` (`record_unobserved` → `record_upstream_error`, baseline and backoff kept), so newly indexed packages never stay `pending`.
- **Observe-only bootstrap:** the first observation of a package records `baseline_version` and dispatches nothing. Only releases newer than the baseline are dispatched; the backlog is left to `run`.
- `dispatch_decision` (pure, unit-tested) evaluates, in order: Baseline, UpToDate, Backlog, AlreadyProposed, Backoff (same failed version only), Dispatch.
- `resolve_siblings` collapses aliases (same repo, `pname`, current and target version) to the shortest attr path; separate packages from one source (a monorepo's `foo` and `foo-cli`) are not merged.
- **Streaming dispatch:** updates are dispatched as soon as their source has been polled, not after the whole sweep (`Dispatcher` in `listener.rs`), so builds overlap polling. Requests for update-group members (a `group_root` in the database, loaded once per repo per cycle) are held and dispatched as group jobs once the sweep is done (`Dispatcher::finish`). The cycle still waits for every dispatched update before it ends.
- **Lazy sessions:** an update session (`SessionMode::Watch`) and its `run_service` task are created on a cycle's first dispatch. Dispatched requests carry the indexed `drv_path` and the repo's `eval_entry_point`, `pr_config` and `fork`.
- Polling volume: one feed request per source per cycle (~5k sources for corepkgs+ekapkgs), so keep the poll interval in the tens of minutes or more (CLI and NixOS module default: 60). After each poll cycle, the CVE scan runs for due packages (unless `--skip-cve`).

**NixOS deployment**: Third systemd service (`ekapkgs-update-watch.service`) alongside the main daemon and web portal. Same user/group, shared database (`/var/lib/ekapkgs-update/updates.db` via the generated config file), `Restart=always`. `watch.repos` option configures multi-repo mode (`select`, `evalEntrypoint`). Module notes:

- `web.database` defaults to `settings.database` (then the state directory's `updates.db`), so the dashboard reads the database the services write.
- `cachix.cacheName` is passed to watch as `--cachix-cache` too. Watch accepts the same PR-enhancement flags as `run` (`--skip-repology`, `--analyze-rebuilds`, `--max-rebuilds`, `--skip-cachix`, `--cachix-cache`, `--audit`, `--no-security`, `--skip-cve`), collected in `WatchConfig.pr_enhancements`; the directory diff stays off in watch.

- `run.enable` (default true) / `run.startAt`: the batch `run` service is a one-shot; give it a timer or disable it when watch covers the same repos. `packagesFile` is only required when it is enabled.
- `extraPackages`: added to the run/watch services' `PATH` (e.g. `pkgs.claude-code` for `--claude-fix`; `environment.systemPackages` is not visible to services).
- `git.userName`/`git.userEmail`/`git.githubTokenCredentialHelper`: a generated git config (`GIT_CONFIG_SYSTEM`) with the commit identity and a credential helper that answers github.com HTTPS auth with `GITHUB_TOKEN` from `environmentFile`.

## Key Patterns

### Hash Discovery (`hash_discovery.rs`, `commands/update/hash_workflows.rs`)

This is intentional, not a bug:

1. Write an **invalid hash** (`sha256-AAAA...A`) into the .nix file
2. Run `nix-build` — it **must fail** with a hash mismatch
3. Parse the **correct hash** from stderr (`got: sha256-<actual>`)
4. Write the correct hash back
5. Build again to verify

Applies to: source hash, cargoHash, vendorHash, npmDepsHash, nugetDepsHash (only when `nugetDeps` is a hash: a deps file or derivation is skipped with a warning, `package::nuget_deps_hash`), composerDepsHash. Each has an `update_*_if_needed` function in `hash_workflows.rs`; the dependency hashes share `discover_dependency_hash`, which writes the old hash back when no hash can be discovered (the fake hash is never left in a file; `fod_refresh` does the same). The discovered hash is the `got:` of the mismatch whose `specified:` is the fake hash (`hash_discovery::extract_hash_for`), and dependency builds run with `--keep-going`: another stale FOD failing first must not lend its hash. A dependency attribute not bound literally (`inherit (data) cargoHash;`) is replaced by value where it appears exactly once among the package's `.nix`/`.json` files.

A version the `.nix` file doesn't bind is looked for in a JSON data file next to it (`"version": "<old>"` exactly once, beads-rust's `hashes.json`); the source hash is then replaced by value in that file.

If the source hash can't be discovered, `update_source_hash` writes the package directory's `.nix`/`.json` files back as they were (no fake hash, no half-done version bump). A source hash shared by several system-keyed lines (greptile-cli's one JS file) is replaced on the build machine's line; the others follow via the per-system refresh.

A version bump whose `src` hash comes back unchanged is an error: the fetcher does not follow the version (a literal tag/rev/URL), and the "update" would only relabel the old source.

### Secondary FODs and Components (`commands/update/fod_refresh.rs`, `components.rs`)

Packages often fetch more than `src`: docker fetches `moby-src` (`tag = "docker-v${version}"`), opencv a `contribSrc`, mission-center two `fetchCargoVendor`s under custom names. No annotation is needed for these:

1. Before any edit, `ClosureSnapshot::capture` instantiates the package and records its `.drv` closure (`nix-store --query --requisites`).
2. After the version bump, FODs (`nix derivation show`, `outputs.out.hash`; base16 hashes and `r:` recursive algos are normalised to SRI, `sri_hash`) whose `.drv` is not in the snapshot have stale hashes, unless the hash is one this update already discovered (`confirmed`: the `src` hash, the `update_*_if_needed` results).
3. Each one's hash is located **by value** (SRI, nix32 or base16; exactly one string literal among the `.nix`/`.json` files of the package directory, or exactly one on a line keyed by the build machine's system; a FOD carrying the fake hash itself is located by a single `lib.fakeHash`/`lib.fakeSha256`/`lib.fakeSri`, which unfinished packages leave in), replaced with the fake hash, the FOD's new `.drv` is realised, and the `got:` hash written back. FODs depending on another changed FOD go after it (`next_stale_fod`); `FodScope::Sources` (end of SourceHashUpdate) only takes FODs without changed FOD inputs, `FodScope::All` (end of DependencyHashUpdate) the rest.

The main `src` hash is also found by value when it isn't bound by `hash`/`sha256`/`outputHash`/`src-hash` (docker's `cliHash`); not finding it is an error.

**Per-system hashes** (`fod_platforms.rs`): hash discovery only sees the build machine's FOD. When the hash it wrote sits on a `<system> = "<hash>";` line, the neighbouring system-keyed lines (mistral-vibe's `wheelHashes`, codex's librusty_v8 `hashes`/`srcBindingHashes`) are updated too: the package is instantiated with `--system <system>`, the FOD still carrying that line's old hash is found in its closure, and its URL is prefetched with `nix store prefetch-file`. Only plain downloads qualify (flat hash, http(s) URL, no `postFetch`/`downloadToTemp`; `Fod.download`); other systems are warned about and left alone. This runs for the main `src` (unless `platform-hashes` handles it) and for every refreshed FOD.

Sources pinned independently of the package (docker's runc/containerd, radare2's sdb, codex's librusty_v8) are declared as components (`passthru.ekapkgs-update.components`). `src` is the attr path of the component's FOD relative to the package; let-bound fetchers must be exposed (`passthru = { inherit sdb; }`). Components are only considered while the package itself is updated, and their bumps are listed in commit messages and PR bodies (`ComponentUpdate`, `UpdateOutcome.components`). A component picks its new version one of two ways:

- **`pinned-by`** (`pins.rs`): follow the pin in a file of the package's new source (`pinned-by.source`, default `src`; it is realised first, after the other sources were refreshed). Built-in formats, chosen by `format` or the file name: `meson-wrap` (`*.wrap`: `[wrap-git]` `revision`, `[wrap-file]` `wrapdb_version`/`source_url`), `cargo-lock` and `go-mod` (both need `name`, the crate/module). `regex` (first capture group) is the fallback when there is no format or its lookup fails. The pin may be a commit (replaces a fetched commit), a tag or a version (replaces the version in the component's tag, keeping its scheme, or, for URL-only fetchers like `fetchurl`, every standalone occurrence of the version from the FOD's name: codex's `version` and `baseUrl`). The component follows the pin exactly, downgrades included.
- Otherwise **release selection**: the upstream comes from the FOD's URL (or `github-repo`), and the version from `get_compatible_release` with the component's own `semver-strategy`/`version-regex`/`include-prereleases`. Commit-pinned components are skipped with a warning.

The tag literal (or, failing that, the version literal) must appear exactly once in the package file. The refresh pass then rehashes the component's FOD (and everything built from it, like a vendored `cargoHash`).

```nix
passthru = {
  inherit sdb;
  ekapkgs-update.components = {
    sdb = { src = "sdb"; pinned-by.file = "subprojects/sdb.wrap"; };
    librusty_v8 = { src = "RUSTY_V8_ARCHIVE"; pinned-by = { file = "codex-rs/Cargo.lock"; name = "v8"; }; };
    runc = {
      src = "docker-runc.src";
      pinned-by = { source = "moby-src"; file = "Dockerfile"; regex = "ARG RUNC_VERSION=(.+)"; };
    };
    containerd = { src = "docker-containerd.src"; semver-strategy = "minor"; };
  };
};
```

### Update Groups (`groups.rs`, `commands/run/grouping.rs`, `commands/run/group_update.rs`)

Packages that must be updated together (corepkgs' `aws-c-*` and `aws-crt-cpp`) declare `passthru.ekapkgs-update.parent = "<attr path>";`. Members of a group are proposed as one PR, with one commit per member.

- **Resolution** (`groups::resolve_parent`): a parent is looked up relative to the child's scope first (`a.b.c` with parent `p` → `a.b.p`, `a.p`, `p`; quoted segments are kept by `split_attr_path`). A parent that matches no evaluated attr path still groups its children as a label (with a warning). `resolve_roots` follows chains to the root; a cycle's root is its shortest, then lexically smallest, member. Every member and the root map to the root.
- **Storage** (migration `20261119_update_groups.sql`): `updates.group_parent` (resolved parent) and `updates.group_root`, replaced per repo by `replace_groups`. Watch writes them at every index build from all evaluated derivations (`INDEX_APPLY_EXPR`'s `parent`, `declared_parent`), unless the evaluation looks incomplete. `run` merges the stored declarations with the ones it evaluated (backed-off packages are not evaluated) in `RunGroups::finish`; when the checker's eval stream failed (`RunGroups::note_incomplete`), the stored groups are kept as they are.
- **Dispatch** (`grouping::group_jobs`): requests whose package has a `group_root` become one `GroupRequest` per root. It contains every member whose `latest_upstream_version` (from the database) is newer than its `current_version` (except members still backing off from a failure at that same version, unless they triggered the job; `GroupMember::pending_update`), not just the triggering ones, so a regenerated PR keeps the members it proposed before. `verify` lists every tracked member and the root. Watch holds group members until the poll sweep is done (singles stream immediately); `run` holds requests for packages that declare a parent, were grouped before or are named as a parent, and groups them at the end. A root evaluated before its children on a first run is dispatched alone.
- **Executor** (`perform_group_update` in worktree mode: one worktree, `group-<root>`; `perform_branch_group_update` in branch mode: the checkout itself). Both share `run_group` (member loop + verification), parameterized by a `GroupWorkspace` (directory, entry point, `Committer`). Members are updated in dependency order (`nix-instantiate` + `nix-store --query --requisites`, `groups::dependency_order`, alphabetical fallback) with tests off, each committed as `<attr>: <old> -> <new>` (a Claude fix of a member's own build is part of its commit). Then every `verify` attr is built (a root that doesn't evaluate is skipped), then their `passthru.tests`. One Claude fix is allowed for the verification, committed as `<root>: fix build with updated dependencies`. Phases of the verification and PR are recorded under the root's attr path.
- **All-or-nothing**: if any member or the verification fails, nothing is proposed. The member that failed records its own error. The others record `group <root> failed: <attr> (<phase>)` (an `InfrastructureError` with component `group`, `member_failure`) and back off.
- **PR**: stable branch `update/group/<root>` (`git::group_branch_name`), rebuilt from the current base and force-pushed (`force_push_branch`). An open PR from that branch is found (`github::find_open_pull_request`) and its title/body replaced (`update_pull_request`); otherwise a new PR is opened (draft if tests failed; an existing PR's draft state is not changed). The directory diff is not run for groups (it compares against `HEAD~1`).
- **Branch mode** (`perform_branch_group_update`): under the git mutex a `branch_git::GroupCheckpoint` records `HEAD`, the checked out branch and the dirty paths (a `DirtySnapshot`). Each member's commit stages only what the group dirtied since then (plus its package directory, like singles) and records the commit it made; so does the verification's Claude fix. No PR; success records every updated member like a branch-mode single (`record_success`, `proposed`). Failed `passthru.tests` fail the group (no draft PR to report them in). On failure `GroupCheckpoint::undo` runs `git reset --soft <start>` and then reverts the paths the group introduced (paths dirty before it are kept). `plan_undo` (pure) only allows the reset when the same branch is checked out, `start` is an ancestor of `HEAD` and `rev-list start..HEAD` is exactly the group's own commits; otherwise nothing is touched and an error is logged for manual cleanup. With `--preserve-failures`, the failure is preserved first (see Export), filed under the same member as in worktree mode (`preserved_member`).

### PyPI File Names (`pypi/sdist.rs`, `hash_workflows::align_pypi_src`)

`fetchPypi` builds its URL from its arguments, but projects often rename their sdists between releases (`sphinxcontrib-openapi-0.8.4.tar.gz` → `sphinxcontrib_openapi-0.10.0.tar.gz`), change the archive type (`.zip` → `.tar.gz`), or change their wheel's python tag (`py2.py3` → `py3`). For PyPI upstreams, `update_source_hash` therefore checks the bumped `src` URL against the files PyPI lists for the new version (`/pypi/<name>/<version>/json`) before discovering the hash:

- **sdist** with the same name under PEP 503 normalisation → `update_fetch_pypi_sdist()` sets `fetchPypi`'s `pname` (taking it out of `inherit (finalAttrs) pname version;`) and/or `extension`.
- **wheel** (`format = "wheel";`) with the same name and ABI/platform tags (`plan_wheel_fix`, preferring the current python tag) → `update_fetch_pypi_wheel()` sets `pname`, `python` and/or `dist`. In corepkgs' `fetchPypi`, `dist` defaults to `py2.py3` independently of `python`, so a tag change sets both; the target `dist` is PyPI's `python_version`. A `python`/`dist` that isn't a string literal (`dist = python;`) or is inherited is refused rather than duplicated.
- Either way the new URL is re-evaluated to confirm the filename (and wheel directory).
- No sdist at all for an sdist-based package (wheel-only release) → the update fails with an explicit error instead of a 404.
- Unrelated names, missing platform wheels, PyPI errors and `fetchPypi` calls that can't be rewritten are logged and left to the source build.

### Nix AST Rewriting (`rewrite/`)

Uses **regex with rnix AST validation**, not pure AST transformation:

1. Parse content with `rnix::Root::parse()` to validate syntax
2. Build a regex to match `attr_name = "value";` patterns; the name must start a binding (line start, whitespace, `{` or `;` before it), so `version` never matches `node_version`, `minversion` or a dotted `src.version` (same for `rev` in `rev_update`/`rev_fixup`)
3. Perform text replacement
4. Re-parse to validate the result is still valid Nix

Key functions: `find_and_update_attr()`, `try_update_rev_attr()`, `try_fixup_stale_rev()`, `align_ref_with_tag()`, `remove_patch_from_array()`, `replace_maintainers_with_empty()`, `replace_teams_with_empty()`, `update_variant_attr()`, `add_variant_entry()`, `update_fetch_pypi_sdist()`, `update_fetch_pypi_wheel()`.

Error type: `RewriteError` with an `is_not_found()` method. When `NotFound` is returned for a version attribute, `file_update.rs` falls back to searching sibling files (mkManyVariants pattern).

### Git Refs and Release Tags (`commands/update/file_update.rs`, `rewrite/rev_fixup.rs`)

After the version bump, `update_nix_file` checks the file's git ref against the release tag (`ReleaseTag`, only for GitHub/GitLab/SourceHut upstreams via `UpstreamSource::git_repo_name`; other upstreams' `tag_name` is a bare version). A stale commit-SHA `rev` is rewritten to reference the version (`try_fixup_stale_rev`), and a ref built from an older tag scheme is rewritten to the release's tag (`align_ref_with_tag`: HDF5's `rev = "hdf5_${version}"` → `"${version}"`; a literal still naming the old version is replaced by the tag outright, vibe-kanban's `tag = "v0.1.44-20260424091429"` → `"v0.1.46.1794744340"`). Both apply only when `is_single_ref_from` holds: the file has exactly one `rev`/`tag` and binds `repo = "<upstream repo>"`. Files with several refs (opencv's pinned third-party sources) are never touched.

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

### Config File Values as CLI Defaults (`main.rs`, `cli.rs`)

`main.rs` parses arguments, loads the config file, then re-parses with `cli::apply_config_defaults`, which rewrites clap defaults so the precedence is CLI flag > config file > built-in default for `database`, `file`, `select` and `eval_file` (every subcommand that has those arguments), `[github] pr_author` (`--pr-author` on `run`/`update`/`watch`), `[autofix] max_attempts` and `[watch] poll_interval`/`index_refresh_hours`. `[nix] builders`/`max_jobs` are applied by `apply_nix_build_args`. Without this, the NixOS module's generated `database`/`file` were silently ignored.

### GitHub App and Webhooks (`github/app.rs`, `github/auth.rs`, `commands/watch/webhook.rs`)

`[github_app]` in the config file (`client_id`, `private_key_file`, `webhook_secret_file`; overridable by `EKAPKGS_GITHUB_APP_CLIENT_ID` / `EKAPKGS_GITHUB_APP_KEY_FILE` / `EKAPKGS_GITHUB_WEBHOOK_SECRET_FILE`) is loaded by `main.rs` (`auth::configure`). Every GitHub credential goes through `github::auth`: `token_for(owner, repo)` (PRs, reviews, forks: installation token, else `$GITHUB_TOKEN`), `read_token()` (release lookups: `$GITHUB_TOKEN`, else any installation token) and `git_env(owner, repo)`. `git::create_and_push_branch` and the standalone PR path commit and push with `git::github_git_env`: the bot's identity (`<slug>[bot]`, `<id>+<slug>[bot]@users.noreply.github.com`) and a `GIT_CONFIG_*` credential helper reading the token from `$EKAPKGS_GIT_TOKEN` (never argv or URLs). Without an app it returns nothing and git's own config (the NixOS credential helper) applies. Do not read `GITHUB_TOKEN` directly in new code.

`watch --webhook-listen <addr>` serves `POST /github/webhook` behind a TLS-terminating proxy. Deliveries are verified (`X-Hub-Signature-256`, constant time), deduplicated by `X-GitHub-Delivery` in `webhook_deliveries` (kept 7 days, with an `outcome`), matched to a watched repo by `pr_config` owner/repo, and answered at once:

- a push to the base branch, or a merged PR → the head watcher checks that repo now (forced sync, then re-index if `HEAD` moved);
- a PR closed unmerged → `record_pr_closed`: the `proposed` row (by `pr_number`) becomes `failed` with `failed_version` = the proposal and failure backoff, so the same version is retried after the backoff and a newer release is dispatched at once.

GitHub never redelivers on its own; the periodic head check covers missed deliveries. NixOS: `githubApp.{clientId,privateKeyFile,webhookSecretFile}` (secrets via `LoadCredential=`) and `watch.webhook.{enable,host,port}` (default `127.0.0.1:3041`). App permissions: Contents and Pull requests read & write; events: Push, Pull request.

### Version Comparison (`vcs_sources/version_order.rs`, `vcs_sources/mod.rs`)

`compare_version_components` is the single version order, used for release sorting (`find_best_release`), `is_version_acceptable`, watch dispatch (`is_newer`, `update_version_for_entry`), `UpdateStatus::observed` and `version_at_least`. Versions are tokenized at `.`/`-`/`_`/`+`/`~` and digit/letter boundaries; numbers compare numerically, trailing zeros of a number run don't count (`1.0 == 1.0.0`). Prerelease words rank below the release, separated or not (`dev < alpha/a<n> < beta/b<n> < pre/preview < rc < end`, so `3.0.0rc1 < 3.0.0` and `1.3.0-rc1 < 1.3.0`); other words rank above the end but below a number (`1.1.1 < 1.1.1w`, `1.2.3 < 1.2.3-unstable-…`, `0-unstable-2024-02-01 < 0.1`). It is a total order.

`is_version_acceptable` accepts dot-less versions (`137`, `20240115`, `2024-01-15`) under Latest/Major only, and never across schemes (dotted ↔ dot-less, dot-less date ↔ dot-less counter). Minor/Patch compare the first one/two dot-separated numbers, so 4-component and leading-zero date versions (`2024.01.15`) work. The version written for a release is `Release::resolved_version(version_regex)` (checker target, update workflow, variants), so `version-regex` `v(.*)-release` writes `1.2.3`. `extract_version_from_tag` drops a name ending in digits (`lz4-1.9.4` → `1.9.4`, `gtk4-4.12.0` → `4.12.0`).

### Explicit Upstream Source

When `src.url` can't be parsed (mirror:// schemes, custom domains), packages can set `passthru.ekapkgs-update.github-repo = "owner/repo"` to explicitly specify the GitHub repository for release discovery. This takes priority over URL-based detection.

### Database State and Backoff (`database/updates.rs`)

`updates` is keyed by `(repo_name, attr_path)`. `repo_name = ''` is used by `run` and legacy watch; multi-repo watch uses the config name (`default` is reserved — the dashboard labels `''` with it; rejected by `ConfigFile::validate` and a NixOS module assertion; names are limited to `[A-Za-z0-9][A-Za-z0-9_-]*` so the `+` separator stays unambiguous). All `updates` methods take `repo_name` first. `autofix_queue` / `autofix_attempts` are repo-scoped the same way (via `FailureArtifacts.repo_name`; `autofix status/history/enqueue` take `--repo`, and validation builds the worktree's copy of the repo's entry point, `[[repos]]` `eval_entrypoint`/`entrypoint` or the `--file` path relative to its repo; preserved failures live under `failed/<session>/[<repo>+]<attr>/`; older `<repo>-<attr>` dirs are still found via `manifest.json`).

`status` values:

- `pending`: indexed, not yet compared
- `up_to_date`
- `outdated`: newer compatible release, not proposed
- `proposed`: built and proposed (worktree mode: only once the PR exists — a push/PR failure is recorded via `record_failed_update` with phase `pr_creation`, and an update without a PR is not recorded at all; branch mode: once committed); becomes `up_to_date` when `current_version` catches up

Update branches (`git::update_branch_name`, `update/<attr>/<version>`) are created with `checkout -B` and force-pushed, and `cleanup_worktree` deletes the local `update/*` branch checked out in a removed worktree, so retrying a version whose push or PR failed works. Before pushing, `create_pr_for_update` looks for an open PR from the branch (`github::find_open_pull_request`) and reuses it without pushing over it. It then scans the repository's open PRs by the PR author (`github::existing_prs::scan`: search API with `author:`, then an exact title match `<attr>: <old> -> <new>`), so PRs from other branches, forks or deployments are found and contributors' PRs never count. The author is `--pr-author` / `[github] pr_author`, else the configured GitHub App's bot (`<slug>[bot]`), else the login of the token's user (`GET /user`, looked up once). An open PR proposing the same or a newer version is reused (no push, no new PR); older ones are listed as `Supersedes #N` in the new PR's body. A failed scan or an unknown author is logged and the PR is opened as before. The standalone `update --create-pr` runs the same scan. Group PRs are not scanned.
- `failed`
- `upstream_error`

Two kinds of backoff:

- **Check backoff** (`record_no_update`): 2 → 4 → 6 days, by time since the last check.
- **Failure backoff** (`record_failed_update`): 2 → 4 → 6 days, by `failure_count`. Also sets `failed_version` and `last_error`, and writes `update_logs`; the key is the drv path, or `<repo>:<attr>@<version>` when there is none.

Successful updates (`record_successful_update`) reset `failure_count` and back off 2 days. Watch ignores the check backoff and honours the failure backoff only for the same `failed_version`.

Failures that weren't the package's fault (a broken nix daemon) are retried with `ekapkgs-update reset-failures` (`Database::reset_failures`, `FailureResetFilter`): it clears `next_attempt` and `failure_count` of the matching `failed` rows and keeps `status`/`failed_version`/`last_error` until the retry overwrites them. Filters (`--repo`, `--attr`, `--session`, `--since`/`--until`, `--error-match` against `last_error` or the last failure's full log) combine with AND; no filter needs `--all`; `--dry-run` only lists.

The web dashboard (`ekapkgs-update-web`) reads these tables directly. Keep the migration comments in `migrations/20261111_dashboard_data.sql` in sync with the column semantics.

Databases from before 20261111: `20261113_legacy_failure_backfill.sql` marks packages whose newest failure log is newer than `last_attempted` as `failed`. After the migrations, `Database::new` calls `repair_derived_state` (`database/updates.rs`), which does the parts that need Rust. It merges `proposed` rows the tree has reached (`version_at_least`) and fills a missing `last_error` with `summarize_error`. It is idempotent and best effort. Never edit an applied migration, because sqlx checksums the whole file, comments included. If one was edited anyway without changing its schema, add the old checksum to `SUPERSEDED_MIGRATION_CHECKSUMS` (`database/mod.rs`); `Database::new` rewrites it to the current one before migrating (20261111's column comments, c358116).

### CommitStrategy (`cli.rs`)

- **Worktrees** (default): Each update in an isolated git worktree. Full concurrency and PR support.
- **Branch**: Direct commits to the current branch. No PRs. Updates run one at a time: `updater_concurrency` (`commands/run/config.rs`) clamps `--concurrent-updates` to 1 in `run_service`, for both `run` and `watch`, and logs a warning. The checker still uses the requested value. All updates share one working tree, and `branch_git::DirtySnapshot` treats every path dirtied after an update started as that update's own. Deletions the update already staged (a Claude fix's `git rm`) are not passed to `git add`, whose unmatched pathspec would abort it, and a failed `git add` fails and reverts the update rather than committing a partial index. Two updates in flight would commit each other's files, or revert them mid-build. Git operations are also serialized via `Arc<Mutex<()>>`. Update groups commit one commit per member on the branch and, when they fail, reset the branch to where the group started (only over the group's own commits; see Update Groups).

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
7. *(if `--tests`)* Build `passthru.tests` for each successfully built package. Results and the approval only say tests passed when some ran (`tests_summary`). Test failures prompt post/edit/skip on a terminal; without one (batch runs) the REQUEST_CHANGES review is posted
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

### CVE Scans (`cve/scan.rs`, `cve/store.rs`, `commands/cve_scan.rs`)

Every tracked package with a GitHub, GitLab, SourceHut or PyPI upstream (`SCANNABLE_UPSTREAMS`) and a `current_version` is checked against OSV.dev. The package and version come from the `updates` row, so no extra evaluation is needed.

- **Query** (`osv_packages`): GIT ecosystem with the repo URL (`https://github.com/owner/repo`, `https://<instance>/<key>`, `https://git.sr.ht/~u/r`), where OSV matches the version against the repo's tags; PyPI by name. Python packages (`python*Packages.*`, or a PyPI upstream) are queried on PyPI **only**: a GIT query for a monorepo (`langchain-*`) reports the whole repo's advisories.
- **Batch**: `POST /v1/querybatch` (1000 queries per request, paginated by `page_token`) for both `current_version` and a newer `latest_upstream_version`. That gives `fixed_in_latest`, i.e. whether the available update fixes it.
- **Records**: `GET /v1/vulns/{id}`, cached in `osv_vulnerabilities` and refetched only when the batch reports a different `modified`. Withdrawn records are ignored.
- **Merge** (`merge_vulns`): records aliasing each other (CVE, GHSA, PYSEC, CURL-CVE-…) are merged with union-find into one row keyed by the CVE id when there is one.
- **Severity** (`osv::best_cvss_score`, shared with the PR-body CVE diff): the highest CVSS v3 score (vector via `osv::parse_cvss_vector_score`, rounded to one decimal), else the highest CVSS v4 score (`cvss4::score`), else the highest `database_specific`/`ecosystem_specific` severity label, else `unknown`. `cvss_vector` records which version the score came from (the 20261117 column comments still say "v3"; it is applied, so it was left alone). `cvss4.rs` follows the reference `cvss40.js` step for step, including its float arithmetic and rounding. It matched all 579,918 vectors of the Red Hat `cvss` library's `tests/vectors_*4` files, and a sample of them is in its unit tests. Keep it exact: the dashboard sorts by score. A CVSS score of 0.0 (no C/I/A impact) takes the label's rating. `20261118_osv_cvss_v4_rescore.sql` dropped cached records without a CVSS score or rated `unknown` so they were rescored.
- **State**: `package_vulnerabilities` rows are replaced per package on each scan (keeping `first_seen`). `updates.cve_scanned_at`/`cve_scanned_version`/`cve_scan_error` hold the scan state. Rows whose `version` is no longer `current_version` are stale and never shown.
- **When**: a package is due when it was never scanned, its version changed, or the last scan is older than 24h (`ScanOptions::max_age`; `--force` rescans everything). Scans run from `ekapkgs-update cve-scan [--repo] [attrs…]`, at the end of `run` (repo `''`) and after every watch poll cycle (cheap when nothing is due). `--skip-cve` on `run`/`watch` disables both this and the PR-body CVE section.
- Directory-listing upstreams (GNU FTP, SourceForge) have no OSV package identity and are not scanned.

The dashboard's packages page shows a "Packages with high-severity CVEs" table (`/api/packages` → `high_severity`, following the page filters, top 50), and the package page a "Known vulnerabilities" panel (`/api/packages/:attr` → `vulnerabilities`).

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

Requires `--preserve-failures` to have been set during the `run` that produced the failure. `preserve_failure` moves the failed worktree into `failed/<session>/…/worktree` with `git worktree move`, so it stays a working git worktree (autofix and `apply` run `git diff`/`checkout` in it) and the original location is not leaked. If git cannot move it (submodules, another filesystem), it is copied and the original kept, because the copy's `.git` file still points at it.

Branch mode (`preservation::preserve_branch_failure`, singles and groups) has no worktree to move, so before the undo/revert, under the git mutex, it creates a detached worktree at the start commit (`<artifacts>/worktree.staging`) and recreates the failed changes there (`BranchChanges::replay_into`): a group's member commits are cherry-picked with their messages (`GroupCheckpoint::replay_into`), then the paths the update dirtied and did not commit are copied (`DirtySnapshot::copy_introduced`). Paths dirty before the update (the user's work, even when a member's commit swept it in from its package directory) are left out. That worktree then goes through `preserve_failure`, so the artifacts match worktree mode's. A failed preservation removes the staging worktree and is only logged; the undo and the recorded outcome are unchanged. Tests file artifacts under a temp `failed/` root (`preserve_failure_in`, `list_preserved_failures_in`, `GroupSteps::failed_root`).

## Important Types

| Type | Location | Purpose |
|------|----------|---------|
| `PackageMetadata` | `package/mod.rs` | Metadata from Nix eval (version, hashes, passthru attrs) |
| `UpstreamSource` | `vcs_sources/mod.rs` | Enum: GitHub, GitLab (multi-instance), SourceHut, PyPI, DirectoryListing |
| `SemverStrategy` | `vcs_sources/mod.rs` | Enum: Latest, Major, Minor, Patch |
| `Release` | `vcs_sources/mod.rs` | Upstream release with version extraction and matching |
| `UpdatePhase` | `commands/update/types.rs` | Workflow phases (MetadataExtraction … PatchRecovery, Testing, AgentFix, PrCreation); snake_case `as_str()` stored in the DB |
| `UpdateOutcome` | `commands/update/mod.rs` | Result of `update_from_file_path`: removed patches, test result, component bumps |
| `ComponentUpdate` | `commands/update/components.rs` | A component bumped along with its package (`name: old -> new`) |
| `ClosureSnapshot` | `commands/update/fod_refresh.rs` | Pre-bump `.drv` closure used to find changed FODs |
| `UpdateError` | `commands/update/errors.rs` | Structured errors with LLM-friendly serialization |
| `RewriteError` | `rewrite/error.rs` | Parse, NotFound, InvalidResult, Regex, Structural |
| `RunConfig` | `commands/run/config.rs` | Batch mode configuration |
| `UpdateConfig` | `commands/update/config.rs` | Single-update configuration |
| `UpdateRequest` | `commands/run/types.rs` | Message from checker → updater (attr_path, versions, per-repo routing) |
| `GroupRequest` / `UpdateJob` | `commands/run/types.rs` | An update group dispatched as one job (root, members, attrs to verify); updater input (single or group) |
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
| `NixEvalDrv` | `nix/nix_eval_jobs.rs` | Deserialized `iets eval-jobs` output per package (nix-eval-jobs JSON format) |
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
| `UpstreamIndexEntry` | `database/mod.rs` | Row from upstream_index table (repo_name + source → attr_path, drv_path, strategy/regex/prereleases → `version_policy()`) |
| `NewUpstreamIndexEntry` | `database/mod.rs` | Entry passed to `Database::replace_upstream_index` |
| `IndexTarget` | `commands/watch/index.rs` | What to index: repo name, enumeration file, select, repo dir |
| `VersionPolicy` | `vcs_sources/mod.rs` | Semver strategy + prerelease + regex; `best_release` picks the target |
| `UpdateStrategyInference` | `variant_strategy.rs` | Shared strategy decision (explicit / variant / attr suffix / pinned) for `run` and `watch` |
| `EvalJobsOptions` | `nix/run_eval.rs` | `iets eval-jobs` `--workers`/`--select`/`--apply` |
| `ReviewConfig` | `commands/review.rs` | PR review configuration (repo, PR#, tests, best-practices flags) |
| `RepoCoords` | `commands/review.rs` | Parsed repository owner/repo coordinates |

## External Dependencies

Runtime tools (provided by Nix dev shell):

- `nix-instantiate` — Nix expression evaluation
- `nix-build` — Package building (supports `--system` for cross-platform eval)
- `nix store prefetch-file` — URL hash prefetching for platform-hashes
- `iets eval-jobs` — Parallel package evaluation (streaming JSON in the nix-eval-jobs format). Always run with `--impure`: unlike nix-eval-jobs, iets evaluates a file purely by default, so `builtins.currentSystem` (used by package sets' default `system`) is missing and every job fails
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
- **OSV.dev** — CVE vulnerability checking (PR-body diff with a 24h cache; `querybatch` + `vulns/{id}` package scans)
- **Repology** — Cross-distribution version validation (72h cache, 1 req/sec rate limit)

## Testing

- **Unit tests**: `#[test]` with inline assertions. Located in same file or `tests_*.rs` siblings.
- **Table-driven tests**: `rewrite/tests_attributes.rs`, `tests_maintainers.rs`, `tests_patches.rs`, `tests_rev.rs`.
- **Test seams** (no mocking frameworks): watch poll cycles take a `Poller`; `UpdaterServiceConfig.executor` (`#[cfg(test)]`, `JobExecutor`) runs jobs in place of the update workflow so a test can hold one in flight (`listener.rs` shutdown tests); group updates take their nix steps (commit order, member edit, verification) through the private `GroupSteps` trait (`NixSteps` in production), so `group_update.rs` tests drive branch-mode groups in a temp git repo against in-memory SQLite. `commit_order_follows_the_derivation_graph` runs the real `commit_order` (`nix-instantiate` + `nix-store --query --requisites`) on a small expression of bare `derivation`s in a temp dir (instantiated only, nothing built, no network); it prints a notice and passes when nix cannot instantiate and query the store there (no nix on `PATH`, or the build sandbox without a daemon).
- **Integration tests**: `tests/migrate_tests.rs` with fixture files in `tests/migrate/`; `tests/watch_tests.rs` for the upstream index, version policy, dispatch guards and update-group storage/dispatch (in-memory SQLite).
- **Web crate tests**: `routes/tests.rs` drives every API endpoint, the app shell and static assets via `tower::ServiceExt::oneshot` against in-memory SQLite with the real migrations (a fixture covering every table, plus an empty database), including response compression. The Elm frontend has table-driven tests in `frontend/tests/` (Format, Route, Session/Item); run them with `nix develop --command ekapkgs-update-web/frontend/test-runner/run.sh` (elm + QuickJS, no node) or `nix build .#checks.x86_64-linux.web-frontend-tests`.
- **Web crate frontend**: `build.rs` compiles `frontend/` with `elm`, which the dev shell provides on x86_64-linux and aarch64-linux (`nix/overlay.nix` adds upstream's aarch64 binary, which ekapkgs lacks). `EKAPKGS_WEB_ELM_JS` can point at a prebuilt `elm.js` instead; a missing file there is an error. Without either, the build warns and embeds a stub `elm.js` (a "frontend not built" notice), so `cargo build/test --workspace` work without `elm`. An Elm compile error always fails the build. `EKAPKGS_WEB_REQUIRE_ELM=1` makes a missing `elm` fatal too. After changing `frontend/elm.json`, run `nix/web-frontend/update.sh`.
- **Run**: `cargo test --workspace`; Elm: `ekapkgs-update-web/frontend/test-runner/run.sh` (`nix flake check` runs it in the sandbox)

## Concurrency and Resource Gotchas

1. **OOM risk** — Checker tasks spawn nix-instantiate subprocesses. Concurrency MUST be limited via Semaphore (`max(4, concurrency * 2)`).
2. **File descriptor limit** — `init::increase_fd_limit()` raises soft RLIMIT_NOFILE at startup. Without this, concurrent worktree operations hit "Too many open files".
3. **Branch mode is serial** — Branch mode never runs two updates at once (`updater_concurrency`). The mutex alone is not enough: builds and Claude fixes run outside it, and snapshot-based staging and revert would claim another update's files. All git operations also go through `Arc<Mutex<()>>`. Do not bypass either guard. A branch-mode update also holds the checkout lock (gotcha 7) for its whole duration, since it edits the shared tree.
4. **Extra nix-build args** — Set once via `OnceLock` in `commands/update/build.rs`. Second call is a no-op.
5. **Database pool** — `Database` is `Clone` (shared pool). Safe to pass across async tasks.
6. **Never `set_current_dir` in run/watch code** — the cwd is process-wide and updates run concurrently. Pass the directory to each subprocess (`Command::current_dir`) instead. `build_and_get_outputs(work_dir, …)` runs `nix-build` in `work_dir` and writes its `result*` out-links to a private temporary directory, so concurrent builds never share them. Only the single-shot `apply` command changes the cwd (`retry` passes its directory to subprocesses).
7. **Checkout lock** — `git::checkout_lock(repo_dir)` (a per-checkout tokio mutex, not reentrant) serializes everything that moves or reads a checkout's `HEAD`: `sync::sync_repo` (takes it; `sync_repo_locked` when the caller holds it), index builds (`refresh::index_repo_locked`, held with the sync before it), `git worktree add` in `create_worktree`, and whole branch-mode updates. Sync runs at startup, at re-index, from the head watcher and before a repo's first update of a poll cycle, so it can overlap running updates; never move a checkout's `HEAD` without the lock.
8. **Worktree paths** — `perform_update` resolves the repo root with `git rev-parse --show-toplevel` and maps both the entry point and the package file into the worktree relative to it. Files outside the repository (e.g. `meta.position` in the Nix store) are skipped, never edited.
9. **Package attribute expressions** — select packages as `let pkgs = import <entry> { }; in pkgs.<attr>`, never `with import <entry> { }; <attr>`. Attr paths can be quoted (`"389-ds-base"`), which is not an expression under `with`, and bare names under `with` lose to builtins such as `map` or `toString`.

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
