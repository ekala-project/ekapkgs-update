//! Structured error types for update workflow diagnosis
//!
//! This module provides detailed, categorized error types that help LLMs and humans
//! understand what went wrong during an update. Each error type includes relevant
//! context and suggested remediation actions.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::types::UpdatePhase;

/// Structured error types for LLM understanding
///
/// Each variant contains detailed context about the failure, enabling
/// programmatic diagnosis and remediation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpdateError {
    /// Failed to extract package metadata from Nix
    MetadataError {
        phase: UpdatePhase,
        attr_path: String,
        details: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        nix_error: Option<String>,
    },

    /// Network-related failure
    NetworkError {
        phase: UpdatePhase,
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status_code: Option<u16>,
        details: String,
    },

    /// Hash mismatch during fetch or build
    HashMismatchError {
        phase: UpdatePhase,
        context: String, // "source" or "cargo" or "npm" etc
        expected: String,
        actual: String,
    },

    /// Build failed with Nix
    BuildError {
        phase: UpdatePhase,
        exit_code: i32,
        stderr: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        suspected_cause: Option<BuildFailureCause>,
    },

    /// Tests failed
    TestError {
        phase: UpdatePhase,
        #[serde(skip_serializing_if = "Option::is_none")]
        test_name: Option<String>,
        output: String,
        exit_code: i32,
    },

    /// Git operation failed
    GitError {
        phase: UpdatePhase,
        operation: String, // "commit", "push", "create-branch", etc
        details: String,
    },

    /// Version selection constraints couldn't be satisfied
    VersionConstraintError {
        phase: UpdatePhase,
        current_version: String,
        available_versions: Vec<String>,
        constraint: String,
        reason: String,
    },

    /// Infrastructure/tooling failure
    InfrastructureError {
        phase: UpdatePhase,
        component: String, // "database", "worktree", "nix-eval", etc
        details: String,
    },
}

/// Common build failure patterns for quick diagnosis
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case")]
pub enum BuildFailureCause {
    MissingDependency { package: String },
    CompilerError { error_type: String },
    LinkError { missing_symbol: String },
    ObsoletePatch { patch_file: String },
    IncompatibleVersion { component: String, issue: String },
    TestFailure { test: String },
    Unknown,
}

impl UpdateError {
    /// Generate LLM-friendly description
    pub fn to_llm_context(&self) -> serde_json::Value {
        json!({
            "error_type": self.error_type_name(),
            "phase": self.phase(),
            "summary": self.summary(),
            "details": self.details(),
            "suggested_actions": self.suggested_actions(),
            "relevant_files": self.relevant_files(),
        })
    }

    /// Short error type name
    pub fn error_type_name(&self) -> &'static str {
        match self {
            Self::MetadataError { .. } => "MetadataError",
            Self::NetworkError { .. } => "NetworkError",
            Self::HashMismatchError { .. } => "HashMismatchError",
            Self::BuildError { .. } => "BuildError",
            Self::TestError { .. } => "TestError",
            Self::GitError { .. } => "GitError",
            Self::VersionConstraintError { .. } => "VersionConstraintError",
            Self::InfrastructureError { .. } => "InfrastructureError",
        }
    }

    /// Get the phase where error occurred
    pub fn phase(&self) -> UpdatePhase {
        match self {
            Self::MetadataError { phase, .. } => *phase,
            Self::NetworkError { phase, .. } => *phase,
            Self::HashMismatchError { phase, .. } => *phase,
            Self::BuildError { phase, .. } => *phase,
            Self::TestError { phase, .. } => *phase,
            Self::GitError { phase, .. } => *phase,
            Self::VersionConstraintError { phase, .. } => *phase,
            Self::InfrastructureError { phase, .. } => *phase,
        }
    }

    /// One-line summary
    pub fn summary(&self) -> String {
        match self {
            Self::BuildError {
                suspected_cause: Some(cause),
                ..
            } => match cause {
                BuildFailureCause::MissingDependency { package } => {
                    format!("Build failed: missing dependency '{}'", package)
                },
                BuildFailureCause::CompilerError { error_type } => {
                    format!("Build failed: compiler error ({})", error_type)
                },
                BuildFailureCause::LinkError { missing_symbol } => {
                    format!(
                        "Build failed: link error (missing symbol '{}')",
                        missing_symbol
                    )
                },
                BuildFailureCause::ObsoletePatch { patch_file } => {
                    format!("Build failed: obsolete patch '{}'", patch_file)
                },
                BuildFailureCause::IncompatibleVersion { component, issue } => {
                    format!("Build failed: incompatible {} ({})", component, issue)
                },
                BuildFailureCause::TestFailure { test } => {
                    format!("Build failed: test '{}' failed", test)
                },
                BuildFailureCause::Unknown => "Build failed: unknown cause".to_string(),
            },
            Self::TestError {
                test_name: Some(name),
                ..
            } => {
                format!("Test '{}' failed", name)
            },
            Self::NetworkError {
                url,
                status_code: Some(code),
                ..
            } => {
                format!("Network error fetching {} (HTTP {})", url, code)
            },
            Self::HashMismatchError { context, .. } => {
                format!("Hash mismatch in {}", context)
            },
            Self::MetadataError { attr_path, .. } => {
                format!("Failed to extract metadata for {}", attr_path)
            },
            Self::GitError { operation, .. } => {
                format!("Git operation failed: {}", operation)
            },
            Self::VersionConstraintError { constraint, .. } => {
                format!("No version satisfies constraint: {}", constraint)
            },
            Self::InfrastructureError { component, .. } => {
                format!("Infrastructure failure: {}", component)
            },
            _ => format!("{}: {}", self.error_type_name(), self.details()),
        }
    }

    /// Detailed context
    pub fn details(&self) -> String {
        match self {
            Self::BuildError { stderr, .. } => stderr.clone(),
            Self::TestError { output, .. } => output.clone(),
            Self::MetadataError { details, .. } => details.clone(),
            Self::NetworkError { details, .. } => details.clone(),
            Self::HashMismatchError {
                expected, actual, ..
            } => {
                format!("Expected hash: {}\nActual hash: {}", expected, actual)
            },
            Self::GitError { details, .. } => details.clone(),
            Self::VersionConstraintError { reason, .. } => reason.clone(),
            Self::InfrastructureError { details, .. } => details.clone(),
        }
    }

    /// Suggested remediation actions for LLM
    pub fn suggested_actions(&self) -> Vec<String> {
        match self {
            Self::HashMismatchError { .. } => vec![
                "Verify the source URL is correct".to_string(),
                "Check if upstream changed the release artifact".to_string(),
                "Try fetching manually and inspecting the hash".to_string(),
            ],
            Self::BuildError {
                suspected_cause: Some(BuildFailureCause::ObsoletePatch { patch_file }),
                ..
            } => vec![
                format!("Remove or update obsolete patch: {}", patch_file),
                "Check if the patch is still needed in new version".to_string(),
            ],
            Self::BuildError {
                suspected_cause: Some(BuildFailureCause::MissingDependency { package }),
                ..
            } => vec![
                format!("Add missing dependency: {}", package),
                "Check if the dependency name changed in new version".to_string(),
            ],
            Self::TestError { .. } => vec![
                "Review test output for specific failures".to_string(),
                "Check if tests need to be disabled or patched".to_string(),
                "Verify test dependencies are available".to_string(),
            ],
            Self::NetworkError { .. } => vec![
                "Check network connectivity".to_string(),
                "Verify the URL is correct and accessible".to_string(),
                "Check if authentication/credentials are needed".to_string(),
            ],
            Self::GitError { .. } => vec![
                "Check git configuration and credentials".to_string(),
                "Verify remote repository permissions".to_string(),
            ],
            Self::VersionConstraintError { .. } => vec![
                "Review version selection strategy".to_string(),
                "Check if a newer version is available".to_string(),
                "Consider relaxing version constraints".to_string(),
            ],
            _ => vec![
                "Review error details".to_string(),
                "Check if similar updates have failed before".to_string(),
            ],
        }
    }

    /// Files likely relevant to fixing this error
    pub fn relevant_files(&self) -> Vec<String> {
        let mut files = vec!["Package Nix file".to_string()];

        match self {
            Self::BuildError { .. } => {
                files.push("Build logs".to_string());
                files.push("Diff of changes made".to_string());
            },
            Self::TestError { .. } => {
                files.push("Test output".to_string());
                files.push("Package test configuration".to_string());
            },
            Self::HashMismatchError { .. } => {
                files.push("Source URL".to_string());
                files.push("Upstream release info".to_string());
            },
            _ => {
                files.push("Diff of changes made".to_string());
            },
        }

        files
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.summary())
    }
}

impl std::error::Error for UpdateError {}

/// Convert anyhow errors to UpdateError when phase context is available
impl UpdateError {
    /// Wrap a generic error with phase context
    pub fn from_anyhow(phase: UpdatePhase, error: anyhow::Error) -> Self {
        UpdateError::InfrastructureError {
            phase,
            component: "unknown".to_string(),
            details: format!("{:#}", error),
        }
    }
}
