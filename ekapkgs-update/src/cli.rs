//! Command-line interface definitions
//!
//! This module contains all clap-related structs and command execution logic.

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{ColorChoice, Parser, Subcommand, ValueEnum};

use crate::commands;
use crate::config::ConfigFile;
use crate::paths::DEFAULT_DATABASE_PATH;
use crate::vcs_sources::SemverStrategy;

pub const CLI_STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .invalid(AnsiColor::Yellow.on_default().effects(Effects::BOLD));

/// When to emit ANSI color in help/usage output.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Default)]
#[value(rename_all = "lower")]
pub enum ColorWhen {
    /// Color when stdout/stderr is a terminal.
    #[default]
    Auto,
    /// Always emit color, even when not a terminal.
    Always,
    /// Never emit color.
    Never,
}

/// Strategy for committing updates in run mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Default, serde::Serialize)]
#[value(rename_all = "lower")]
pub enum CommitStrategy {
    /// Use isolated git worktrees for each update (supports concurrency and PRs).
    #[default]
    Worktrees,
    /// Commit each successful update directly to the current branch
    /// (single-threaded, no PRs).
    Branch,
}

impl From<ColorWhen> for ColorChoice {
    fn from(value: ColorWhen) -> Self {
        match value {
            ColorWhen::Auto => Self::Auto,
            ColorWhen::Always => Self::Always,
            ColorWhen::Never => Self::Never,
        }
    }
}

#[derive(Parser)]
#[command(name = "ekapkgs-update")]
#[command(about = "Update ekapkgs packages", long_about = None)]
#[command(styles = CLI_STYLES)]
pub struct Args {
    /// Coloring
    #[arg(
        long = "color",
        value_name = "WHEN",
        global = true,
        default_value = "auto",
        value_enum
    )]
    pub color: ColorWhen,

    /// Path to TOML config file. Defaults to ~/.config/ekapkgs-update/config.toml
    /// or the EKAPKGS_CONFIG_FILE env var.
    #[arg(long, global = true, value_name = "PATH")]
    pub config_file: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

/// Command dispatch and execution logic.
///
/// ## Command Flow
///
/// Each subcommand follows this pattern:
///
/// | Subcommand         | Config Type      | Execution Path                       |
/// |--------------------|------------------|--------------------------------------|
/// | `Run`              | `RunConfig`      | `commands::run::execute`             |
/// | `Update`           | `UpdateConfig` + `VersionConfig` | `commands::update::execute` (or `update_flake::update_flake_package` if `--flake`) |
/// | `PruneMaintainers` | (direct args)    | `commands::prune_maintainers::execute` |
/// | `Log`              | (direct args)    | `commands::log::execute`             |
/// | `Migrate`          | (direct args)    | `commands::migrate::execute`         |
/// | `Review`           | `ReviewConfig`   | `commands::review::execute`          |
///
/// The `execute()` method acts as a thin dispatcher: it extracts flags from the
/// clap-parsed enum, builds the appropriate config struct (when applicable), and
/// calls the downstream execute function. This keeps `cli.rs` focused on argument
/// parsing while delegating business logic to the `commands` module.
#[derive(Subcommand)]
pub enum Commands {
    /// Run the update process
    Run {
        /// Nix file to evaluate
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// nix-eval-jobs `--select` function applied to the file before
        /// traversal, e.g. 'x: x.buildable' for an entry point that returns
        /// `{ buildable = ...; }`. Requires --eval-file.
        #[arg(long, value_name = "EXPR")]
        select: Option<String>,
        /// Nix file that attribute paths are evaluated, located and built
        /// against (e.g. the repository's default.nix). Defaults to --file.
        #[arg(long, value_name = "FILE")]
        eval_file: Option<String>,
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Upstream git remote. Inferred if left unset. E.g. nixpkgs
        #[arg(long)]
        upstream: Option<String>,
        /// Remote repository to push branches. E.g. my-fork
        #[arg(long, default_value = "origin")]
        fork: String,
        /// Run passthru.tests if available before considering update successful
        #[arg(long)]
        run_passthru_tests: bool,
        /// Check for updates without rewriting, building, committing, or creating PRs
        #[arg(long)]
        dry_run: bool,
        /// Maximum number of concurrent package updates (default: CPU cores / 4).
        /// Branch commit strategy always runs one update at a time.
        #[arg(long)]
        concurrent_updates: Option<usize>,
        /// Skip packages with 'unstable' in their version
        #[arg(long)]
        skip_unstable: bool,
        /// Analyze and report rebuild counts for each update
        #[arg(long)]
        analyze_rebuilds: bool,
        /// Skip updates that would cause more than N rebuilds
        #[arg(long)]
        max_rebuilds: Option<usize>,
        /// Skip CVE vulnerability checking
        #[arg(long)]
        skip_cve: bool,
        /// Skip Repology cross-distribution version checking
        #[arg(long)]
        skip_repology: bool,
        /// Skip directory diff comparison in PR body
        #[arg(long)]
        skip_directory_diff: bool,
        /// Skip pushing build outputs to Cachix
        #[arg(long)]
        skip_cachix: bool,
        /// Cachix cache name to push successful builds to. Falls back to the
        /// `CACHIX_CACHE_NAME` environment variable. Requires
        /// `CACHIX_AUTH_TOKEN` to be set.
        #[arg(long, value_name = "NAME")]
        cachix_cache: Option<String>,
        /// Interactive mode: prompt before submitting PRs with summary and commit info. Forces
        /// single-threaded execution.
        #[arg(long)]
        interactive: bool,
        /// Preserve failed worktrees and artifacts for later inspection
        #[arg(long)]
        preserve_failures: bool,
        /// Nix builders to use for builds (passed as --builders to nix-build).
        /// E.g., 'external' to offload builds to remote builders.
        #[arg(long)]
        builders: Option<String>,
        /// Maximum number of local nix build jobs (passed as --max-jobs to nix-build).
        /// Set to 0 to force all builds to run on remote builders.
        #[arg(long)]
        max_jobs: Option<usize>,
        /// Strategy for committing updates: 'worktrees' (default) uses isolated worktrees
        /// and optionally creates PRs; 'branch' commits each update directly to the current
        /// branch with concurrent builds.
        #[arg(long, default_value = "worktrees")]
        commit_strategy: CommitStrategy,
        /// Run package audits after each successful build and include findings in PR body
        #[arg(long)]
        audit: bool,
        /// Disable security checks during audit (setuid/setgid, credentials, suspicious URLs,
        /// etc.)
        #[arg(long)]
        no_security: bool,
        /// On build/test failure in worktree mode, invoke Claude Code CLI to attempt
        /// an automated fix before giving up. Requires `claude` on PATH.
        #[arg(long)]
        claude_fix: bool,
        /// Maximum number of Claude Code agent turns per fix attempt
        #[arg(long, default_value = "10")]
        claude_fix_max_turns: u32,
        /// Timeout in seconds for each Claude Code fix attempt
        #[arg(long, default_value = "300")]
        claude_fix_timeout: u64,
    },
    /// Update a package in a Nix file
    Update {
        /// Nix file to update
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// Attribute path of the package to update
        attr_path: String,
        /// Version selection strategy: latest, major, minor, or patch
        #[arg(long, default_value_t = SemverStrategy::Latest)]
        semver: SemverStrategy,
        /// Ignore update script and use generic update method
        #[arg(long, default_value = "false")]
        ignore_update_script: bool,
        /// Force update even if package has passthru.ekapkgs-update.skip = true
        #[arg(long)]
        force: bool,
        /// Create a git commit after successful update
        #[arg(long)]
        commit: bool,
        /// Create a pull request after successful update (implies --commit)
        #[arg(long)]
        create_pr: bool,
        /// Upstream git remote. Inferred if left unset. E.g. nixpkgs.
        /// Only used with --create-pr.
        #[arg(long)]
        upstream: Option<String>,
        /// Remote repository to push branches. E.g. my-fork
        /// Only used with --create-pr.
        #[arg(long, default_value = "origin")]
        fork: String,
        /// Run passthru.tests if available before considering update successful
        #[arg(long)]
        run_passthru_tests: bool,
        /// For mkManyVariants packages: update only this specific variant (e.g., v1_2, v0_20)
        #[arg(long)]
        variant: Option<String>,
        /// For mkManyVariants packages: explicitly update all variants (this is the default)
        #[arg(long)]
        all_variants: bool,
        /// Enable flake mode: update a package exposed by a flake
        #[arg(long)]
        flake: bool,
        /// Flake output prefix (e.g., 'packages.x86_64-linux'). Auto-detected if not specified.
        #[arg(long)]
        flake_output: Option<String>,
        /// Only update source hash, skip dependency hashes (npmDeps, nugetDeps, composerDeps,
        /// etc.)
        #[arg(long)]
        src_only: bool,
        /// Explicit version to update to (overrides --semver). Can be a specific version like
        /// "2.5.1" or a tag name.
        #[arg(long)]
        version: Option<String>,
        /// Custom regex to extract version from tags (e.g., 'jq-(.*)' to extract version from
        /// 'jq-1.6' tags)
        #[arg(long)]
        version_regex: Option<String>,
        /// Format updated files using nixfmt
        #[arg(long)]
        format: bool,
        /// Override the filename to update (useful when meta.position points to the wrong file)
        #[arg(long)]
        override_filename: Option<String>,
        /// Skip directory diff comparison in PR body
        #[arg(long)]
        skip_directory_diff: bool,
        /// Nix builders to use for builds (passed as --builders to nix-build).
        /// E.g., 'external' to offload builds to remote builders.
        #[arg(long)]
        builders: Option<String>,
        /// Maximum number of local nix build jobs (passed as --max-jobs to nix-build).
        /// Set to 0 to force all builds to run on remote builders.
        #[arg(long)]
        max_jobs: Option<usize>,
    },
    /// Prune maintainers from all .nix files in a directory
    PruneMaintainers {
        /// Directory to process
        directory: String,
        /// Check mode: fail if any changes would be made
        #[arg(long, default_value = "false")]
        check: bool,
    },
    /// Show update failure logs for a package
    Log {
        /// Drv path (e.g., /nix/store/...drv or hash-name.drv) or attr path (e.g.,
        /// python.pkgs.setuptools)
        identifier: String,
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
    },
    /// Inspect detailed failure information for a package
    Inspect {
        /// Package attribute path to inspect
        identifier: String,
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
    },
    /// Query update failures with filtering
    Query {
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Filter by error type
        #[arg(long)]
        error_type: Option<String>,
        /// Filter by phase
        #[arg(long)]
        phase: Option<String>,
        /// Filter by status (success, failed, running, skipped)
        #[arg(long)]
        status: Option<String>,
        /// Filter by package name pattern (SQL LIKE pattern)
        #[arg(long)]
        package: Option<String>,
        /// Filter to entries from the last N days
        #[arg(long)]
        since_days: Option<u32>,
        /// Limit number of results
        #[arg(long)]
        limit: Option<usize>,
        /// Group results by error type
        #[arg(long)]
        group_by_error: bool,
    },
    /// Generate a categorized markdown report of failed updates
    Report {
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Filter by package name pattern (SQL LIKE pattern)
        #[arg(long)]
        package: Option<String>,
        /// Filter to failures from the last N days
        #[arg(long)]
        since_days: Option<u32>,
        /// Output file (writes to stdout if not specified)
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },
    /// Show status of current/recent update runs
    Status {
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
    },
    /// Retry a failed update from preserved worktree
    Retry {
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Package attribute path to retry
        attr_path: String,
        /// Repository of the failure (a `[[repos]]` name, or `default` for
        /// `run` / single-repo watch). Required when the attr failed in
        /// several repos.
        #[arg(long)]
        repo: Option<String>,
        /// Resume from specific phase (if supported)
        #[arg(long)]
        from_phase: Option<String>,
        /// Apply patch file before retrying
        #[arg(long)]
        apply_patch: Option<std::path::PathBuf>,
        /// Override version to update to
        #[arg(long)]
        version: Option<String>,
    },
    /// Export failure context for LLM analysis
    Export {
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Package attribute path to export
        attr_path: String,
        /// Repository of the failure (a `[[repos]]` name, or `default` for
        /// `run` / single-repo watch). Required when the attr failed in
        /// several repos.
        #[arg(long)]
        repo: Option<String>,
        /// Export format: json or markdown
        #[arg(long, default_value = "json")]
        format: String,
        /// Output file (writes to stdout if not specified)
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },
    /// Apply LLM-generated fix to preserved worktree
    Apply {
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Package attribute path to apply fix to
        attr_path: String,
        /// Repository of the failure (a `[[repos]]` name, or `default` for
        /// `run` / single-repo watch). Required when the attr failed in
        /// several repos.
        #[arg(long)]
        repo: Option<String>,
        /// Patch file to apply
        #[arg(long)]
        patch: Option<std::path::PathBuf>,
        /// JSON file with structured changes
        #[arg(long)]
        changes_json: Option<std::path::PathBuf>,
        /// Validate changes by building
        #[arg(long)]
        validate: bool,
        /// Resume update workflow after applying fix
        #[arg(long)]
        resume: bool,
    },
    /// Migrate a package from nixpkgs to ekapkgs paradigms
    Migrate {
        /// Nix file to evaluate (for attr paths)
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// Attribute path or file path to migrate
        target: String,
    },
    /// Manage preserved failure worktrees
    Worktrees {
        #[command(subcommand)]
        command: WorktreesCommand,
    },
    /// Automatically fix failed updates using LLM assistance
    Autofix {
        #[command(subcommand)]
        command: AutofixCommand,
    },
    /// Audit a built Nix package for correctness issues
    Audit {
        /// Target to audit: .drv path, store path, or attr path
        target: String,
        /// Nix file to evaluate (used when target is an attr path)
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// Output format: human, json, or markdown
        #[arg(long, default_value = "human")]
        format: String,
        /// Minimum severity to report: info, warning, or error
        #[arg(long, default_value = "info")]
        min_severity: String,
        /// Skip metadata checks (meta.description, meta.license, etc.)
        #[arg(long)]
        skip_metadata: bool,
        /// Skip binary/ELF checks (RPATH, shared libs)
        #[arg(long)]
        skip_binary_checks: bool,
        /// Disable security checks (setuid/setgid, credentials, suspicious URLs, etc.)
        #[arg(long)]
        no_security: bool,
        /// Nix builders to use for builds (passed as --builders to nix-build)
        #[arg(long)]
        builders: Option<String>,
        /// Maximum number of local nix build jobs (passed as --max-jobs to nix-build)
        #[arg(long)]
        max_jobs: Option<usize>,
    },
    /// Review a pull request: checkout, merge base, build changed packages,
    /// optionally run tests and a best-practices agent, then merge if all pass.
    Review {
        /// PR number to review
        pr_number: u64,
        /// Repository in 'owner/repo' or 'github.com/owner/repo' format.
        /// Assumes github.com when given 'owner/repo'.
        #[arg(long)]
        repo: String,
        /// Nix file to evaluate
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// Also build passthru.tests for each package (if they exist)
        #[arg(long)]
        tests: bool,
        /// Run a best-practices agent review on the changed Nix files
        #[arg(long)]
        best_practices: bool,
        /// Agent CLI command to use for best-practices review (must accept --print
        /// and --dangerously-skip-permissions flags, like Claude Code)
        #[arg(long, default_value = "claude")]
        best_practices_agent: String,
        /// Maximum agent turns for best-practices review
        #[arg(long, default_value = "10")]
        best_practices_max_turns: u32,
        /// Timeout in seconds for the best-practices agent
        #[arg(long, default_value = "300")]
        best_practices_timeout: u64,
        /// Merge method: merge, squash, or rebase
        #[arg(long, default_value = "merge")]
        merge_method: String,
        /// Skip merging the PR even if all checks pass
        #[arg(long)]
        no_merge: bool,
        /// Drop into an interactive shell with built packages available on PATH
        #[arg(short, long)]
        interactive: bool,
        /// Nix builders to use for builds (passed as --builders to nix-build)
        #[arg(long)]
        builders: Option<String>,
        /// Maximum number of local nix build jobs (passed as --max-jobs to nix-build)
        #[arg(long)]
        max_jobs: Option<usize>,
    },
    /// Lint a Nix package expression for whitespace formatting compliance.
    ///
    /// Checks that header attributes (pname, version, extensions, pyproject,
    /// disabled) are grouped at the top without blank lines, that all other
    /// attributes are separated by blank lines, and that meta is last.
    Lint {
        /// Nix file or directory to lint (directories are searched recursively for .nix files)
        path: String,
        /// Fix formatting issues in-place (default: check only)
        #[arg(long)]
        fix: bool,
        /// Descend into hidden directories (names starting with '.')
        #[arg(long)]
        no_ignore: bool,
    },
    /// Watch upstream sources for new releases and update packages reactively.
    ///
    /// Builds a reverse index mapping upstream sources to attr_paths, then
    /// polls RSS/Atom feeds and APIs at a configurable interval.  When a new
    /// release is detected, it runs the same update pipeline as the `run`
    /// command but only for affected packages.
    Watch {
        /// Nix file to evaluate
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// nix-eval-jobs `--select` function applied to the file before
        /// traversal, e.g. 'x: x.buildable' for an entry point that returns
        /// `{ buildable = ...; }`. Requires --eval-file.
        #[arg(long, value_name = "EXPR")]
        select: Option<String>,
        /// Nix file that attribute paths are evaluated, located and built
        /// against (e.g. the repository's default.nix). Defaults to --file.
        #[arg(long, value_name = "FILE")]
        eval_file: Option<String>,
        /// Path to SQLite database for tracking updates
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Poll interval in minutes (how often to check feeds)
        #[arg(long, default_value = "10")]
        poll_interval: u64,
        /// Index refresh interval in hours (how often to rebuild the full index)
        #[arg(long, default_value = "24")]
        index_refresh_hours: u64,
        /// Upstream git remote. Inferred if left unset.
        #[arg(long)]
        upstream: Option<String>,
        /// Remote repository to push branches
        #[arg(long, default_value = "origin")]
        fork: String,
        /// Run passthru.tests if available
        #[arg(long)]
        run_passthru_tests: bool,
        /// Check for updates without performing them
        #[arg(long)]
        dry_run: bool,
        /// Maximum number of concurrent package updates.
        /// Branch commit strategy always runs one update at a time.
        #[arg(long)]
        concurrent_updates: Option<usize>,
        /// Strategy for committing updates
        #[arg(long, default_value = "worktrees")]
        commit_strategy: CommitStrategy,
        /// Preserve failed worktrees and artifacts for later inspection
        #[arg(long)]
        preserve_failures: bool,
        /// Nix builders (passed as --builders to nix-build)
        #[arg(long)]
        builders: Option<String>,
        /// Max local nix build jobs (passed as --max-jobs to nix-build)
        #[arg(long)]
        max_jobs: Option<usize>,
        /// On build/test failure, invoke Claude Code CLI to attempt a fix
        #[arg(long)]
        claude_fix: bool,
        /// Maximum Claude Code agent turns per fix attempt
        #[arg(long, default_value = "10")]
        claude_fix_max_turns: u32,
        /// Timeout in seconds for each Claude Code fix attempt
        #[arg(long, default_value = "300")]
        claude_fix_timeout: u64,
    },
}

/// Autofix subcommands
#[derive(Subcommand)]
pub enum AutofixCommand {
    /// Process the autofix queue (run LLM fixes serially)
    Run {
        /// Path to SQLite database
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Nix file to evaluate
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// Maximum fix attempts per package before escalation
        #[arg(long, default_value = "3")]
        max_attempts: i64,
        /// Maximum items to process this run
        #[arg(long)]
        limit: Option<usize>,
        /// Show prompts without calling the LLM
        #[arg(long)]
        dry_run: bool,
        /// Only attempt fixes for these error types (comma-separated)
        #[arg(long)]
        error_types: Option<String>,
        /// Nix builders (passed as --builders to nix-build)
        #[arg(long)]
        builders: Option<String>,
        /// Max local nix build jobs (passed as --max-jobs to nix-build)
        #[arg(long)]
        max_jobs: Option<usize>,
    },
    /// Show autofix queue status
    Status {
        /// Path to SQLite database
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
    },
    /// Show autofix attempt history
    History {
        /// Path to SQLite database
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Filter by package name pattern
        #[arg(long)]
        package: Option<String>,
        /// Limit number of results
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Manually enqueue preserved failures for autofix
    Enqueue {
        /// Path to SQLite database
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Specific package to enqueue
        #[arg(long)]
        package: Option<String>,
        /// Specific session to enqueue from
        #[arg(long)]
        session: Option<String>,
        /// Maximum fix attempts per package
        #[arg(long, default_value = "3")]
        max_attempts: i64,
    },
    /// Export training dataset as JSONL for LLM fine-tuning
    ExportDataset {
        /// Path to SQLite database
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Dataset format: 'sft' (supervised fine-tuning) or 'dpo' (preference optimization)
        #[arg(long, default_value = "sft")]
        format: String,
        /// Quality filter: 'verified_success', 'build_failed', 'parse_error', or 'all'
        #[arg(long, default_value = "verified_success")]
        quality: String,
        /// Filter by error type
        #[arg(long)]
        error_type: Option<String>,
        /// Only include samples from the last N days
        #[arg(long)]
        since_days: Option<u32>,
        /// Minimum number of samples required to export
        #[arg(long)]
        min_samples: Option<usize>,
        /// Output file (writes to stdout if not specified)
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },
}

/// Configure extra `nix-build` arguments (`--builders`, `--max-jobs`).
///
/// CLI flags take precedence over the config file's `[nix]` section.
fn apply_nix_build_args(builders: Option<String>, max_jobs: Option<usize>, config: &ConfigFile) {
    let builders = builders.or_else(|| config.nix.builders.clone());
    let max_jobs = max_jobs.or(config.nix.max_jobs);

    let mut extra_args = Vec::new();
    if let Some(builders) = builders {
        extra_args.push("--builders".to_owned());
        extra_args.push(builders);
    }
    if let Some(max_jobs) = max_jobs {
        extra_args.push("--max-jobs".to_owned());
        extra_args.push(max_jobs.to_string());
    }
    if !extra_args.is_empty() {
        commands::update::set_extra_nix_build_args(extra_args);
    }
}

/// Use config-file values as defaults for CLI arguments.
///
/// Rewrites the defaults of every (sub)command argument so the precedence is
/// CLI flag > config file > built-in default:
///
/// - `database`: config `database` replaces the built-in
///   `~/.cache/ekapkgs-update/updates.db`;
/// - `file`: config `file` replaces the built-in `default.nix`;
/// - `select` / `eval_file`: config `select` / `eval_file`.
///
/// Without this, settings such as `database` in a NixOS-generated config file
/// were silently ignored.
pub fn apply_config_defaults(cmd: clap::Command, config: &ConfigFile) -> clap::Command {
    let defaults: [(&str, Option<&String>); 4] = [
        ("database", config.database.as_ref()),
        ("file", config.file.as_ref()),
        ("select", config.select.as_ref()),
        ("eval_file", config.eval_file.as_ref()),
    ];
    apply_defaults_recursive(cmd, &defaults)
}

fn apply_defaults_recursive(
    mut cmd: clap::Command,
    defaults: &[(&str, Option<&String>)],
) -> clap::Command {
    for (id, value) in defaults {
        let Some(value) = value else {
            continue;
        };
        if cmd.get_arguments().any(|a| a.get_id() == *id) {
            let value = (*value).clone();
            cmd = cmd.mut_arg(*id, move |arg| arg.default_value(value));
        }
    }

    let names: Vec<String> = cmd
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect();
    for name in names {
        cmd = cmd.mut_subcommand(name, |sub| apply_defaults_recursive(sub, defaults));
    }
    cmd
}

/// Worktrees subcommands
#[derive(Subcommand)]
pub enum WorktreesCommand {
    /// List all preserved failed worktrees
    List {
        /// Path to SQLite database
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
        /// Only list failures from this repository (`default` for `run` /
        /// single-repo watch)
        #[arg(long)]
        repo: Option<String>,
    },
    /// Show details of a specific preserved worktree
    Show {
        /// Package attribute path
        attr_path: String,
        /// Repository of the failure (a `[[repos]]` name, or `default` for
        /// `run` / single-repo watch). Required when the attr failed in
        /// several repos.
        #[arg(long)]
        repo: Option<String>,
        /// Path to SQLite database
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
    },
    /// Clean up old preserved worktrees
    Clean {
        /// Remove artifacts older than N days
        #[arg(long, default_value = "7")]
        older_than: u32,
    },
}

impl Commands {
    /// Execute the command.
    ///
    /// This is intentionally a thin dispatcher: each variant forwards to the
    /// appropriate `from_args` constructor (or top-level function) and then
    /// awaits `.execute()`. The argument plumbing for the more complex
    /// `Run` and `Update` arms lives in their respective `*Config::from_args`
    /// constructors.
    pub async fn execute(self, config_file: ConfigFile) -> anyhow::Result<()> {
        match self {
            Commands::Run {
                file,
                select,
                eval_file,
                database,
                upstream,
                fork,
                run_passthru_tests,
                dry_run,
                concurrent_updates,
                skip_unstable,
                analyze_rebuilds,
                max_rebuilds,
                skip_cve,
                skip_repology,
                skip_directory_diff,
                skip_cachix,
                cachix_cache,
                interactive,
                preserve_failures,
                builders,
                max_jobs,
                commit_strategy,
                audit,
                no_security,
                claude_fix,
                claude_fix_max_turns,
                claude_fix_timeout,
            } => {
                apply_nix_build_args(builders, max_jobs, &config_file);
                commands::run::RunConfig::from_args(
                    file,
                    select,
                    eval_file,
                    database,
                    upstream,
                    fork,
                    run_passthru_tests,
                    dry_run,
                    concurrent_updates,
                    skip_unstable,
                    analyze_rebuilds,
                    max_rebuilds,
                    skip_cve,
                    skip_repology,
                    skip_directory_diff,
                    skip_cachix,
                    cachix_cache,
                    interactive,
                    preserve_failures,
                    commit_strategy,
                    audit,
                    no_security,
                    claude_fix,
                    claude_fix_max_turns,
                    claude_fix_timeout,
                )
                .execute()
                .await
            },
            Commands::Update {
                file,
                attr_path,
                semver,
                ignore_update_script,
                force,
                commit,
                create_pr,
                upstream,
                fork,
                run_passthru_tests,
                variant,
                all_variants,
                flake,
                flake_output,
                src_only,
                version,
                version_regex,
                format,
                override_filename,
                skip_directory_diff,
                builders,
                max_jobs,
            } => {
                apply_nix_build_args(builders, max_jobs, &config_file);
                commands::update::UpdateParams::from_args(
                    file,
                    attr_path,
                    semver,
                    ignore_update_script,
                    force,
                    commit,
                    create_pr,
                    upstream,
                    fork,
                    run_passthru_tests,
                    variant,
                    all_variants,
                    flake,
                    flake_output,
                    src_only,
                    version,
                    version_regex,
                    format,
                    override_filename,
                    skip_directory_diff,
                )
                .execute()
                .await
            },
            Commands::PruneMaintainers { directory, check } => {
                commands::prune_maintainers::prune_maintainers(directory, check).await
            },
            Commands::Log {
                identifier,
                database,
            } => commands::log::show_log(database, identifier).await,
            Commands::Inspect {
                identifier,
                database,
            } => commands::inspect::inspect(database, identifier).await,
            Commands::Query {
                database,
                error_type,
                phase,
                status,
                package,
                since_days,
                limit,
                group_by_error,
            } => {
                commands::query::query(
                    database,
                    error_type,
                    phase,
                    status,
                    package,
                    since_days,
                    limit,
                    group_by_error,
                )
                .await
            },
            Commands::Report {
                database,
                package,
                since_days,
                output,
            } => commands::report::report(database, package, since_days, output).await,
            Commands::Status { database } => commands::status::status(database).await,
            Commands::Retry {
                database,
                attr_path,
                repo,
                from_phase,
                apply_patch,
                version,
            } => {
                commands::retry::retry(database, attr_path, repo, from_phase, apply_patch, version)
                    .await
            },
            Commands::Export {
                database,
                attr_path,
                repo,
                format,
                output,
            } => {
                let fmt: commands::export::ExportFormat = format.parse()?;
                commands::export::export(database, attr_path, repo, fmt, output).await
            },
            Commands::Apply {
                database,
                attr_path,
                repo,
                patch,
                changes_json,
                validate,
                resume,
            } => {
                commands::apply::apply(
                    database,
                    attr_path,
                    repo,
                    patch,
                    changes_json,
                    validate,
                    resume,
                )
                .await
            },
            Commands::Migrate { file, target } => commands::migrate::migrate(file, target).await,
            Commands::Worktrees { command } => match command {
                WorktreesCommand::List { database, repo } => {
                    commands::worktrees::list_worktrees(database, repo).await
                },
                WorktreesCommand::Show {
                    attr_path,
                    repo,
                    database,
                } => commands::worktrees::show_worktree(database, attr_path, repo).await,
                WorktreesCommand::Clean { older_than } => {
                    commands::worktrees::clean_worktrees(older_than).await
                },
            },
            Commands::Autofix { command } => match command {
                AutofixCommand::Run {
                    database,
                    file,
                    max_attempts,
                    limit,
                    dry_run,
                    error_types,
                    builders,
                    max_jobs,
                } => {
                    apply_nix_build_args(builders, max_jobs, &config_file);

                    let autofix_config = commands::autofix::config::AutofixConfig {
                        database_path: database,
                        eval_entry_point: file,
                        max_attempts,
                        limit,
                        dry_run,
                        error_types: error_types
                            .map(|s| s.split(',').map(|t| t.trim().to_owned()).collect()),
                        llm: config_file.llm,
                    };
                    commands::autofix::run(autofix_config).await
                },
                AutofixCommand::Status { database } => commands::autofix::status(database).await,
                AutofixCommand::History {
                    database,
                    package,
                    limit,
                } => commands::autofix::history(database, package, limit).await,
                AutofixCommand::Enqueue {
                    database,
                    package,
                    session,
                    max_attempts,
                } => commands::autofix::enqueue(database, package, session, max_attempts).await,
                AutofixCommand::ExportDataset {
                    database,
                    format,
                    quality,
                    error_type,
                    since_days,
                    min_samples,
                    output,
                } => {
                    commands::autofix::export_dataset_cmd(
                        database,
                        format,
                        quality,
                        error_type,
                        since_days,
                        min_samples,
                        output,
                    )
                    .await
                },
            },
            Commands::Audit {
                target,
                file,
                format,
                min_severity,
                skip_metadata,
                skip_binary_checks,
                no_security,
                builders,
                max_jobs,
            } => {
                apply_nix_build_args(builders, max_jobs, &config_file);
                commands::audit::execute(
                    target,
                    file,
                    format,
                    min_severity,
                    skip_metadata,
                    skip_binary_checks,
                    no_security,
                )
                .await
            },
            Commands::Review {
                pr_number,
                repo,
                file,
                tests,
                best_practices,
                best_practices_agent,
                best_practices_max_turns,
                best_practices_timeout,
                merge_method,
                no_merge,
                interactive,
                builders,
                max_jobs,
            } => {
                apply_nix_build_args(builders, max_jobs, &config_file);

                let repo_coords = commands::review::parse_repo_arg(&repo)?;
                commands::review::execute(commands::review::ReviewConfig {
                    repo: repo_coords,
                    pr_number,
                    file,
                    run_tests: tests,
                    best_practices,
                    best_practices_agent,
                    best_practices_max_turns,
                    best_practices_timeout,
                    merge_method,
                    no_merge,
                    interactive,
                })
                .await
            },
            Commands::Lint {
                path,
                fix,
                no_ignore,
            } => commands::lint::lint(path, fix, no_ignore).await,
            Commands::Watch {
                file,
                select,
                eval_file,
                database,
                poll_interval,
                index_refresh_hours,
                upstream,
                fork,
                run_passthru_tests,
                dry_run,
                concurrent_updates,
                commit_strategy,
                preserve_failures,
                builders,
                max_jobs,
                claude_fix,
                claude_fix_max_turns,
                claude_fix_timeout,
            } => {
                apply_nix_build_args(builders, max_jobs, &config_file);
                let mut watch_config = commands::watch::WatchConfig::from_args(
                    file,
                    select,
                    eval_file,
                    database,
                    poll_interval,
                    index_refresh_hours,
                    upstream,
                    fork,
                    run_passthru_tests,
                    dry_run,
                    concurrent_updates,
                    commit_strategy,
                    preserve_failures,
                    claude_fix,
                    claude_fix_max_turns,
                    claude_fix_timeout,
                );
                watch_config.repos = config_file.repos;
                watch_config.execute().await
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, FromArgMatches};

    use super::*;

    fn parse(config: &ConfigFile, argv: &[&str]) -> Args {
        let cmd = apply_config_defaults(Args::command(), config);
        let matches = cmd
            .try_get_matches_from(argv)
            .unwrap_or_else(|e| panic!("{argv:?}: {e}"));
        Args::from_arg_matches(&matches).unwrap()
    }

    fn config() -> ConfigFile {
        ConfigFile {
            database: Some("/var/lib/ekapkgs-update/updates.db".to_owned()),
            file: Some("/repos/corepkgs/ci/packages.nix".to_owned()),
            select: Some("x: x.buildable".to_owned()),
            eval_file: Some("/repos/corepkgs/default.nix".to_owned()),
            ..ConfigFile::default()
        }
    }

    #[test]
    fn config_file_values_become_defaults() {
        let args = parse(&config(), &["ekapkgs-update", "watch"]);
        let Commands::Watch {
            file,
            select,
            eval_file,
            database,
            ..
        } = args.command
        else {
            panic!("expected watch");
        };
        assert_eq!(database, "/var/lib/ekapkgs-update/updates.db");
        assert_eq!(file, "/repos/corepkgs/ci/packages.nix");
        assert_eq!(select.as_deref(), Some("x: x.buildable"));
        assert_eq!(eval_file.as_deref(), Some("/repos/corepkgs/default.nix"));
    }

    #[test]
    fn cli_flags_override_config_file() {
        let args = parse(
            &config(),
            &[
                "ekapkgs-update",
                "run",
                "--database",
                "/tmp/x.db",
                "-f",
                "other.nix",
            ],
        );
        let Commands::Run { file, database, .. } = args.command else {
            panic!("expected run");
        };
        assert_eq!(database, "/tmp/x.db");
        assert_eq!(file, "other.nix");
    }

    #[test]
    fn nested_subcommands_get_config_database() {
        let args = parse(&config(), &["ekapkgs-update", "worktrees", "list"]);
        let Commands::Worktrees {
            command: WorktreesCommand::List { database, .. },
        } = args.command
        else {
            panic!("expected worktrees list");
        };
        assert_eq!(database, "/var/lib/ekapkgs-update/updates.db");
    }

    #[test]
    fn builtin_defaults_without_config() {
        let args = parse(&ConfigFile::default(), &["ekapkgs-update", "watch"]);
        let Commands::Watch {
            file,
            select,
            database,
            ..
        } = args.command
        else {
            panic!("expected watch");
        };
        assert_eq!(database, DEFAULT_DATABASE_PATH);
        assert_eq!(file, "default.nix");
        assert_eq!(select, None);
    }
}
