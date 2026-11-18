//! SQLite persistence for the CVE scan (`osv_vulnerabilities`,
//! `package_vulnerabilities` and the `updates.cve_*` scan state).

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::{FromRow, SqlitePool};

use super::scan::{MergedVuln, SCANNABLE_UPSTREAMS, ScanTarget, VulnInfo, VulnSeverity};

/// Packages due for a scan: never scanned, scanned before `stale_before`
/// (RFC 3339), or scanned at another version. `force` selects every
/// scannable package.
pub async fn due_targets(
    pool: &SqlitePool,
    repo: Option<&str>,
    stale_before: &str,
    force: bool,
) -> Result<Vec<ScanTarget>> {
    let upstreams = serde_json::to_string(&SCANNABLE_UPSTREAMS)?;
    sqlx::query_as::<_, ScanTarget>(
        "SELECT repo_name, attr_path, pname, upstream_type, upstream_key, upstream_instance, \
         current_version, latest_upstream_version FROM updates \
         WHERE current_version IS NOT NULL AND current_version != '' \
           AND upstream_type IN (SELECT value FROM json_each(?1)) \
           AND upstream_key IS NOT NULL \
           AND (?2 IS NULL OR repo_name = ?2) \
           AND (?3 = 1 OR cve_scanned_at IS NULL OR cve_scanned_at < ?4 \
                OR cve_scanned_version IS NOT current_version) \
         ORDER BY cve_scanned_at IS NOT NULL, cve_scanned_at, repo_name, attr_path",
    )
    .bind(upstreams)
    .bind(repo)
    .bind(i64::from(force))
    .bind(stale_before)
    .fetch_all(pool)
    .await
    .context("select packages due for a CVE scan")
}

#[derive(FromRow)]
struct VulnRow {
    id: String,
    modified: String,
    aliases: String,
    summary: Option<String>,
    severity: String,
    cvss_score: Option<f64>,
    cvss_vector: Option<String>,
    published: Option<String>,
    withdrawn: Option<String>,
}

impl From<VulnRow> for VulnInfo {
    fn from(row: VulnRow) -> Self {
        Self {
            id: row.id,
            modified: row.modified,
            aliases: serde_json::from_str(&row.aliases).unwrap_or_default(),
            summary: row.summary,
            severity: VulnSeverity::parse(&row.severity),
            cvss_score: row.cvss_score,
            cvss_vector: row.cvss_vector,
            published: row.published,
            withdrawn: row.withdrawn,
        }
    }
}

/// Cached OSV records for `ids`.
pub async fn load_vulns(pool: &SqlitePool, ids: &[String]) -> Result<HashMap<String, VulnInfo>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<VulnRow> = sqlx::query_as(
        "SELECT id, modified, aliases, summary, severity, cvss_score, cvss_vector, published, \
         withdrawn FROM osv_vulnerabilities WHERE id IN (SELECT value FROM json_each(?1))",
    )
    .bind(serde_json::to_string(ids)?)
    .fetch_all(pool)
    .await
    .context("load cached OSV records")?;
    Ok(rows
        .into_iter()
        .map(|row| (row.id.clone(), VulnInfo::from(row)))
        .collect())
}

/// Cache fetched OSV records.
pub async fn store_vulns(pool: &SqlitePool, vulns: &[VulnInfo]) -> Result<()> {
    if vulns.is_empty() {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    for v in vulns {
        sqlx::query(
            "INSERT INTO osv_vulnerabilities (id, modified, aliases, summary, severity, \
             cvss_score, cvss_vector, published, withdrawn, fetched_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(id) DO UPDATE SET modified = excluded.modified, \
             aliases = excluded.aliases, summary = excluded.summary, \
             severity = excluded.severity, cvss_score = excluded.cvss_score, \
             cvss_vector = excluded.cvss_vector, published = excluded.published, \
             withdrawn = excluded.withdrawn, fetched_at = excluded.fetched_at",
        )
        .bind(&v.id)
        .bind(&v.modified)
        .bind(serde_json::to_string(&v.aliases)?)
        .bind(&v.summary)
        .bind(v.severity.as_str())
        .bind(v.cvss_score)
        .bind(&v.cvss_vector)
        .bind(&v.published)
        .bind(&v.withdrawn)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("cache OSV record {}", v.id))?;
    }
    tx.commit().await?;
    Ok(())
}

/// Replace a package's vulnerabilities with the result of a scan at `now`
/// (keeping `first_seen` of those already known) and mark it scanned.
pub async fn record_scan(
    pool: &SqlitePool,
    target: &ScanTarget,
    vulns: &[MergedVuln],
    now: &str,
) -> Result<()> {
    let latest = target
        .latest_upstream_version
        .as_deref()
        .filter(|v| *v != target.current_version);
    let mut tx = pool.begin().await?;
    for v in vulns {
        sqlx::query(
            "INSERT INTO package_vulnerabilities (repo_name, attr_path, vuln_id, osv_ids, \
             version, severity, cvss_score, summary, fixed_in_latest, latest_version, \
             first_seen, last_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11) \
             ON CONFLICT(repo_name, attr_path, vuln_id) DO UPDATE SET \
             osv_ids = excluded.osv_ids, version = excluded.version, \
             severity = excluded.severity, cvss_score = excluded.cvss_score, \
             summary = excluded.summary, fixed_in_latest = excluded.fixed_in_latest, \
             latest_version = excluded.latest_version, last_seen = excluded.last_seen",
        )
        .bind(&target.repo_name)
        .bind(&target.attr_path)
        .bind(&v.vuln_id)
        .bind(serde_json::to_string(&v.osv_ids)?)
        .bind(&target.current_version)
        .bind(v.severity.as_str())
        .bind(v.cvss_score)
        .bind(&v.summary)
        .bind(v.fixed_in_latest)
        .bind(latest)
        .bind(now)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("record {} for {}", v.vuln_id, target.attr_path))?;
    }
    sqlx::query(
        "DELETE FROM package_vulnerabilities \
         WHERE repo_name = ?1 AND attr_path = ?2 AND last_seen != ?3",
    )
    .bind(&target.repo_name)
    .bind(&target.attr_path)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE updates SET cve_scanned_at = ?3, cve_scanned_version = ?4, cve_scan_error = NULL \
         WHERE repo_name = ?1 AND attr_path = ?2",
    )
    .bind(&target.repo_name)
    .bind(&target.attr_path)
    .bind(now)
    .bind(&target.current_version)
    .execute(&mut *tx)
    .await?;
    tx.commit()
        .await
        .with_context(|| format!("record CVE scan of {}", target.attr_path))
}

/// Record a failed scan. Earlier results and the scan time are kept, so the
/// package is retried by the next scan.
pub async fn record_scan_error(pool: &SqlitePool, target: &ScanTarget, error: &str) -> Result<()> {
    sqlx::query("UPDATE updates SET cve_scan_error = ?3 WHERE repo_name = ?1 AND attr_path = ?2")
        .bind(&target.repo_name)
        .bind(&target.attr_path)
        .bind(error)
        .execute(pool)
        .await
        .with_context(|| format!("record CVE scan error for {}", target.attr_path))?;
    Ok(())
}

/// A package with high or critical vulnerabilities at its current version.
#[derive(Debug, Clone, FromRow)]
pub struct VulnerablePackage {
    pub repo_name: String,
    pub attr_path: String,
    pub current_version: String,
    pub latest_upstream_version: Option<String>,
    pub critical: i64,
    pub high: i64,
    pub fixed_by_update: i64,
    pub max_score: Option<f64>,
}

/// Packages with high or critical vulnerabilities, most severe first.
pub async fn high_severity_packages(
    pool: &SqlitePool,
    repo: Option<&str>,
) -> Result<Vec<VulnerablePackage>> {
    sqlx::query_as(
        "SELECT u.repo_name, u.attr_path, u.current_version, u.latest_upstream_version, \
         SUM(v.severity = 'critical') AS critical, SUM(v.severity = 'high') AS high, \
         SUM(COALESCE(v.fixed_in_latest, 0)) AS fixed_by_update, MAX(v.cvss_score) AS max_score \
         FROM package_vulnerabilities v JOIN updates u \
           ON u.repo_name = v.repo_name AND u.attr_path = v.attr_path \
          AND u.current_version = v.version \
         WHERE v.severity IN ('critical', 'high') AND (?1 IS NULL OR u.repo_name = ?1) \
         GROUP BY u.repo_name, u.attr_path \
         ORDER BY critical DESC, high DESC, max_score DESC, u.attr_path",
    )
    .bind(repo)
    .fetch_all(pool)
    .await
    .context("select packages with high-severity vulnerabilities")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        for (attr, kind, version, latest) in [
            ("curl", "github", "8.4.0", Some("8.5.0")),
            ("hello", "directory", "2.12", None),
            ("python3Packages.django", "pypi", "4.2.0", None),
            ("unversioned", "github", "", None),
        ] {
            sqlx::query(
                "INSERT INTO updates (repo_name, attr_path, status, current_version, \
                 latest_upstream_version, upstream_type, upstream_key) \
                 VALUES ('', ?1, 'up_to_date', ?2, ?3, ?4, 'k')",
            )
            .bind(attr)
            .bind(version)
            .bind(latest)
            .bind(kind)
            .execute(&pool)
            .await
            .unwrap();
        }
        pool
    }

    fn merged(id: &str, severity: VulnSeverity, fixed: Option<bool>) -> MergedVuln {
        MergedVuln {
            vuln_id: id.to_owned(),
            osv_ids: vec![id.to_owned()],
            severity,
            cvss_score: Some(7.5),
            summary: Some("s".to_owned()),
            fixed_in_latest: fixed,
        }
    }

    fn attrs(targets: &[ScanTarget]) -> Vec<&str> {
        targets.iter().map(|t| t.attr_path.as_str()).collect()
    }

    #[tokio::test]
    async fn due_targets_selects_scannable_unscanned_packages() {
        let pool = setup().await;
        let later = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let past = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let due = due_targets(&pool, None, &past, false).await.unwrap();
        assert_eq!(attrs(&due), vec!["curl", "python3Packages.django"]);

        let now = Utc::now().to_rfc3339();
        record_scan(&pool, &due[0], &[], &now).await.unwrap();
        let due2 = due_targets(&pool, None, &past, false).await.unwrap();
        assert_eq!(attrs(&due2), vec!["python3Packages.django"]);
        // Too old.
        let due3 = due_targets(&pool, None, &later, false).await.unwrap();
        assert_eq!(attrs(&due3), vec!["python3Packages.django", "curl"]);
        // Forced.
        assert_eq!(
            due_targets(&pool, None, &past, true).await.unwrap().len(),
            2
        );
        // Version changed since the scan.
        sqlx::query("UPDATE updates SET current_version = '8.5.0' WHERE attr_path = 'curl'")
            .execute(&pool)
            .await
            .unwrap();
        let due4 = due_targets(&pool, None, &past, false).await.unwrap();
        assert_eq!(attrs(&due4), vec!["python3Packages.django", "curl"]);
        // Other repo.
        assert!(
            due_targets(&pool, Some("other"), &past, true)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn record_scan_replaces_rows_and_keeps_first_seen() {
        let pool = setup().await;
        let past = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let curl = due_targets(&pool, None, &past, false)
            .await
            .unwrap()
            .remove(0);

        record_scan(
            &pool,
            &curl,
            &[
                merged("CVE-1", VulnSeverity::Critical, Some(true)),
                merged("CVE-2", VulnSeverity::High, Some(false)),
                merged("CVE-3", VulnSeverity::Medium, Some(true)),
            ],
            "2026-11-01T00:00:00+00:00",
        )
        .await
        .unwrap();
        let high = high_severity_packages(&pool, None).await.unwrap();
        assert_eq!(high.len(), 1);
        assert_eq!(
            (high[0].critical, high[0].high, high[0].fixed_by_update),
            (1, 1, 1)
        );

        record_scan(
            &pool,
            &curl,
            &[merged("CVE-2", VulnSeverity::High, Some(false))],
            "2026-11-02T00:00:00+00:00",
        )
        .await
        .unwrap();
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT vuln_id, first_seen, last_seen FROM package_vulnerabilities ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            vec![(
                "CVE-2".to_owned(),
                "2026-11-01T00:00:00+00:00".to_owned(),
                "2026-11-02T00:00:00+00:00".to_owned()
            )]
        );

        // Rows of an older version are not reported.
        sqlx::query("UPDATE updates SET current_version = '8.5.0' WHERE attr_path = 'curl'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            high_severity_packages(&pool, None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn record_scan_error_keeps_results() {
        let pool = setup().await;
        let past = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let curl = due_targets(&pool, None, &past, false)
            .await
            .unwrap()
            .remove(0);
        record_scan_error(&pool, &curl, "HTTP 503").await.unwrap();
        let (at, err): (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT cve_scanned_at, cve_scan_error FROM updates WHERE attr_path = 'curl'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((at, err.as_deref()), (None, Some("HTTP 503")));
    }

    #[tokio::test]
    async fn vuln_cache_round_trip() {
        let pool = setup().await;
        let v = VulnInfo {
            id: "GHSA-x".to_owned(),
            modified: "m1".to_owned(),
            aliases: vec!["CVE-9".to_owned()],
            summary: Some("s".to_owned()),
            severity: VulnSeverity::High,
            cvss_score: Some(8.1),
            cvss_vector: Some("CVSS:3.1/…".to_owned()),
            published: None,
            withdrawn: None,
        };
        store_vulns(&pool, std::slice::from_ref(&v)).await.unwrap();
        let loaded = load_vulns(&pool, &["GHSA-x".to_owned(), "nope".to_owned()])
            .await
            .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded["GHSA-x"], v);
    }
}
