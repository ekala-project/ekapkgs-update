//! Types for tracking update workflow phases and state

use serde::{Deserialize, Serialize};

/// Represents distinct phases of the update process
///
/// Each phase represents a logical step in updating a package. Tracking phases
/// separately allows for fine-grained error diagnosis, performance analysis,
/// and retry capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    /// Extracting package metadata from Nix evaluation
    MetadataExtraction,

    /// Discovering upstream source repository and releases
    SourceDiscovery,

    /// Selecting compatible version based on semver strategy
    VersionSelection,

    /// Updating src.hash in Nix expression
    SourceHashUpdate,

    /// Updating dependency hashes (cargo, npm, vendor, etc.)
    DependencyHashUpdate,

    /// Building the package with Nix
    Build,

    /// Detecting and removing obsolete patches
    PatchRecovery,

    /// Running passthru.tests
    Testing,

    /// Creating git commit and GitHub PR
    PrCreation,
}

impl UpdatePhase {
    /// Human-readable description for logs
    pub fn description(&self) -> &'static str {
        match self {
            Self::MetadataExtraction => "Extracting package metadata",
            Self::SourceDiscovery => "Discovering upstream sources",
            Self::VersionSelection => "Selecting compatible version",
            Self::SourceHashUpdate => "Updating source hash",
            Self::DependencyHashUpdate => "Updating dependency hashes",
            Self::Build => "Building package",
            Self::PatchRecovery => "Recovering from patch failures",
            Self::Testing => "Running tests",
            Self::PrCreation => "Creating pull request",
        }
    }

    /// Whether this phase can be retried independently
    ///
    /// Some phases (like building or testing) can be retried without
    /// re-running earlier phases, while others (like metadata extraction)
    /// need the full workflow.
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            Self::Build | Self::Testing | Self::PrCreation | Self::PatchRecovery
        )
    }

    /// Serialize phase name for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MetadataExtraction => "metadata_extraction",
            Self::SourceDiscovery => "source_discovery",
            Self::VersionSelection => "version_selection",
            Self::SourceHashUpdate => "source_hash_update",
            Self::DependencyHashUpdate => "dependency_hash_update",
            Self::Build => "build",
            Self::PatchRecovery => "patch_recovery",
            Self::Testing => "testing",
            Self::PrCreation => "pr_creation",
        }
    }

    /// Parse phase name from string (for database queries)
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "metadata_extraction" => Some(Self::MetadataExtraction),
            "source_discovery" => Some(Self::SourceDiscovery),
            "version_selection" => Some(Self::VersionSelection),
            "source_hash_update" => Some(Self::SourceHashUpdate),
            "dependency_hash_update" => Some(Self::DependencyHashUpdate),
            "build" => Some(Self::Build),
            "patch_recovery" => Some(Self::PatchRecovery),
            "testing" => Some(Self::Testing),
            "pr_creation" => Some(Self::PrCreation),
            _ => None,
        }
    }
}

impl std::fmt::Display for UpdatePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
