//! Configuration structures for update operations
//!
//! This module contains configuration structs that group related parameters
//! to reduce function argument counts and improve code organization.

use tracing::{debug, info, warn};

use crate::commands::pr_enhancements::PrEnhancementsConfig;
use crate::nix::{
    eval_nix_expr, get_variants_list, is_many_variants_package, normalize_entry_point,
};
use crate::variant_strategy::{infer_strategy_from_variant, is_variant_pinned};
use crate::vcs_sources::SemverStrategy;

/// Common update configuration shared across all update operations
#[derive(Debug, Clone)]
pub struct UpdateConfig {
    /// Whether to create a git commit after successful update
    pub commit: bool,

    /// Whether to create a pull request after successful update
    pub create_pr: bool,

    /// Upstream git remote (for PR creation)
    pub upstream: Option<String>,

    /// Fork git remote (for PR creation)
    pub fork: String,

    /// Whether to run passthru.tests after update
    pub run_passthru_tests: bool,

    /// Whether to only update source hash (skip dependency hashes)
    pub src_only: bool,

    /// Whether to format the file with nixfmt after update
    pub format: bool,

    /// PR enhancement configuration (CVE, Repology, directory diff, rebuilds)
    pub pr_enhancements: PrEnhancementsConfig,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            commit: false,
            create_pr: false,
            upstream: None,
            fork: "origin".to_owned(),
            run_passthru_tests: false,
            src_only: false,
            format: false,
            pr_enhancements: PrEnhancementsConfig {
                directory_diff: true,
                ..PrEnhancementsConfig::default()
            },
        }
    }
}

impl UpdateConfig {
    /// Update a single variant in a mkManyVariants package
    pub async fn update_single_variant(
        self,
        file: &str,
        attr_path: &str,
        variant_name: &str,
        strategy: SemverStrategy,
    ) -> anyhow::Result<()> {
        super::update_single_variant(file, attr_path, variant_name, strategy, self).await
    }

    /// Update a package from a specific file path.
    /// Returns removed patches and the test result.
    pub async fn update_from_file_path(
        self,
        eval_entry_point: String,
        attr_path: String,
        file_location: String,
        version_config: VersionConfig,
    ) -> anyhow::Result<(Vec<String>, super::TestResult)> {
        super::update_from_file_path(
            eval_entry_point,
            attr_path,
            file_location,
            version_config,
            self,
        )
        .await
    }

    /// Update a package exposed by a flake
    pub async fn update_flake_package(
        self,
        flake_ref: String,
        attr_path: String,
        flake_output: Option<String>,
        version_config: VersionConfig,
        override_filename: Option<String>,
    ) -> anyhow::Result<()> {
        super::update_flake_package(
            flake_ref,
            attr_path,
            flake_output,
            version_config,
            self,
            override_filename,
        )
        .await
    }
}

/// Version selection configuration
#[derive(Debug, Clone)]
pub struct VersionConfig {
    /// Semantic versioning strategy for selecting compatible versions
    pub strategy: SemverStrategy,

    /// Explicit version to update to (overrides strategy)
    pub explicit_version: Option<String>,

    /// Custom regex for extracting version from tags
    pub version_regex: Option<String>,
}

impl VersionConfig {
    pub fn new(strategy: SemverStrategy) -> Self {
        Self {
            strategy,
            explicit_version: None,
            version_regex: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_explicit_version(mut self, version: Option<String>) -> Self {
        self.explicit_version = version;
        self
    }

    #[allow(dead_code)]
    pub fn with_version_regex(mut self, regex: Option<String>) -> Self {
        self.version_regex = regex;
        self
    }
}

/// Configuration for mkManyVariants packages
#[derive(Debug, Clone, Default)]
pub struct VariantConfig {
    /// Specific variant to update (None = default variant only)
    pub variant: Option<String>,

    /// Whether to update all variants
    pub all_variants: bool,
}

/// Configuration for flake package updates
#[derive(Debug, Clone, Default)]
pub struct FlakeConfig {
    /// Whether this is a flake package
    pub enabled: bool,

    /// Flake output prefix (e.g., "packages.x86_64-linux")
    pub output: Option<String>,
}

/// Complete parameters for package update operations
#[derive(Debug, Clone)]
pub struct UpdateParams {
    /// Nix file entry point (e.g., "default.nix", "<nixpkgs>")
    pub file: String,

    /// Package attribute path (e.g., "pkgs.hello")
    pub attr_path: String,

    /// Whether to ignore update scripts (passthru.updateScript)
    pub ignore_update_script: bool,

    /// Whether to force update even if passthru.ekapkgs-update.skip = true
    pub force: bool,

    /// Override filename for package definition (ignores meta.position)
    pub override_filename: Option<String>,

    /// Common update configuration
    pub update_config: UpdateConfig,

    /// Version selection configuration
    pub version_config: VersionConfig,

    /// Variant configuration (for mkManyVariants packages)
    pub variant_config: VariantConfig,

    /// Flake configuration
    pub flake_config: FlakeConfig,
}

impl UpdateParams {
    /// Build [`UpdateParams`] from the flat set of CLI flags accepted by the
    /// `Update` subcommand.
    ///
    /// This keeps `Commands::execute` in `cli.rs` to a tight dispatcher by
    /// pushing the nested-struct construction out of the CLI module.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    pub fn from_args(
        file: String,
        attr_path: String,
        semver: SemverStrategy,
        ignore_update_script: bool,
        force: bool,
        commit: bool,
        create_pr: bool,
        upstream: Option<String>,
        fork: String,
        run_passthru_tests: bool,
        variant: Option<String>,
        all_variants: bool,
        flake: bool,
        flake_output: Option<String>,
        src_only: bool,
        version: Option<String>,
        version_regex: Option<String>,
        format: bool,
        override_filename: Option<String>,
        skip_directory_diff: bool,
    ) -> Self {
        Self {
            file,
            attr_path,
            ignore_update_script,
            force,
            override_filename,
            update_config: UpdateConfig {
                commit,
                create_pr,
                upstream,
                fork,
                run_passthru_tests,
                src_only,
                format,
                pr_enhancements: PrEnhancementsConfig {
                    directory_diff: !skip_directory_diff,
                    ..PrEnhancementsConfig::default()
                },
            },
            version_config: VersionConfig {
                strategy: semver,
                explicit_version: version,
                version_regex,
            },
            variant_config: VariantConfig {
                variant,
                all_variants,
            },
            flake_config: FlakeConfig {
                enabled: flake,
                output: flake_output,
            },
        }
    }

    /// Execute the update operation
    pub async fn execute(self) -> anyhow::Result<()> {
        let UpdateParams {
            file,
            attr_path,
            ignore_update_script,
            force,
            override_filename,
            update_config,
            version_config,
            variant_config,
            flake_config,
        } = self;

        let strategy = version_config.strategy;
        info!("Using semver strategy: {}", strategy);

        // Handle flake mode
        if flake_config.enabled {
            info!("Flake mode enabled");
            return update_config
                .update_flake_package(
                    file,
                    attr_path,
                    flake_config.output,
                    version_config,
                    override_filename,
                )
                .await;
        }

        // Check passthru.ekapkgs-update.skip attribute
        use crate::package::PackageMetadata;
        match PackageMetadata::from_attr_path(&file, &attr_path).await {
            Ok(metadata) => {
                if metadata.skip == Some(true) {
                    if !force {
                        warn!(
                            "Package '{}' has passthru.ekapkgs-update.skip = true",
                            attr_path
                        );
                        anyhow::bail!("Update skipped. Use --force to override this setting.");
                    } else {
                        warn!(
                            "Package '{}' has passthru.ekapkgs-update.skip = true, but proceeding \
                             due to --force flag",
                            attr_path
                        );
                    }
                }
            },
            Err(e) => {
                debug!(
                    "Could not extract metadata to check skip flag (continuing anyway): {}",
                    e
                );
            },
        }

        // Check if this is a mkManyVariants package
        let is_many_variants = is_many_variants_package(&file, &attr_path).await?;

        if is_many_variants {
            info!("{} is a mkManyVariants package", attr_path);

            // Determine which variants to update
            let variants_to_update = if let Some(ref specific_variant) = variant_config.variant {
                // Update only the specified variant
                vec![specific_variant.clone()]
            } else if variant_config.all_variants {
                // Update all variants (when --all-variants flag is set)
                get_variants_list(&file, &attr_path).await?
            } else {
                // Update only the default variant
                let default_variant = super::get_default_variant(&file, &attr_path).await?;
                info!("Using default variant: {}", default_variant);
                vec![default_variant]
            };

            info!("Variants to update: {:?}", variants_to_update);

            // Update each variant
            for variant_name in variants_to_update {
                // Skip pinned variants (3+ version components)
                if is_variant_pinned(&variant_name) {
                    info!(
                        "Skipping pinned variant '{}' (3+ version components)",
                        variant_name
                    );
                    continue;
                }

                // Infer or use explicit strategy for this variant
                let variant_strategy = match infer_strategy_from_variant(&variant_name) {
                    Some(inferred) => {
                        info!(
                            "Inferred {} strategy for variant '{}'",
                            inferred, variant_name
                        );
                        inferred
                    },
                    None => {
                        info!(
                            "No strategy inferred for variant '{}', using explicit strategy: {}",
                            variant_name, strategy
                        );
                        strategy
                    },
                };

                // Update this variant
                info!(
                    "Updating variant '{}' with strategy {}",
                    variant_name, variant_strategy
                );
                match update_config
                    .clone()
                    .update_single_variant(&file, &attr_path, &variant_name, variant_strategy)
                    .await
                {
                    Ok(()) => info!("Successfully updated variant '{}'", variant_name),
                    Err(e) => {
                        warn!("Failed to update variant '{}': {}", variant_name, e);
                        // Continue with other variants
                    },
                }
            }

            return Ok(());
        }

        // Not a mkManyVariants package - use regular update flow
        // Try to run update script if not ignored
        if !ignore_update_script {
            let script_executed = super::run_update_script(&file, &attr_path).await?;
            if script_executed {
                return Ok(());
            }
        } else {
            info!("Ignoring update script for {}", attr_path);
        }

        // No update script or ignoring it - use generic update method
        // Determine file location: use override if provided, otherwise get from meta.position
        let expr_file_path = if let Some(ref override_file) = override_filename {
            info!("Using override filename: {}", override_file);
            override_file.clone()
        } else {
            debug!("Attempting to locate package definition...");
            let normalized_entry = normalize_entry_point(&file);
            let position_expr =
                format!("with import {normalized_entry} {{ }}; {attr_path}.meta.position");

            eval_nix_expr(&position_expr).await.and_then(|position| {
                if position.is_empty() {
                    anyhow::bail!("Empty position returned from meta.position");
                }
                // Parse position string (format: "file:line")
                let (file_path, _line_str) = position
                    .rsplit_once(':')
                    .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {position}"))?;
                Ok(file_path.to_owned())
            })?
        };

        let (_removed_patches, _test_result) = update_config
            .update_from_file_path(file, attr_path, expr_file_path, version_config)
            .await?;

        Ok(())
    }
}
