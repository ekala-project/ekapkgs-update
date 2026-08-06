use std::fmt;
use std::path::PathBuf;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

/// Severity of an audit finding
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn label(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Category of the audit check
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckCategory {
    BrokenReferences,
    LayoutIssues,
    BinaryIssues,
    ScriptIssues,
    MetadataIssues,
    SecurityIssues,
}

impl CheckCategory {
    pub fn label(&self) -> &'static str {
        match self {
            CheckCategory::BrokenReferences => "Broken References",
            CheckCategory::LayoutIssues => "Layout Issues",
            CheckCategory::BinaryIssues => "Binary Issues",
            CheckCategory::ScriptIssues => "Script Issues",
            CheckCategory::MetadataIssues => "Metadata Issues",
            CheckCategory::SecurityIssues => "Security Issues",
        }
    }
}

impl fmt::Display for CheckCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A single finding from an audit check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditFinding {
    pub severity: Severity,
    pub category: CheckCategory,
    pub check_name: String,
    pub message: String,
    pub output_name: String,
    pub file_path: Option<Utf8PathBuf>,
    pub details: Option<String>,
}

/// Complete audit result for a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditReport {
    pub attr_path: String,
    pub store_paths: Vec<(String, PathBuf)>,
    pub findings: Vec<AuditFinding>,
    pub checks_run: usize,
    pub duration_ms: u64,
}

impl AuditReport {
    pub fn error_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count()
    }

    pub fn info_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Info)
            .count()
    }

    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }
}

/// Configuration controlling which checks to run
#[derive(Debug, Clone)]
pub struct AuditConfig {
    pub check_symlinks: bool,
    pub check_store_references: bool,
    pub check_shared_libs: bool,
    pub check_layout: bool,
    pub check_rpaths: bool,
    pub check_shebangs: bool,
    pub check_fhs_references: bool,
    pub check_pkg_config: bool,
    pub check_metadata: bool,
    pub check_security: bool,
    /// Minimum severity to include in report (findings below this are filtered)
    pub min_severity: Severity,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            check_symlinks: true,
            check_store_references: true,
            check_shared_libs: true,
            check_layout: true,
            check_rpaths: true,
            check_shebangs: true,
            check_fhs_references: true,
            check_pkg_config: true,
            check_metadata: true,
            check_security: true,
            min_severity: Severity::Info,
        }
    }
}
