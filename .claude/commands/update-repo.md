---
description: "Run ekapkgs-update with --claude-fix on a Nix repository using branch commit strategy"
argument-hint: "<repo-directory> <entrypoint-file> [extra flags...]"
allowed-tools: ["Bash(cargo build:*)", "Bash(cd:*)", "Bash(git:*)", "Bash(ekapkgs-update:*)", "Bash(ls:*)", "Bash(which:*)"]
---

# ekapkgs-update: Branch-Mode Claude-Fix Run

You are running `ekapkgs-update run` against a Nix package repository using the **branch** commit strategy with **--claude-fix** enabled.

## Arguments

The user provided: `$ARGUMENTS`

Parse these arguments as:
1. **First argument**: Path to the git repository directory (required)
2. **Second argument**: Nix entrypoint file relative to the repo directory (required, e.g. `default.nix`, `pkgs/top-level/all-packages.nix`)
3. **Remaining arguments**: Extra flags passed directly to `ekapkgs-update run`

If fewer than 2 arguments were provided, stop and ask the user to provide both a repo directory and entrypoint file.

## Context

- ekapkgs-update binary location: !`which ekapkgs-update 2>/dev/null || echo "$(cd /home/jon/projects/ekapkgs-update && cargo build --release 2>&1 | tail -1 && echo ./target/release/ekapkgs-update)"`
- Current working directory: !`pwd`

## How it works

**Branch commit strategy** means:
- Updates are committed directly to the current branch of the target repo (no worktrees, no PRs)
- Git operations are serialized to prevent races
- On failure, changes are reverted with `git checkout`
- Default concurrency is 1 (safe for branch mode)

**--claude-fix** means:
- When a package update fails to build, the Claude Code CLI is invoked to attempt an automated fix
- Claude reads the build error, the Nix file, and the diff, then tries to edit the Nix files to fix the issue
- If Claude's fix passes a validation build, the update succeeds; otherwise it's reverted

## Your task

1. Validate the repo directory exists and is a git repository
2. Validate the entrypoint file exists inside the repo directory
3. `cd` into the repo directory
4. Run `ekapkgs-update run` with these flags:
   - `--file <entrypoint-file>` — the Nix entrypoint
   - `--commit-strategy branch` — direct branch commits, no worktrees/PRs
   - `--claude-fix` — enable Claude Code automated build fixes
   - `--skip-cve` — skip CVE checking (not needed for branch-mode local runs)
   - `--skip-repology` — skip Repology checking
   - `--skip-cachix` — skip Cachix pushing
   - Plus any extra flags the user provided

Example invocation:
```
ekapkgs-update run \
  --file default.nix \
  --commit-strategy branch \
  --claude-fix \
  --skip-cve \
  --skip-repology \
  --skip-cachix
```

If `ekapkgs-update` is not on PATH, build it first with `cargo build --release` from `/home/jon/projects/ekapkgs-update` and use the full path to the binary.

Report the command you're about to run before executing it. The command may run for a long time — that's expected.
