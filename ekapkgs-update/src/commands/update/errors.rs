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
// The `*Error` names are part of the serialized and stored format
// (`build_error`, `error_type = 'BuildError'`), so they are kept.
#[allow(clippy::enum_variant_names)]
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
            Self::MetadataError { phase, .. }
            | Self::NetworkError { phase, .. }
            | Self::HashMismatchError { phase, .. }
            | Self::BuildError { phase, .. }
            | Self::TestError { phase, .. }
            | Self::GitError { phase, .. }
            | Self::VersionConstraintError { phase, .. }
            | Self::InfrastructureError { phase, .. } => *phase,
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
                BuildFailureCause::Unknown => "Build failed: unknown cause".to_owned(),
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
            Self::HashMismatchError {
                expected, actual, ..
            } => {
                format!("Expected hash: {}\nActual hash: {}", expected, actual)
            },
            Self::VersionConstraintError { reason, .. } => reason.clone(),
            Self::MetadataError { details, .. }
            | Self::NetworkError { details, .. }
            | Self::GitError { details, .. }
            | Self::InfrastructureError { details, .. } => details.clone(),
        }
    }

    /// Suggested remediation actions for LLM
    pub fn suggested_actions(&self) -> Vec<String> {
        match self {
            Self::HashMismatchError { .. } => vec![
                "Verify the source URL is correct".to_owned(),
                "Check if upstream changed the release artifact".to_owned(),
                "Try fetching manually and inspecting the hash".to_owned(),
            ],
            Self::BuildError {
                suspected_cause: Some(BuildFailureCause::ObsoletePatch { patch_file }),
                ..
            } => vec![
                format!("Remove or update obsolete patch: {}", patch_file),
                "Check if the patch is still needed in new version".to_owned(),
            ],
            Self::BuildError {
                suspected_cause: Some(BuildFailureCause::MissingDependency { package }),
                ..
            } => vec![
                format!("Add missing dependency: {}", package),
                "Check if the dependency name changed in new version".to_owned(),
            ],
            Self::TestError { .. } => vec![
                "Review test output for specific failures".to_owned(),
                "Check if tests need to be disabled or patched".to_owned(),
                "Verify test dependencies are available".to_owned(),
            ],
            Self::NetworkError { .. } => vec![
                "Check network connectivity".to_owned(),
                "Verify the URL is correct and accessible".to_owned(),
                "Check if authentication/credentials are needed".to_owned(),
            ],
            Self::GitError { .. } => vec![
                "Check git configuration and credentials".to_owned(),
                "Verify remote repository permissions".to_owned(),
            ],
            Self::VersionConstraintError { .. } => vec![
                "Review version selection strategy".to_owned(),
                "Check if a newer version is available".to_owned(),
                "Consider relaxing version constraints".to_owned(),
            ],
            _ => vec![
                "Review error details".to_owned(),
                "Check if similar updates have failed before".to_owned(),
            ],
        }
    }

    /// Files likely relevant to fixing this error
    pub fn relevant_files(&self) -> Vec<String> {
        let mut files = vec!["Package Nix file".to_owned()];

        match self {
            Self::BuildError { .. } => {
                files.push("Build logs".to_owned());
                files.push("Diff of changes made".to_owned());
            },
            Self::TestError { .. } => {
                files.push("Test output".to_owned());
                files.push("Package test configuration".to_owned());
            },
            Self::HashMismatchError { .. } => {
                files.push("Source URL".to_owned());
                files.push("Upstream release info".to_owned());
            },
            _ => {
                files.push("Diff of changes made".to_owned());
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
    #[allow(dead_code)]
    pub fn from_anyhow(phase: UpdatePhase, error: &anyhow::Error) -> Self {
        UpdateError::InfrastructureError {
            phase,
            component: "unknown".to_owned(),
            details: format!("{:#}", error),
        }
    }

    /// Classify an untyped workflow error into a structured [`UpdateError`]
    /// based on the phase it occurred in and its message.
    pub fn classify(phase: UpdatePhase, attr_path: &str, error: &anyhow::Error) -> Self {
        let details = format!("{error:#}");
        let lower = details.to_ascii_lowercase();
        let looks_network = [
            "error sending request",
            "connection refused",
            "couldn't resolve host",
            "could not resolve host",
            "timed out",
            "rate limit",
            "http status",
            "status code",
            "unable to download",
            " 404",
            " 403",
            " 429",
            " 500",
            " 502",
            " 503",
        ]
        .iter()
        .any(|needle| lower.contains(needle));
        let status_code = [403u16, 404, 429, 500, 502, 503]
            .into_iter()
            .find(|code| lower.contains(&code.to_string()));

        match phase {
            UpdatePhase::MetadataExtraction | UpdatePhase::SourceDiscovery => {
                UpdateError::MetadataError {
                    phase,
                    attr_path: attr_path.to_owned(),
                    details,
                    nix_error: None,
                }
            },
            UpdatePhase::VersionSelection if !looks_network => {
                UpdateError::VersionConstraintError {
                    phase,
                    current_version: String::new(),
                    available_versions: Vec::new(),
                    constraint: String::new(),
                    reason: details,
                }
            },
            UpdatePhase::VersionSelection => UpdateError::NetworkError {
                phase,
                url: String::new(),
                status_code,
                details,
            },
            UpdatePhase::SourceHashUpdate | UpdatePhase::DependencyHashUpdate
                if looks_network && !lower.contains("builder for") =>
            {
                UpdateError::NetworkError {
                    phase,
                    url: String::new(),
                    status_code,
                    details,
                }
            },
            UpdatePhase::SourceHashUpdate
            | UpdatePhase::DependencyHashUpdate
            | UpdatePhase::Build
            | UpdatePhase::PatchRecovery => UpdateError::BuildError {
                phase,
                exit_code: 1,
                suspected_cause: suspect_build_cause(&details),
                stderr: details,
            },
            UpdatePhase::Testing => UpdateError::TestError {
                phase,
                test_name: None,
                output: details,
                exit_code: 1,
            },
            UpdatePhase::PrCreation if looks_network => UpdateError::NetworkError {
                phase,
                url: String::new(),
                status_code,
                details,
            },
            UpdatePhase::PrCreation => UpdateError::GitError {
                phase,
                operation: "commit/push/pr".to_owned(),
                details,
            },
            UpdatePhase::AgentFix => UpdateError::InfrastructureError {
                phase,
                component: "agent".to_owned(),
                details,
            },
        }
    }
}

/// Best-effort guess at a build failure cause from its log text.
fn suspect_build_cause(log: &str) -> Option<BuildFailureCause> {
    let lower = log.to_ascii_lowercase();
    if lower.contains("hunk") && lower.contains("failed")
        || lower.contains("reversed (or previously applied) patch")
    {
        let patch_file = log
            .lines()
            .find_map(|l| l.trim().strip_prefix("applying patch "))
            .unwrap_or("")
            .to_owned();
        return Some(BuildFailureCause::ObsoletePatch { patch_file });
    }
    if lower.contains("undefined reference to") {
        let missing_symbol = log
            .split("undefined reference to")
            .nth(1)
            .and_then(|rest| rest.split(['`', '\'', '\n']).find(|s| !s.trim().is_empty()))
            .unwrap_or("")
            .trim()
            .to_owned();
        return Some(BuildFailureCause::LinkError { missing_symbol });
    }
    for marker in [
        "modulenotfounderror: no module named ",
        "no package '",
        "could not find ",
    ] {
        if let Some(idx) = lower.find(marker) {
            let rest = &log[idx + marker.len()..];
            let package: String = rest
                .trim_start_matches(['\'', '"'])
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '\'' && *c != '"')
                .collect();
            return Some(BuildFailureCause::MissingDependency { package });
        }
    }
    None
}

#[cfg(test)]
mod classify_tests {
    use super::*;

    #[test]
    fn classify_by_phase_and_message() {
        let cases = [
            (
                UpdatePhase::MetadataExtraction,
                "eval failed",
                "MetadataError",
            ),
            (
                UpdatePhase::VersionSelection,
                "No compatible releases found",
                "VersionConstraintError",
            ),
            (
                UpdatePhase::VersionSelection,
                "error sending request for url",
                "NetworkError",
            ),
            (
                UpdatePhase::SourceHashUpdate,
                "unable to download 'https://x': HTTP error 404",
                "NetworkError",
            ),
            (
                UpdatePhase::Build,
                "builder for '/nix/store/x.drv' failed",
                "BuildError",
            ),
            (UpdatePhase::Testing, "tests failed", "TestError"),
            (UpdatePhase::PrCreation, "git push failed", "GitError"),
            (
                UpdatePhase::AgentFix,
                "agent timed out",
                "InfrastructureError",
            ),
        ];
        for (phase, message, expected) in cases {
            let err = UpdateError::classify(phase, "pkg", &anyhow::anyhow!("{message}"));
            assert_eq!(err.error_type_name(), expected, "{phase}: {message}");
            assert_eq!(err.phase(), phase);
        }
    }

    #[test]
    fn suspects_common_build_causes() {
        assert!(matches!(
            suspect_build_cause("ModuleNotFoundError: No module named 'foo'"),
            Some(BuildFailureCause::MissingDependency { package }) if package == "foo"
        ));
        assert!(matches!(
            suspect_build_cause("main.c: undefined reference to `bar'"),
            Some(BuildFailureCause::LinkError { missing_symbol }) if missing_symbol == "bar"
        ));
        assert!(matches!(
            suspect_build_cause("applying patch fix.patch\nHunk #1 FAILED at 10."),
            Some(BuildFailureCause::ObsoletePatch { .. })
        ));
        assert!(suspect_build_cause("random failure").is_none());
    }
}
