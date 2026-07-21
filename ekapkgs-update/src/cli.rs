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
        /// Maximum number of concurrent package updates (default: CPU cores / 4)
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

/// Worktrees subcommands
#[derive(Subcommand)]
pub enum WorktreesCommand {
    /// List all preserved failed worktrees
    List {
        /// Path to SQLite database
        #[arg(short, long, default_value = DEFAULT_DATABASE_PATH)]
        database: String,
    },
    /// Show details of a specific preserved worktree
    Show {
        /// Package attribute path
        attr_path: String,
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
            } => {
                let mut extra_args = Vec::new();
                if let Some(ref builders_val) = builders {
                    extra_args.push("--builders".to_owned());
                    extra_args.push(builders_val.clone());
                }
                if let Some(max_jobs_val) = max_jobs {
                    extra_args.push("--max-jobs".to_owned());
                    extra_args.push(max_jobs_val.to_string());
                }
                if !extra_args.is_empty() {
                    commands::update::set_extra_nix_build_args(extra_args);
                }
                commands::run::RunConfig::from_args(
                    file,
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
                let mut extra_args = Vec::new();
                if let Some(ref builders_val) = builders {
                    extra_args.push("--builders".to_owned());
                    extra_args.push(builders_val.clone());
                }
                if let Some(max_jobs_val) = max_jobs {
                    extra_args.push("--max-jobs".to_owned());
                    extra_args.push(max_jobs_val.to_string());
                }
                if !extra_args.is_empty() {
                    commands::update::set_extra_nix_build_args(extra_args);
                }
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
                from_phase,
                apply_patch,
                version,
            } => {
                commands::retry::retry(database, attr_path, from_phase, apply_patch, version).await
            },
            Commands::Export {
                database,
                attr_path,
                format,
                output,
            } => {
                let fmt = commands::export::ExportFormat::from_str(&format)?;
                commands::export::export(database, attr_path, fmt, output).await
            },
            Commands::Apply {
                database,
                attr_path,
                patch,
                changes_json,
                validate,
                resume,
            } => {
                commands::apply::apply(database, attr_path, patch, changes_json, validate, resume)
                    .await
            },
            Commands::Migrate { file, target } => commands::migrate::migrate(file, target).await,
            Commands::Worktrees { command } => match command {
                WorktreesCommand::List { database } => {
                    commands::worktrees::list_worktrees(database).await
                },
                WorktreesCommand::Show {
                    attr_path,
                    database,
                } => commands::worktrees::show_worktree(database, attr_path).await,
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
                    // CLI flags override config file for nix build args
                    let resolved_builders = builders.or_else(|| config_file.nix.builders.clone());
                    let resolved_max_jobs = max_jobs.or(config_file.nix.max_jobs);

                    let mut extra_args = Vec::new();
                    if let Some(ref builders_val) = resolved_builders {
                        extra_args.push("--builders".to_owned());
                        extra_args.push(builders_val.clone());
                    }
                    if let Some(max_jobs_val) = resolved_max_jobs {
                        extra_args.push("--max-jobs".to_owned());
                        extra_args.push(max_jobs_val.to_string());
                    }
                    if !extra_args.is_empty() {
                        commands::update::set_extra_nix_build_args(extra_args);
                    }

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
                AutofixCommand::Status { database } => {
                    commands::autofix::status(database).await
                },
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
                } => {
                    commands::autofix::enqueue(database, package, session, max_attempts).await
                },
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
                        database, format, quality, error_type, since_days, min_samples, output,
                    )
                    .await
                },
            },
        }
    }
}
