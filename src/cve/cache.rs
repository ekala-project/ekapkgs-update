use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use sqlx::{Row, SqlitePool};
use tracing::debug;

use super::types::Vulnerability;

/// Cache key format for storing CVE data
/// Format: "{ecosystem}:{package_name}:{version}"
fn make_cache_key(ecosystem: &str, package_name: &str, version: &str) -> String {
    format!("{}:{}:{}", ecosystem, package_name, version)
}

/// Get cached CVE data if available and not expired
///
/// # Arguments
/// * `pool` - SQLite connection pool
/// * `ecosystem` - Package ecosystem
/// * `package_name` - Package name
/// * `version` - Package version
///
/// # Returns
/// Some(vulnerabilities) if cached data exists and is not expired, None otherwise
pub async fn get_cached_cve_data(
    pool: &SqlitePool,
    ecosystem: &str,
    package_name: &str,
    version: &str,
) -> Result<Option<Vec<Vulnerability>>> {
    let key = make_cache_key(ecosystem, package_name, version);

    let row = sqlx::query(
        r#"
        SELECT vulnerabilities, expires_at
        FROM cve_cache
        WHERE package_key = ?
        "#,
    )
    .bind(&key)
    .fetch_optional(pool)
    .await?;

    match row {
        Some(row) => {
            let expires_at: String = row.try_get("expires_at")?;
            let expires_at = DateTime::parse_from_rfc3339(&expires_at)
                .ok()
                .map(|dt| dt.with_timezone(&Utc));

            // Check if cache entry has expired
            if let Some(exp) = expires_at {
                if exp < Utc::now() {
                    debug!("CVE cache expired for {}", key);
                    return Ok(None);
                }
            }

            // Deserialize vulnerabilities from JSON
            let vulns_json: String = row.try_get("vulnerabilities")?;
            let vulnerabilities: Vec<Vulnerability> = serde_json::from_str(&vulns_json)?;

            debug!(
                "CVE cache hit for {} ({} vulnerabilities)",
                key,
                vulnerabilities.len()
            );
            Ok(Some(vulnerabilities))
        }
        None => {
            debug!("CVE cache miss for {}", key);
            Ok(None)
        }
    }
}

/// Store CVE data in cache with 24-hour TTL
///
/// # Arguments
/// * `pool` - SQLite connection pool
/// * `ecosystem` - Package ecosystem
/// * `package_name` - Package name
/// * `version` - Package version
/// * `vulnerabilities` - List of vulnerabilities to cache
pub async fn cache_cve_data(
    pool: &SqlitePool,
    ecosystem: &str,
    package_name: &str,
    version: &str,
    vulnerabilities: &[Vulnerability],
) -> Result<()> {
    let key = make_cache_key(ecosystem, package_name, version);
    let cached_at = Utc::now();
    let expires_at = cached_at + Duration::hours(24); // 24-hour TTL

    let vulns_json = serde_json::to_string(vulnerabilities)?;

    sqlx::query(
        r#"
        INSERT INTO cve_cache (package_key, vulnerabilities, cached_at, expires_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(package_key) DO UPDATE SET
            vulnerabilities = excluded.vulnerabilities,
            cached_at = excluded.cached_at,
            expires_at = excluded.expires_at
        "#,
    )
    .bind(&key)
    .bind(&vulns_json)
    .bind(cached_at.to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .execute(pool)
    .await?;

    debug!(
        "Cached {} vulnerabilities for {} (expires at {})",
        vulnerabilities.len(),
        key,
        expires_at.format("%Y-%m-%d %H:%M:%S")
    );

    Ok(())
}

/// Clean up expired cache entries
///
/// This should be called periodically (e.g., on database initialization) to
/// remove stale cache entries and keep the database size manageable.
///
/// # Arguments
/// * `pool` - SQLite connection pool
///
/// # Returns
/// Number of expired entries deleted
pub async fn cleanup_expired_cache(pool: &SqlitePool) -> Result<u64> {
    let now = Utc::now().to_rfc3339();

    let result = sqlx::query(
        r#"
        DELETE FROM cve_cache
        WHERE expires_at < ?
        "#,
    )
    .bind(&now)
    .execute(pool)
    .await?;

    let rows_deleted = result.rows_affected();
    if rows_deleted > 0 {
        debug!("Cleaned up {} expired CVE cache entries", rows_deleted);
    }

    Ok(rows_deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cve::types::Severity;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        // Create the cve_cache table
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
    async fn test_cache_miss() {
        let pool = setup_test_db().await;

        let result = get_cached_cve_data(&pool, "PyPI", "test-package", "1.0.0")
            .await
            .unwrap();

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_cache_hit() {
        let pool = setup_test_db().await;

        let vulnerabilities = vec![Vulnerability {
            id: "CVE-2024-1234".to_string(),
            severity: Severity::High,
            summary: "Test vulnerability".to_string(),
            details_url: "https://osv.dev/CVE-2024-1234".to_string(),
            fixed_in: vec!["1.2.3".to_string()],
        }];

        // Cache the data
        cache_cve_data(&pool, "PyPI", "test-package", "1.0.0", &vulnerabilities)
            .await
            .unwrap();

        // Retrieve it
        let result = get_cached_cve_data(&pool, "PyPI", "test-package", "1.0.0")
            .await
            .unwrap();

        assert!(result.is_some());
        let cached_vulns = result.unwrap();
        assert_eq!(cached_vulns.len(), 1);
        assert_eq!(cached_vulns[0].id, "CVE-2024-1234");
    }

    #[tokio::test]
    async fn test_cache_expiration() {
        let pool = setup_test_db().await;

        // Insert an expired entry manually
        let key = make_cache_key("PyPI", "test-package", "1.0.0");
        let expired_time = Utc::now() - Duration::hours(25); // Expired 25 hours ago
        let vulns_json = "[]";

        sqlx::query(
            r#"
            INSERT INTO cve_cache (package_key, vulnerabilities, cached_at, expires_at)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(&key)
        .bind(vulns_json)
        .bind(expired_time.to_rfc3339())
        .bind(expired_time.to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

        // Try to retrieve - should return None due to expiration
        let result = get_cached_cve_data(&pool, "PyPI", "test-package", "1.0.0")
            .await
            .unwrap();

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let pool = setup_test_db().await;

        // Insert an expired entry
        let expired_time = Utc::now() - Duration::hours(25);
        sqlx::query(
            r#"
            INSERT INTO cve_cache (package_key, vulnerabilities, cached_at, expires_at)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind("PyPI:old-package:1.0.0")
        .bind("[]")
        .bind(expired_time.to_rfc3339())
        .bind(expired_time.to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

        // Insert a valid entry
        cache_cve_data(&pool, "PyPI", "new-package", "2.0.0", &[])
            .await
            .unwrap();

        // Clean up expired entries
        let deleted = cleanup_expired_cache(&pool).await.unwrap();

        assert_eq!(deleted, 1);

        // Verify only the valid entry remains
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cve_cache")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_cache_update() {
        let pool = setup_test_db().await;

        // Cache initial data
        let vulns1 = vec![Vulnerability {
            id: "CVE-2024-1111".to_string(),
            severity: Severity::Low,
            summary: "First".to_string(),
            details_url: "https://osv.dev/CVE-2024-1111".to_string(),
            fixed_in: vec![],
        }];

        cache_cve_data(&pool, "PyPI", "test-package", "1.0.0", &vulns1)
            .await
            .unwrap();

        // Update with new data
        let vulns2 = vec![Vulnerability {
            id: "CVE-2024-2222".to_string(),
            severity: Severity::High,
            summary: "Second".to_string(),
            details_url: "https://osv.dev/CVE-2024-2222".to_string(),
            fixed_in: vec![],
        }];

        cache_cve_data(&pool, "PyPI", "test-package", "1.0.0", &vulns2)
            .await
            .unwrap();

        // Verify the data was updated
        let result = get_cached_cve_data(&pool, "PyPI", "test-package", "1.0.0")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "CVE-2024-2222");
    }
}
