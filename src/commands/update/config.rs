//! Configuration structures for update operations
//!
//! This module contains configuration structs that group related parameters
//! to reduce function argument counts and improve code organization.

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
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            commit: false,
            create_pr: false,
            upstream: None,
            fork: "origin".to_string(),
            run_passthru_tests: false,
            src_only: false,
            format: false,
        }
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

    /// Override filename for package definition (ignores meta.position)
    pub override_filename: Option<String>,

    /// System parameter for cross-platform evaluation (TODO: implement)
    pub system: Option<String>,

    /// Common update configuration
    pub update_config: UpdateConfig,

    /// Version selection configuration
    pub version_config: VersionConfig,

    /// Variant configuration (for mkManyVariants packages)
    pub variant_config: VariantConfig,

    /// Flake configuration
    pub flake_config: FlakeConfig,
}
