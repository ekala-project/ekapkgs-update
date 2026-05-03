use serde::{Deserialize, Serialize};

/// Represents the severity level of a vulnerability
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// Parse severity from OSV severity string
    #[allow(dead_code)]
    pub fn from_osv_severity(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "CRITICAL" => Severity::Critical,
            "HIGH" => Severity::High,
            "MEDIUM" | "MODERATE" => Severity::Medium,
            "LOW" => Severity::Low,
            _ => Severity::Medium, // Default to Medium for unknown
        }
    }

    /// Get a human-readable string for display
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Critical => "Critical",
            Severity::High => "High",
            Severity::Medium => "Medium",
            Severity::Low => "Low",
        }
    }
}

/// Represents a single vulnerability (CVE or other identifier)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vulnerability {
    /// The vulnerability ID (e.g., "CVE-2024-1234" or "GHSA-xxxx-yyyy-zzzz")
    pub id: String,

    /// Severity level of the vulnerability
    pub severity: Severity,

    /// Short summary of the vulnerability
    pub summary: String,

    /// URL to vulnerability details
    pub details_url: String,

    /// Versions where this vulnerability is fixed (if applicable)
    pub fixed_in: Vec<String>,
}

impl Vulnerability {
    /// Generate a markdown list item for this vulnerability
    /// Format: "- [CVE-ID](url) - Severity: Summary"
    pub fn to_markdown(&self) -> String {
        format!(
            "- [{}]({}) - {}: {}",
            self.id, self.details_url, self.severity.as_str(), self.summary
        )
    }
}

/// Analysis of CVE changes between two versions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CveAnalysis {
    /// Vulnerabilities fixed by updating to the new version
    pub resolved: Vec<Vulnerability>,

    /// New vulnerabilities introduced in the new version
    pub introduced: Vec<Vulnerability>,

    /// Vulnerabilities present in both old and new versions
    pub present: Vec<Vulnerability>,
}

impl CveAnalysis {
    /// Check if there are any CVEs to report
    pub fn has_cves(&self) -> bool {
        !self.resolved.is_empty() || !self.introduced.is_empty() || !self.present.is_empty()
    }

    /// Generate a markdown section for PR descriptions
    /// Returns None if there are no CVEs to report (matching nixpkgs-update behavior)
    pub fn to_markdown(&self) -> Option<String> {
        if !self.has_cves() {
            return None;
        }

        let mut sections = Vec::new();

        if !self.resolved.is_empty() {
            let mut section = String::from("### CVEs Resolved ✅\n");
            for vuln in &self.resolved {
                section.push_str(&vuln.to_markdown());
                section.push('\n');
            }
            sections.push(section);
        }

        if !self.introduced.is_empty() {
            let mut section = String::from("### CVEs Introduced ⚠️\n");
            for vuln in &self.introduced {
                section.push_str(&vuln.to_markdown());
                section.push('\n');
            }
            sections.push(section);
        }

        if !self.present.is_empty() {
            let mut section = String::from("### CVEs Present in Both Versions\n");
            for vuln in &self.present {
                section.push_str(&vuln.to_markdown());
                section.push('\n');
            }
            sections.push(section);
        }

        Some(format!("## Security\n\n{}", sections.join("\n")))
    }

    /// Count total number of resolved CVEs (for database storage)
    pub fn resolved_count(&self) -> usize {
        self.resolved.len()
    }

    /// Count total number of introduced CVEs (for database storage)
    pub fn introduced_count(&self) -> usize {
        self.introduced.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_parsing() {
        assert_eq!(Severity::from_osv_severity("CRITICAL"), Severity::Critical);
        assert_eq!(Severity::from_osv_severity("high"), Severity::High);
        assert_eq!(Severity::from_osv_severity("MEDIUM"), Severity::Medium);
        assert_eq!(Severity::from_osv_severity("low"), Severity::Low);
        assert_eq!(Severity::from_osv_severity("unknown"), Severity::Medium);
    }

    #[test]
    fn test_vulnerability_markdown() {
        let vuln = Vulnerability {
            id: "CVE-2024-1234".to_string(),
            severity: Severity::Critical,
            summary: "Remote code execution".to_string(),
            details_url: "https://osv.dev/CVE-2024-1234".to_string(),
            fixed_in: vec!["1.2.3".to_string()],
        };

        assert_eq!(
            vuln.to_markdown(),
            "- [CVE-2024-1234](https://osv.dev/CVE-2024-1234) - Critical: Remote code execution"
        );
    }

    #[test]
    fn test_cve_analysis_has_cves() {
        let mut analysis = CveAnalysis::default();
        assert!(!analysis.has_cves());

        analysis.resolved.push(Vulnerability {
            id: "CVE-2024-1234".to_string(),
            severity: Severity::High,
            summary: "Test".to_string(),
            details_url: "https://osv.dev/CVE-2024-1234".to_string(),
            fixed_in: vec![],
        });
        assert!(analysis.has_cves());
    }

    #[test]
    fn test_cve_analysis_markdown_empty() {
        let analysis = CveAnalysis::default();
        assert_eq!(analysis.to_markdown(), None);
    }

    #[test]
    fn test_cve_analysis_markdown_with_resolved() {
        let mut analysis = CveAnalysis::default();
        analysis.resolved.push(Vulnerability {
            id: "CVE-2024-1234".to_string(),
            severity: Severity::Critical,
            summary: "Buffer overflow".to_string(),
            details_url: "https://osv.dev/CVE-2024-1234".to_string(),
            fixed_in: vec!["1.2.3".to_string()],
        });

        let markdown = analysis.to_markdown().unwrap();
        assert!(markdown.contains("## Security"));
        assert!(markdown.contains("### CVEs Resolved ✅"));
        assert!(markdown.contains("CVE-2024-1234"));
        assert!(markdown.contains("Buffer overflow"));
    }
}
