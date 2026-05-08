use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use sqlx::{Row, SqlitePool};
use tracing::debug;

use super::types::RepologyInfo;

/// Get cached Repology data if available and not expired
///
/// # Arguments
/// * `pool` - SQLite connection pool
/// * `project_name` - Repology project name
///
/// # Returns
/// Some(RepologyInfo) if cached data exists and is not expired, None otherwise
///
/// # Cache TTL
/// Repology data is cached for 72 hours (3 days) as Repology updates are not real-time
pub async fn get_cached_repology_data(
    pool: &SqlitePool,
    project_name: &str,
) -> Result<Option<RepologyInfo>> {
    let row = sqlx::query(
        r#"
        SELECT project_data, expires_at
        FROM repology_cache
        WHERE project_name = ?
        "#,
    )
    .bind(project_name)
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
                    debug!("Repology cache expired for {}", project_name);
                    return Ok(None);
                }
            }

            // Deserialize project data from JSON
            let project_json: String = row.try_get("project_data")?;
            let info: RepologyInfo = serde_json::from_str(&project_json)?;

            debug!("Repology cache hit for {}", project_name);
            Ok(Some(info))
        },
        None => {
            debug!("Repology cache miss for {}", project_name);
            Ok(None)
        },
    }
}

/// Store Repology data in cache with 72-hour TTL
///
/// # Arguments
/// * `pool` - SQLite connection pool
/// * `info` - RepologyInfo to cache
pub async fn cache_repology_data(pool: &SqlitePool, info: &RepologyInfo) -> Result<()> {
    let cached_at = Utc::now();
    let expires_at = cached_at + Duration::hours(72); // 72-hour TTL

    let project_json = serde_json::to_string(info)?;

    sqlx::query(
        r#"
        INSERT INTO repology_cache (project_name, project_data, cached_at, expires_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(project_name) DO UPDATE SET
            project_data = excluded.project_data,
            cached_at = excluded.cached_at,
            expires_at = excluded.expires_at
        "#,
    )
    .bind(&info.project_name)
    .bind(&project_json)
    .bind(cached_at.to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .execute(pool)
    .await?;

    debug!(
        "Cached Repology data for {} (expires at {})",
        info.project_name,
        expires_at.format("%Y-%m-%d %H:%M:%S")
    );

    Ok(())
}

/// Clean up expired Repology cache entries
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
        DELETE FROM repology_cache
        WHERE expires_at < ?
        "#,
    )
    .bind(&now)
    .execute(pool)
    .await?;

    let rows_deleted = result.rows_affected();
    if rows_deleted > 0 {
        debug!("Cleaned up {} expired Repology cache entries", rows_deleted);
    }

    Ok(rows_deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repology::types::RepologyPackage;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        // Create the repology_cache table
        sqlx::query(
            r#"
            CREATE TABLE repology_cache (
                project_name TEXT PRIMARY KEY,
                project_data TEXT NOT NULL,
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

        let result = get_cached_repology_data(&pool, "nonexistent-project")
            .await
            .unwrap();

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_cache_hit() {
        let pool = setup_test_db().await;

        let info = RepologyInfo {
            project_name: "test-project".to_owned(),
            packages: vec![RepologyPackage {
                repo: "arch".to_owned(),
                version: "1.2.3".to_owned(),
                status: Some(crate::repology::types::PackageStatus::Newest),
                srcname: None,
                binname: None,
                visiblename: None,
                summary: None,
                categories: None,
                licenses: None,
                maintainers: None,
            }],
            fetched_at: Utc::now().to_rfc3339(),
        };

        // Cache the data
        cache_repology_data(&pool, &info).await.unwrap();

        // Retrieve it
        let result = get_cached_repology_data(&pool, "test-project")
            .await
            .unwrap();

        assert!(result.is_some());
        let cached_info = result.unwrap();
        assert_eq!(cached_info.project_name, "test-project");
        assert_eq!(cached_info.packages.len(), 1);
        assert_eq!(cached_info.packages[0].version, "1.2.3");
    }

    #[tokio::test]
    async fn test_cache_expiration() -> anyhow::Result<()> {
        let pool = setup_test_db().await;

        // Insert an expired entry manually
        let expired_time = Utc::now() - Duration::hours(73); // Expired 73 hours ago
        let info = RepologyInfo {
            project_name: "expired-project".to_owned(),
            packages: Vec::new(),
            fetched_at: expired_time.to_rfc3339(),
        };
        let info_json = serde_json::to_string(&info)?;

        sqlx::query(
            r#"
            INSERT INTO repology_cache (project_name, project_data, cached_at, expires_at)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind("expired-project")
        .bind(&info_json)
        .bind(expired_time.to_rfc3339())
        .bind(expired_time.to_rfc3339())
        .execute(&pool)
        .await?;

        // Try to retrieve - should return None due to expiration
        let result = get_cached_repology_data(&pool, "expired-project").await?;

        assert_eq!(result, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let pool = setup_test_db().await;

        // Insert an expired entry
        let expired_time = Utc::now() - Duration::hours(73);
        let info_json = "{}";

        sqlx::query(
            r#"
            INSERT INTO repology_cache (project_name, project_data, cached_at, expires_at)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind("old-project")
        .bind(info_json)
        .bind(expired_time.to_rfc3339())
        .bind(expired_time.to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

        // Insert a valid entry
        let info = RepologyInfo {
            project_name: "new-project".to_owned(),
            packages: Vec::new(),
            fetched_at: Utc::now().to_rfc3339(),
        };
        cache_repology_data(&pool, &info).await.unwrap();

        // Clean up expired entries
        let deleted = cleanup_expired_cache(&pool).await.unwrap();

        assert_eq!(deleted, 1);

        // Verify only the valid entry remains
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM repology_cache")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(count, 1);
    }
}
