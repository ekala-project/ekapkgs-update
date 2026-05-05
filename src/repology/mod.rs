mod api;
mod cache;
mod types;

use anyhow::Result;
pub use api::RepologyClient;
use sqlx::SqlitePool;
use tracing::{debug, info, warn};
pub use types::RepologyInfo;

/// Check Repology for the latest version of a package
///
/// This function queries Repology.org to find what other distributions consider
/// the newest stable version of a package. This provides cross-distribution
/// validation of version information.
///
/// # Arguments
/// * `pool` - Database connection pool for caching
/// * `client` - Repology API client (with rate limiting)
/// * `package_name` - Package name (will try multiple normalized forms)
/// * `skip_repology` - If true, skip Repology check and return None
///
/// # Returns
/// RepologyInfo if successful, None if package not found or check is skipped
///
/// # Caching
/// Results are cached for 72 hours to minimize API calls
pub async fn check_repology_version(
    pool: &SqlitePool,
    client: &RepologyClient,
    package_name: &str,
    skip_repology: bool,
) -> Result<Option<RepologyInfo>> {
    if skip_repology {
        debug!("Repology checking disabled");
        return Ok(None);
    }

    // Try multiple normalized forms of the package name
    let candidates = RepologyClient::normalize_package_name(package_name);

    for candidate in &candidates {
        debug!("Trying Repology lookup for: {}", candidate);

        // Check cache first
        if let Some(cached) = cache::get_cached_repology_data(pool, candidate).await? {
            if !cached.packages.is_empty() {
                info!(
                    "Found {} in Repology ({} packages across distributions)",
                    candidate,
                    cached.packages.len()
                );
                return Ok(Some(cached));
            }
            // Empty cache entry means package not found, try next candidate
            continue;
        }

        // Cache miss - fetch from Repology API
        match client.get_project(candidate).await {
            Ok(info) => {
                // Cache the result (even if empty)
                cache::cache_repology_data(pool, &info).await?;

                if !info.packages.is_empty() {
                    info!(
                        "Found {} in Repology ({} packages)",
                        candidate,
                        info.packages.len()
                    );
                    return Ok(Some(info));
                }
                // Empty result, try next candidate
            },
            Err(e) => {
                warn!("Failed to fetch Repology data for {}: {}", candidate, e);
                // Continue to next candidate on error
            },
        }
    }

    debug!(
        "Package {} not found in Repology (tried {} candidates)",
        package_name,
        candidates.len()
    );
    Ok(None)
}

/// Clean up expired Repology cache entries
///
/// Should be called on database initialization to keep cache size manageable.
pub async fn cleanup_cache(pool: &SqlitePool) -> Result<u64> {
    cache::cleanup_expired_cache(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

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
    async fn test_check_with_skip_flag() {
        let pool = setup_test_db().await;
        let client = RepologyClient::new();

        let result = check_repology_version(&pool, &client, "firefox", true)
            .await
            .unwrap();

        assert_eq!(result, None);
    }

    // Integration test - requires network access
    #[tokio::test]
    #[ignore]
    async fn test_check_real_package() {
        let pool = setup_test_db().await;
        let client = RepologyClient::new();

        let result = check_repology_version(&pool, &client, "firefox", false)
            .await
            .unwrap();

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.project_name, "firefox");
        assert!(!info.packages.is_empty());

        // Should have a newest version
        assert!(info.get_newest_version().is_some());
    }

    #[tokio::test]
    #[ignore]
    async fn test_check_with_name_normalization() {
        let pool = setup_test_db().await;
        let client = RepologyClient::new();

        // Try with python prefix - should normalize to just "requests"
        let result = check_repology_version(&pool, &client, "python3-requests", false)
            .await
            .unwrap();

        // Should find it as "requests" in Repology
        assert!(result.is_some());
        let info = result.unwrap();
        assert!(!info.packages.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_caching_works() {
        let pool = setup_test_db().await;
        let client = RepologyClient::new();

        // First request - should hit API
        let result1 = check_repology_version(&pool, &client, "firefox", false)
            .await
            .unwrap();
        assert!(result1.is_some());

        // Second request - should hit cache (no network call)
        let result2 = check_repology_version(&pool, &client, "firefox", false)
            .await
            .unwrap();
        assert!(result2.is_some());

        // Results should be the same
        assert_eq!(result1.unwrap().project_name, result2.unwrap().project_name);
    }
}
