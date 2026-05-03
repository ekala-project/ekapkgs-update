use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::types::{Severity, Vulnerability};

/// OSV.dev API query request
#[derive(Debug, Serialize)]
struct OsvQueryRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<OsvPackage>,
    version: String,
}

/// Package information for OSV query
#[derive(Debug, Serialize)]
struct OsvPackage {
    name: String,
    ecosystem: String,
}

/// OSV.dev API response containing vulnerabilities
#[derive(Debug, Deserialize)]
struct OsvQueryResponse {
    #[serde(default)]
    vulns: Vec<OsvVulnerability>,
}

/// Individual vulnerability from OSV.dev
#[derive(Debug, Deserialize)]
struct OsvVulnerability {
    id: String,
    summary: Option<String>,
    details: Option<String>,
    #[serde(default)]
    severity: Vec<OsvSeverity>,
    #[serde(default)]
    affected: Vec<OsvAffected>,
}

/// Severity information from OSV
#[derive(Debug, Deserialize)]
struct OsvSeverity {
    #[serde(rename = "type")]
    severity_type: String,
    score: Option<String>,
}

/// Affected package versions
#[derive(Debug, Deserialize)]
struct OsvAffected {
    #[serde(default)]
    ranges: Vec<OsvRange>,
}

/// Version range information
#[derive(Debug, Deserialize)]
struct OsvRange {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    range_type: String,
    #[serde(default)]
    events: Vec<OsvEvent>,
}

/// Range event (introduced/fixed)
#[derive(Debug, Deserialize)]
struct OsvEvent {
    #[allow(dead_code)]
    introduced: Option<String>,
    fixed: Option<String>,
}

/// Fetch vulnerabilities for a package and version from OSV.dev
///
/// Queries the OSV.dev API for known vulnerabilities affecting a specific
/// package version. OSV.dev aggregates vulnerability data from 24+ sources
/// including NVD, GitHub Security Advisories, PyPA, RustSec, and more.
///
/// # Arguments
/// * `ecosystem` - The package ecosystem (e.g., "PyPI", "crates.io", "npm")
/// * `package_name` - The package name within that ecosystem
/// * `version` - The specific version to check
///
/// # Returns
/// A vector of Vulnerability structs for all CVEs affecting this version
///
/// # Errors
/// Returns an error if the API request fails or the response cannot be parsed
pub async fn fetch_vulnerabilities(
    ecosystem: &str,
    package_name: &str,
    version: &str,
) -> anyhow::Result<Vec<Vulnerability>> {
    let url = "https://api.osv.dev/v1/query";

    let request = OsvQueryRequest {
        package: Some(OsvPackage {
            name: package_name.to_string(),
            ecosystem: ecosystem.to_string(),
        }),
        version: version.to_string(),
    };

    debug!(
        "Querying OSV.dev for vulnerabilities: {} {}@{}",
        ecosystem, package_name, version
    );

    let client = reqwest::Client::new();
    let response = client
        .post(url)
        .header("User-Agent", "ekapkgs-update")
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("OSV API request failed with status: {}", response.status());
    }

    let osv_response: OsvQueryResponse = response.json().await?;

    debug!(
        "Found {} vulnerabilities for {} {}@{}",
        osv_response.vulns.len(),
        ecosystem,
        package_name,
        version
    );

    Ok(osv_response
        .vulns
        .into_iter()
        .map(|v| convert_osv_vulnerability(v, ecosystem, package_name))
        .collect())
}

/// Convert an OSV vulnerability to our internal Vulnerability type
fn convert_osv_vulnerability(
    osv_vuln: OsvVulnerability,
    _ecosystem: &str,
    _package_name: &str,
) -> Vulnerability {
    // Extract severity - prefer CVSS, fall back to other types
    let severity = osv_vuln
        .severity
        .iter()
        .find(|s| s.severity_type == "CVSS_V3")
        .or_else(|| osv_vuln.severity.first())
        .and_then(|s| s.score.as_ref())
        .map(|score| parse_cvss_severity(score))
        .unwrap_or(Severity::Medium);

    // Get summary, preferring summary field over details
    let summary = osv_vuln
        .summary
        .or(osv_vuln.details)
        .unwrap_or_else(|| "No description available".to_string())
        .lines()
        .next() // Take only first line for summary
        .unwrap_or("No description available")
        .to_string();

    // Extract fixed versions from affected ranges
    let fixed_in: Vec<String> = osv_vuln
        .affected
        .iter()
        .flat_map(|affected| &affected.ranges)
        .flat_map(|range| &range.events)
        .filter_map(|event| event.fixed.clone())
        .collect();

    // Generate OSV.dev URL for the vulnerability
    let details_url = format!("https://osv.dev/{}", osv_vuln.id);

    Vulnerability {
        id: osv_vuln.id,
        severity,
        summary,
        details_url,
        fixed_in,
    }
}

/// Parse CVSS score string to severity level
/// CVSS v3 scoring: 0.0-3.9=Low, 4.0-6.9=Medium, 7.0-8.9=High, 9.0-10.0=Critical
fn parse_cvss_severity(score_str: &str) -> Severity {
    // Try to extract numeric score from string like "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"
    // or just a plain number like "9.8"
    let score: f32 = if score_str.contains('/') {
        // Parse base score from CVSS vector string
        // This is simplified - proper parsing would require CVSS library
        warn!(
            "CVSS vector string parsing not fully implemented: {}",
            score_str
        );
        return Severity::Medium; // Conservative default
    } else {
        score_str.parse().unwrap_or(5.0)
    };

    match score {
        s if s >= 9.0 => Severity::Critical,
        s if s >= 7.0 => Severity::High,
        s if s >= 4.0 => Severity::Medium,
        _ => Severity::Low,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cvss_severity() {
        assert_eq!(parse_cvss_severity("9.8"), Severity::Critical);
        assert_eq!(parse_cvss_severity("7.5"), Severity::High);
        assert_eq!(parse_cvss_severity("5.3"), Severity::Medium);
        assert_eq!(parse_cvss_severity("2.1"), Severity::Low);
    }

    #[test]
    fn test_convert_osv_vulnerability() {
        let osv_vuln = OsvVulnerability {
            id: "CVE-2024-1234".to_string(),
            summary: Some("Test vulnerability".to_string()),
            details: None,
            severity: vec![OsvSeverity {
                severity_type: "CVSS_V3".to_string(),
                score: Some("9.8".to_string()),
            }],
            affected: vec![OsvAffected {
                ranges: vec![OsvRange {
                    range_type: "SEMVER".to_string(),
                    events: vec![OsvEvent {
                        introduced: Some("1.0.0".to_string()),
                        fixed: Some("1.2.3".to_string()),
                    }],
                }],
            }],
        };

        let vuln = convert_osv_vulnerability(osv_vuln, "PyPI", "test-package");

        assert_eq!(vuln.id, "CVE-2024-1234");
        assert_eq!(vuln.severity, Severity::Critical);
        assert_eq!(vuln.summary, "Test vulnerability");
        assert_eq!(vuln.details_url, "https://osv.dev/CVE-2024-1234");
        assert_eq!(vuln.fixed_in, vec!["1.2.3"]);
    }

    // Integration test - requires network access, so marked as ignored
    #[tokio::test]
    #[ignore]
    async fn test_fetch_vulnerabilities_real_api() {
        // Test with a known vulnerable package version
        let result = fetch_vulnerabilities("PyPI", "django", "2.0.0").await;
        assert!(result.is_ok());
        // Django 2.0.0 is old and should have known vulnerabilities
        let vulns = result.unwrap();
        assert!(!vulns.is_empty());
    }
}
