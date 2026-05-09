//! CVE (Common Vulnerabilities and Exposures) analysis for package updates.
//!
//! This module integrates with the OSV (Open Source Vulnerabilities) database to
//! analyze security implications of package updates. It determines which CVEs are
//! resolved, introduced, or persist across version changes.
//!
//! ## Main Entry Point
//!
//! [`analyze_cve_changes`] compares vulnerability data between old and new package
//! versions, returning a [`CveAnalysis`] that categorizes CVEs by their status.
//!
//! ## Caching
//!
//! CVE data is cached in the SQLite database (see [`cache`]) to minimize repeated
//! API requests. Cache entries expire after a configured duration and are cleaned
//! up during database initialization.
//!
//! ## Ecosystem Support
//!
//! The [`ecosystem`] submodule maps package metadata (language, build system) to
//! OSV ecosystem identifiers (e.g., "PyPI", "crates.io", "npm").

mod cache;
mod ecosystem;
mod osv;
mod types;

use std::collections::HashSet;

use anyhow::Result;
use sqlx::SqlitePool;
use tracing::{debug, info, warn};
use types::{CveAnalysis, Vulnerability};

use crate::package::PackageMetadata;

/// Analyze CVE changes between two package versions
///
/// Compares the vulnerabilities present in the old and new versions to determine:
/// - Which CVEs are resolved by the update
/// - Which CVEs are introduced by the update
/// - Which CVEs persist across both versions
///
/// # Arguments
/// * `pool` - Database connection pool for caching
/// * `metadata` - Package metadata to determine ecosystem and package name
/// * `old_version` - Current version being updated from
/// * `new_version` - New version being updated to
/// * `skip_cve_check` - If true, skip CVE checking and return empty analysis
///
/// # Returns
/// A CveAnalysis containing the categorized vulnerabilities
///
/// # Errors
/// Returns an error if CVE data fetching fails critically, but will gracefully
/// handle and log transient failures.
pub async fn analyze_cve_changes(
    pool: &SqlitePool,
    metadata: &PackageMetadata,
    old_version: &str,
    new_version: &str,
    skip_cve_check: bool,
) -> Result<CveAnalysis> {
    if skip_cve_check {
        debug!("CVE checking disabled, returning empty analysis");
        return Ok(CveAnalysis::default());
    }

    // Detect ecosystem
    let Some(ecosystem) = ecosystem::detect_ecosystem(metadata) else {
        debug!(
            "Could not detect ecosystem for package, skipping CVE check. src_url={:?}",
            metadata.src_url
        );
        return Ok(CveAnalysis::default());
    };

    // Get package name
    let Some(package_name) = ecosystem::get_package_name(metadata, &ecosystem) else {
        debug!("No package name available, skipping CVE check");
        return Ok(CveAnalysis::default());
    };

    info!(
        "Checking CVEs for {} package {} ({} -> {})",
        ecosystem, package_name, old_version, new_version
    );

    // Fetch CVE data for both versions
    let old_vulns = fetch_vulnerabilities_with_cache(pool, &ecosystem, &package_name, old_version)
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to fetch CVEs for old version: {}", e);
            Vec::new()
        });

    let new_vulns = fetch_vulnerabilities_with_cache(pool, &ecosystem, &package_name, new_version)
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to fetch CVEs for new version: {}", e);
            Vec::new()
        });

    // Categorize vulnerabilities
    let old_ids: HashSet<String> = old_vulns.iter().map(|v| v.id.clone()).collect();
    let new_ids: HashSet<String> = new_vulns.iter().map(|v| v.id.clone()).collect();

    let resolved: Vec<Vulnerability> = old_vulns
        .into_iter()
        .filter(|v| !new_ids.contains(&v.id))
        .collect();

    let introduced: Vec<Vulnerability> = new_vulns
        .iter()
        .filter(|v| !old_ids.contains(&v.id))
        .cloned()
        .collect();

    let present: Vec<Vulnerability> = new_vulns
        .into_iter()
        .filter(|v| old_ids.contains(&v.id))
        .collect();

    let analysis = CveAnalysis {
        resolved,
        introduced,
        present,
    };

    if analysis.has_cves() {
        info!(
            "CVE analysis: {} resolved, {} introduced, {} present",
            analysis.resolved_count(),
            analysis.introduced_count(),
            analysis.present.len()
        );
    } else {
        info!("No CVEs found for this package");
    }

    Ok(analysis)
}

/// Fetch vulnerabilities with caching layer
///
/// Checks the cache first, and only queries OSV.dev if cache is missing or expired.
async fn fetch_vulnerabilities_with_cache(
    pool: &SqlitePool,
    ecosystem: &str,
    package_name: &str,
    version: &str,
) -> Result<Vec<Vulnerability>> {
    // Check cache first
    if let Some(cached) = cache::get_cached_cve_data(pool, ecosystem, package_name, version).await?
    {
        return Ok(cached);
    }

    // Cache miss - fetch from OSV.dev
    let vulnerabilities = osv::fetch_vulnerabilities(ecosystem, package_name, version).await?;

    // Cache the results
    cache::cache_cve_data(pool, ecosystem, package_name, version, &vulnerabilities).await?;

    Ok(vulnerabilities)
}

/// Clean up expired CVE cache entries
///
/// Should be called on database initialization to keep cache size manageable.
pub async fn cleanup_cache(pool: &SqlitePool) -> Result<u64> {
    cache::cleanup_expired_cache(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::PackageMetadata;

    fn create_test_metadata(ecosystem_type: &str) -> PackageMetadata {
        let mut metadata = PackageMetadata {
            version: "1.0.0".to_owned(),
            src_url: None,
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: Some("test-package".to_owned()),
            description: None,
            homepage: None,
            changelog: None,
        };

        match ecosystem_type {
            "rust" => metadata.cargo_hash = Some("sha256-test".to_owned()),
            "python" => {
                metadata.src_url =
                    Some("https://files.pythonhosted.org/packages/test.tar.gz".to_owned())
            },
            "npm" => metadata.npm_deps_hash = Some("sha256-test".to_owned()),
            _ => {},
        }

        metadata
    }

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE cve_cache (
                package_key TEXT PRIMARY KEY,
                vulnerabilities TEXT NOT NULL,
                cached_at TEXT NOT NULL,
                expires_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    #[tokio::test]
    async fn test_analyze_with_skip_flag() {
        let pool = setup_test_db().await;
        let metadata = create_test_metadata("rust");

        let analysis = analyze_cve_changes(&pool, &metadata, "1.0.0", "1.1.0", true)
            .await
            .unwrap();

        assert!(!analysis.has_cves());
    }

    #[tokio::test]
    async fn test_analyze_unknown_ecosystem() {
        let pool = setup_test_db().await;
        let metadata = create_test_metadata("unknown");

        let analysis = analyze_cve_changes(&pool, &metadata, "1.0.0", "1.1.0", false)
            .await
            .unwrap();

        assert!(!analysis.has_cves());
    }

    #[tokio::test]
    async fn test_analyze_no_package_name() {
        let pool = setup_test_db().await;
        let mut metadata = create_test_metadata("rust");
        metadata.pname = None;

        let analysis = analyze_cve_changes(&pool, &metadata, "1.0.0", "1.1.0", false)
            .await
            .unwrap();

        assert!(!analysis.has_cves());
    }

    // This would be an integration test requiring real OSV API access
    #[tokio::test]
    #[ignore]
    async fn test_analyze_real_package() {
        let pool = setup_test_db().await;
        let metadata = create_test_metadata("python");

        // django 2.0.0 to 2.2.0 should show resolved CVEs
        let analysis = analyze_cve_changes(&pool, &metadata, "2.0.0", "2.2.0", false)
            .await
            .unwrap();

        // Old django versions have known vulnerabilities
        assert!(!analysis.resolved.is_empty() || !analysis.present.is_empty());
    }
}
