use crate::vcs_sources::SemverStrategy;

/// Configuration for Git operations
#[derive(Debug, Clone)]
pub struct GitConfig {
    /// Whether to create a git commit
    pub commit: bool,
    /// Whether to create a pull request
    pub create_pr: bool,
    /// Fork repository URL for creating PRs
    pub fork: String,
}

impl GitConfig {
    pub fn new(commit: bool, create_pr: bool, fork: String) -> Self {
        Self {
            commit,
            create_pr,
            fork,
        }
    }
}

/// Configuration for test execution
#[derive(Debug, Clone)]
pub struct TestConfig {
    /// Whether to run passthru.tests
    pub run_passthru_tests: bool,
    /// Whether to fail on test failure
    pub fail_on_test_failure: bool,
}

impl TestConfig {
    pub fn new(run_passthru_tests: bool, fail_on_test_failure: bool) -> Self {
        Self {
            run_passthru_tests,
            fail_on_test_failure,
        }
    }
}

/// Configuration for variant updates
#[derive(Debug, Clone)]
pub struct VariantConfig {
    /// Specific variant to update (None means default or all)
    pub variant: Option<String>,
    /// Whether to update all variants
    pub all_variants: bool,
}

impl VariantConfig {
    pub fn new(variant: Option<String>, all_variants: bool) -> Self {
        Self {
            variant,
            all_variants,
        }
    }

    pub fn specific(variant: String) -> Self {
        Self {
            variant: Some(variant),
            all_variants: false,
        }
    }

    pub fn all() -> Self {
        Self {
            variant: None,
            all_variants: true,
        }
    }

    pub fn default_only() -> Self {
        Self {
            variant: None,
            all_variants: false,
        }
    }
}

/// Main configuration for package updates
#[derive(Debug, Clone)]
pub struct UpdateConfig {
    /// Path to the Nix file
    pub file: String,
    /// Package attribute path (e.g., "pkgs.hello")
    pub attr_path: String,
    /// Semver versioning strategy
    pub semver_strategy: SemverStrategy,
    /// Whether to ignore update scripts
    pub ignore_update_script: bool,
    /// Optional upstream URL override
    pub upstream: Option<String>,
    /// Git configuration
    pub git: GitConfig,
    /// Test configuration
    pub test: TestConfig,
    /// Variant configuration
    pub variant: VariantConfig,
}

impl UpdateConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        file: String,
        attr_path: String,
        semver_strategy: SemverStrategy,
        ignore_update_script: bool,
        upstream: Option<String>,
        git: GitConfig,
        test: TestConfig,
        variant: VariantConfig,
    ) -> Self {
        Self {
            file,
            attr_path,
            semver_strategy,
            ignore_update_script,
            upstream,
            git,
            test,
            variant,
        }
    }
}
