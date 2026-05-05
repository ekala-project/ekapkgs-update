//! Command-line interface definitions
//!
//! This module contains all clap-related structs and command execution logic.

use clap::{Parser, Subcommand};

use crate::commands;
use crate::paths::DEFAULT_DATABASE_PATH;
use crate::vcs_sources::SemverStrategy;

#[derive(Parser)]
#[command(name = "ekapkgs-update")]
#[command(about = "Update ekapkgs packages", long_about = None)]
pub struct Args {
    #[command(subcommand)]
    pub command: Commands,
}

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
        /// System to use for evaluation (e.g., 'x86_64-linux', 'aarch64-darwin')
        #[arg(long)]
        system: Option<String>,
        /// Skip directory diff comparison in PR body
        #[arg(long)]
        skip_directory_diff: bool,
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
    /// Migrate a package from nixpkgs to ekapkgs paradigms
    Migrate {
        /// Nix file to evaluate (for attr paths)
        #[arg(short, long, default_value = "default.nix")]
        file: String,
        /// Attribute path or file path to migrate
        target: String,
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
    pub async fn execute(self) -> anyhow::Result<()> {
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
            } => {
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
                )
                .execute()
                .await
            },
            Commands::Update {
                file,
                attr_path,
                semver,
                ignore_update_script,
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
                system,
                skip_directory_diff,
            } => {
                commands::update::UpdateParams::from_args(
                    file,
                    attr_path,
                    semver,
                    ignore_update_script,
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
                    system,
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
            Commands::Migrate { file, target } => commands::migrate::migrate(file, target).await,
        }
    }
}
