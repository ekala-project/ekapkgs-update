use anyhow::Context;
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
            name: package_name.to_owned(),
            ecosystem: ecosystem.to_owned(),
        }),
        version: version.to_owned(),
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
        .await
        .with_context(|| format!("POST {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!("OSV API request failed with status: {}", response.status());
    }

    let osv_response: OsvQueryResponse = response
        .json()
        .await
        .with_context(|| format!("decode JSON from {url}"))?;

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
        .unwrap_or_else(|| "No description available".to_owned())
        .lines()
        .next() // Take only first line for summary
        .unwrap_or("No description available").to_owned();

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

/// Parse CVSS score string to severity level.
/// Accepts either a plain numeric score (e.g. "9.8") or a CVSS v3 vector string
/// (e.g. "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H").
///
/// CVSS v3 scoring: 0.0-3.9=Low, 4.0-6.9=Medium, 7.0-8.9=High, 9.0-10.0=Critical
fn parse_cvss_severity(score_str: &str) -> Severity {
    let score: f32 = if score_str.contains('/') {
        // CVSS vector string — parse base score from metric components
        match parse_cvss_vector_score(score_str) {
            Some(s) => s,
            None => {
                warn!("Could not parse CVSS vector string, defaulting to Medium: {score_str}");
                return Severity::Medium;
            },
        }
    } else {
        score_str.parse().unwrap_or(5.0)
    };

    severity_from_score(score)
}

fn severity_from_score(score: f32) -> Severity {
    match score {
        s if s >= 9.0 => Severity::Critical,
        s if s >= 7.0 => Severity::High,
        s if s >= 4.0 => Severity::Medium,
        _ => Severity::Low,
    }
}

/// Parse a CVSS v3.x vector string and compute the base score.
///
/// Implements the CVSS v3.0/v3.1 base score algorithm per the FIRST specification.
/// Expects a string containing slash-separated `METRIC:VALUE` pairs, optionally
/// prefixed with `CVSS:3.x/`.
fn parse_cvss_vector_score(vector: &str) -> Option<f32> {
    // Strip optional "CVSS:3.x/" prefix
    let metrics_part = vector
        .strip_prefix("CVSS:")
        .and_then(|s| s.split_once('/'))
        .map_or(vector, |(_, rest)| rest);

    let metrics: std::collections::HashMap<&str, &str> = metrics_part
        .split('/')
        .filter_map(|part| part.split_once(':'))
        .collect();

    let av = match *metrics.get("AV")? {
        "N" => 0.85,
        "A" => 0.62,
        "L" => 0.55,
        "P" => 0.20,
        _ => return None,
    };
    let ac = match *metrics.get("AC")? {
        "L" => 0.77,
        "H" => 0.44,
        _ => return None,
    };
    let scope_changed = *metrics.get("S")? == "C";
    let pr = match (*metrics.get("PR")?, scope_changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.50,
        _ => return None,
    };
    let ui = match *metrics.get("UI")? {
        "N" => 0.85,
        "R" => 0.62,
        _ => return None,
    };

    let c = impact_value(metrics.get("C")?)?;
    let i = impact_value(metrics.get("I")?)?;
    let a = impact_value(metrics.get("A")?)?;

    // Impact Sub-Score
    let iss = 1.0 - (1.0 - c) * (1.0 - i) * (1.0 - a);

    let impact = if scope_changed {
        7.52 * (iss - 0.029) - 3.25 * (iss - 0.02_f32).powf(15.0)
    } else {
        6.42 * iss
    };

    if impact <= 0.0 {
        return Some(0.0);
    }

    let exploitability = 8.22 * av * ac * pr * ui;

    let base = if scope_changed {
        (1.08 * (impact + exploitability)).min(10.0)
    } else {
        (impact + exploitability).min(10.0)
    };

    // Round up to one decimal place per CVSS spec
    Some((base * 10.0).ceil() / 10.0)
}

fn impact_value(metric: &str) -> Option<f32> {
    match metric {
        "H" => Some(0.56),
        "L" => Some(0.22),
        "N" => Some(0.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cvss_severity_numeric() {
        assert_eq!(parse_cvss_severity("9.8"), Severity::Critical);
        assert_eq!(parse_cvss_severity("7.5"), Severity::High);
        assert_eq!(parse_cvss_severity("5.3"), Severity::Medium);
        assert_eq!(parse_cvss_severity("2.1"), Severity::Low);
    }

    #[test]
    fn test_parse_cvss_severity_vector_critical() {
        // NIST calculator: AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H = 9.8 Critical
        assert_eq!(
            parse_cvss_severity("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"),
            Severity::Critical,
        );
    }

    #[test]
    fn test_parse_cvss_severity_vector_high() {
        // AV:N/AC:L/PR:L/UI:N/S:U/C:H/I:H/A:N = 8.1 High
        assert_eq!(
            parse_cvss_severity("CVSS:3.1/AV:N/AC:L/PR:L/UI:N/S:U/C:H/I:H/A:N"),
            Severity::High,
        );
    }

    #[test]
    fn test_parse_cvss_severity_vector_medium() {
        // AV:N/AC:H/PR:L/UI:N/S:U/C:L/I:L/A:N = 4.2 Medium
        assert_eq!(
            parse_cvss_severity("CVSS:3.1/AV:N/AC:H/PR:L/UI:N/S:U/C:L/I:L/A:N"),
            Severity::Medium,
        );
    }

    #[test]
    fn test_parse_cvss_severity_vector_low() {
        // AV:P/AC:H/PR:H/UI:R/S:U/C:L/I:N/A:N = 1.6 Low
        assert_eq!(
            parse_cvss_severity("CVSS:3.1/AV:P/AC:H/PR:H/UI:R/S:U/C:L/I:N/A:N"),
            Severity::Low,
        );
    }

    #[test]
    fn test_parse_cvss_severity_vector_scope_changed() {
        // AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H = 10.0 Critical
        assert_eq!(
            parse_cvss_severity("CVSS:3.0/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H"),
            Severity::Critical,
        );
    }

    #[test]
    fn test_parse_cvss_severity_vector_no_impact() {
        // All impact metrics are None → score 0.0 → Low
        assert_eq!(
            parse_cvss_severity("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N"),
            Severity::Low,
        );
    }

    #[test]
    fn test_parse_cvss_vector_score_invalid() {
        assert_eq!(parse_cvss_vector_score("garbage"), None);
        assert_eq!(parse_cvss_vector_score("AV:X/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), None);
    }

    #[test]
    fn test_convert_osv_vulnerability() {
        let osv_vuln = OsvVulnerability {
            id: "CVE-2024-1234".to_owned(),
            summary: Some("Test vulnerability".to_owned()),
            details: None,
            severity: vec![OsvSeverity {
                severity_type: "CVSS_V3".to_owned(),
                score: Some("9.8".to_owned()),
            }],
            affected: vec![OsvAffected {
                ranges: vec![OsvRange {
                    range_type: "SEMVER".to_owned(),
                    events: vec![OsvEvent {
                        introduced: Some("1.0.0".to_owned()),
                        fixed: Some("1.2.3".to_owned()),
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
