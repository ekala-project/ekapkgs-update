# watch - Passive Updates

`ekapkgs-update watch` is an event-driven alternative to `run`. Instead of
evaluating every package on each cycle, it indexes which upstream source each
package tracks and then polls those sources (RSS/Atom feeds for GitHub and
PyPI, APIs for GitLab, SourceHut and directory listings). It only starts an
update when a **new** release appears.

```bash
# Single repository (the current directory)
ekapkgs-update watch --file default.nix

# Several repositories, configured in config.toml (see below)
ekapkgs-update watch
```

## Lifecycle

```
startup ── recover interrupted state ── register repos
   │
   ├─ for each repo: sync (fast-forward) ── index (nix-eval-jobs)
   │
   ├─ poll immediately (bootstrap: record baselines, dispatch nothing)
   │
   ├─ every --poll-interval: poll ── dispatch new releases ── update session
   │
   └─ every --index-refresh-hours: sync + re-index every repo
```

### 1. Sync (multi-repo mode)

Before every index build, each managed checkout is fetched from the remote
whose URL points at the configured upstream (falling back to `origin`).
The checkout is then fast-forwarded to `<remote>/<base-branch>`.
Update worktrees are created from the checkout's `HEAD`, so this keeps
proposed updates based on current upstream state.

The sync never rewrites history:

| Situation | Result | `sync_status` |
|-----------|--------|---------------|
| Already at upstream | nothing to do | `ok` |
| Behind upstream | `git merge --ff-only` | `ok` |
| Local commits on top of upstream | kept as-is | `ok` |
| Local and upstream both moved | **warning**, stale tree kept | `diverged` |
| A branch other than the base branch is checked out | **warning**, untouched | `diverged` |
| Fetch failed | **warning**, stale tree kept | `error` |

A diverged checkout keeps being indexed and polled from its stale tree until
someone reconciles it by hand. In legacy single-repo mode (no `[[repos]]`), the
current directory is never fetched or merged (`unmanaged`).

### 2. Index

`nix-eval-jobs` evaluates the repository. For each package, the indexer
records the following in `upstream_index`:

- the upstream source
- the current version
- the effective semver strategy (see [Version Selection](../advanced/version-selection.md))
- `version-regex` and `include-prereleases`

Packages are skipped when they set `passthru.ekapkgs-update.skip`, have no
resolvable upstream, or are pinned by an attr-path version suffix such as
`foo_1_2_3`.

Every indexed package is also registered in `updates` with its pname,
description and homepage, so the dashboard lists it before the first poll.

Packages that disappear from the repository are pruned from the index. The
pruning is skipped when the evaluation looks incomplete. That covers stream
errors, zero derivations, or fewer than half the previous number of packages.

### 3. Bootstrap (observe-only)

A watched repository usually has a backlog: many packages are already behind
upstream when watch starts. Watch deliberately does **not** try to work through
that backlog:

- The **first** time a package is observed, its best compatible upstream
  release is stored as `baseline_version`, and nothing is dispatched.
- Afterwards, only releases **newer than the baseline** trigger an update.
- Releases between the current version and the baseline (the backlog) are
  left to `ekapkgs-update run`.

The first poll happens immediately after the startup index, so baselines are
in place within minutes. The repository's `bootstrapped_at` records when that
happened. Packages added to the repository later get their own baseline on
their first poll.

### 4. Dispatch

On every poll, each package is compared against the best release allowed by
its own strategy, regex and prerelease settings. The checks run in this order:

| Decision | When |
|----------|------|
| baseline | first observation, so the baseline is recorded |
| up to date | no newer compatible release |
| backlog | newer than current, but not newer than the baseline |
| already proposed | this exact version was already built and proposed |
| backoff | this exact version failed recently and its backoff hasn't expired |
| duplicate | an alias with the same source, current and target version is handled instead |
| **dispatch** | otherwise |

Aliases (for example `python312Packages.foo` and `python313Packages.foo`) are
collapsed so that only the shortest attr path is updated. If any alias was
already proposed, or is backing off for that version, none of them are
dispatched.

A failed attempt only blocks **that** version: a newer release is dispatched
right away, even while the backoff for the failed version is still running.

### 5. Sessions and cycles

- A poll cycle that dispatches at least one update creates an update session
  (`mode = 'watch'`). The session has the same per-phase tracking as `run`.
- Cycles that dispatch nothing create no session.
- Every poll and every index build is recorded in `watch_cycles`, which drives
  the dashboard's Watch page.
- Uneventful poll cycles are pruned after 7 days.

If the watch process is killed, the next start closes out what it left behind:

- running cycles are marked `failed`
- repos stuck in `indexing` are marked `error`
- running watch sessions are marked `cancelled`

## Configuration

```toml
[watch]
poll_interval_minutes = 10   # default: 10 (minimum 1)
index_refresh_hours = 24     # default: 24

[[repos]]
name = "core"
upstream = "ekala-project/ekapkgs"   # owner/repo or a github.com URL
entrypoint = "default.nix"           # relative to the checkout
fork = "fork"                        # remote to push update branches to
path = "/var/lib/ekapkgs-update/repos/core"
```

Repo names must be unique and must not be `default`, which is reserved for the
unnamed scope used by `run` and single-repo watch.

Missing checkouts are cloned on startup. If `fork` is not `origin` and that
remote does not exist yet, a GitHub fork is created (this needs
`GITHUB_TOKEN`). Repositories removed from the config are forgotten: their
`watched_repos` row and index entries are deleted, and their `updates`
history is kept.

## Flags

| Flag | Default | Description |
|------|---------|-------------|
| `-f, --file` | `default.nix` | Entry point (legacy single-repo mode) |
| `-d, --database` | XDG data dir | SQLite database |
| `--poll-interval` | `10` | Minutes between polls |
| `--index-refresh-hours` | `24` | Hours between sync + re-index |
| `--upstream` | inferred | Upstream remote (legacy mode) |
| `--fork` | `origin` | Remote to push branches to (legacy mode) |
| `--run-passthru-tests` | off | Build `passthru.tests` |
| `--dry-run` | off | Report updates without performing them |
| `--concurrent-updates` | `cpus / 4` | Concurrent updates per cycle |
| `--commit-strategy` | `worktrees` | `worktrees` (PRs) or `branch` |
| `--preserve-failures` | off | Keep failed worktrees for `inspect` / `export` |
| `--claude-fix` | off | Ask Claude Code to fix build/test failures |

## Related

- [Web Dashboard](../web-dashboard.md): the Watch page shows repos, cycles and source health
- [Database Schema](../advanced/database.md): `watched_repos`, `watch_cycles`, `upstream_sources`
- [Backoff Strategy](../advanced/backoff.md)
