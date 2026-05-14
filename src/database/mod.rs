//! SQLite database interface for tracking package update attempts and history.
//!
//! ## Schema
//!
//! The database manages two primary tables:
//!
//! - **`updates`**: Tracks the state of each package, including when it was last checked, when it
//!   should be re-checked, current version, proposed version, and rebuild impact (if known). The
//!   `attr_path` column is the primary key.
//!
//! - **`update_logs`**: Records failures for post-mortem analysis, indexed by derivation path. Each
//!   log entry captures the error message, old/new versions, and timestamp.
//!
//! ## Backoff Semantics
//!
//! When an update check finds no new version, the system backs off exponentially:
//!
//! - **First failure**: re-check in 2 days ([`FIRST_BACKOFF_DAYS`])
//! - **Second failure** (within 2 days of the first): re-check in 4 days ([`SECOND_BACKOFF_DAYS`])
//! - **Subsequent failures**: re-check in 6 days ([`MAX_BACKOFF_DAYS`])
//!
//! Successful updates reset the backoff to 2 days. This prevents excessive API
//! traffic while ensuring packages are eventually re-checked.
//!
//! ## Connection Pool and Cleanup
//!
//! [`Database::new`] establishes a connection pool and runs schema migrations via
//! `sqlx::migrate!`. After migrations, it opportunistically cleans up expired cache
//! entries for both CVE and Repology data by calling their respective `cleanup_cache`
//! functions. Cleanup failures are logged but do not fail database initialization.
//!
//! The connection pool remains alive as long as the `Database` struct is in scope.
//! `Database` is `Clone`, so multiple handles can share the same underlying pool.

mod sessions;
mod types;

pub use sessions::{PhaseRecord, SessionStatus, UpdateSession};

use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use tracing::{debug, info};
pub use types::UpdateLog;
use types::UpdateRecord;

/// Parse an optional RFC 3339 timestamp string into UTC, ignoring parse errors.
///
/// Used by row-mapping code where invalid stored timestamps degrade to `None`
/// rather than aborting record loading.
fn parse_rfc3339(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// First-failure backoff: how many days before re-checking after the first
/// failed update attempt.
const FIRST_BACKOFF_DAYS: i64 = 2;
/// Second-failure backoff: applied when the previous attempt was within
/// `FIRST_BACKOFF_DAYS` days.
const SECOND_BACKOFF_DAYS: i64 = 4;
/// Maximum backoff: applied for any subsequent failure.
const MAX_BACKOFF_DAYS: i64 = 6;

/// Upper bound (inclusive) of the "small" rebuild bucket.
const REBUILD_BUCKET_SMALL_MAX: i64 = 10;
/// Upper bound (inclusive) of the "medium" rebuild bucket.
const REBUILD_BUCKET_MEDIUM_MAX: i64 = 50;
/// Upper bound (inclusive) of the "large" rebuild bucket.
const REBUILD_BUCKET_LARGE_MAX: i64 = 100;

/// Histogram bucket for `rebuild_count` distributions.
///
/// The label values returned by [`RebuildBucket::label`] match exactly the
/// strings the rebuild-distribution SQL `CASE` expression emits, so they are
/// also used as map keys when consumers need a stable ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RebuildBucket {
    /// `0..=REBUILD_BUCKET_SMALL_MAX`
    Small,
    /// `(REBUILD_BUCKET_SMALL_MAX..=REBUILD_BUCKET_MEDIUM_MAX]`
    Medium,
    /// `(REBUILD_BUCKET_MEDIUM_MAX..=REBUILD_BUCKET_LARGE_MAX]`
    Large,
    /// `> REBUILD_BUCKET_LARGE_MAX`
    Huge,
}

impl RebuildBucket {
    /// Render the human-readable bucket label (e.g. `"0-10"`, `"101+"`).
    ///
    /// The string matches the SQL `CASE` branches in
    /// [`Database::get_rebuild_distribution`].
    pub fn label(self) -> String {
        match self {
            RebuildBucket::Small => format!("0-{REBUILD_BUCKET_SMALL_MAX}"),
            RebuildBucket::Medium => format!(
                "{}-{REBUILD_BUCKET_MEDIUM_MAX}",
                REBUILD_BUCKET_SMALL_MAX + 1
            ),
            RebuildBucket::Large => format!(
                "{}-{REBUILD_BUCKET_LARGE_MAX}",
                REBUILD_BUCKET_MEDIUM_MAX + 1
            ),
            RebuildBucket::Huge => format!("{}+", REBUILD_BUCKET_LARGE_MAX + 1),
        }
    }
}

impl std::fmt::Display for RebuildBucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// Database connection wrapper for tracking package updates
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl From<SqlitePool> for Database {
    /// Construct a `Database` from a pre-built `SqlitePool`.
    ///
    /// This is primarily useful for tests, which want to bind to
    /// `SqlitePool::connect("sqlite::memory:")` and skip
    /// [`Database::new`]'s schema-bootstrap steps when the test fixture
    /// has already run them.
    fn from(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Database {
    /// Initialize the database connection and create tables if needed
    pub async fn new(db_path: &str) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = Path::new(db_path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }

        // Create connection options
        let options = SqliteConnectOptions::from_str(db_path)
            .with_context(|| format!("parse sqlite URL {db_path}"))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        // Connect to database
        let pool = SqlitePool::connect_with(options)
            .await
            .with_context(|| format!("connect to sqlite database {db_path}"))?;

        info!("Connected to database at {}", db_path);

        // Run migrations
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("Failed to run database migrations")?;

        debug!("Database migrations completed");

        // Cleanup expired CVE cache entries
        if let Err(e) = crate::cve::cleanup_cache(&pool).await {
            debug!("Failed to cleanup CVE cache: {}", e);
        }

        // Cleanup expired Repology cache entries
        if let Err(e) = crate::repology::cleanup_cache(&pool).await {
            debug!("Failed to cleanup Repology cache: {}", e);
        }

        Ok(Self { pool })
    }

    /// Get a reference to the database connection pool
    ///
    /// This is used by other modules (like CVE) that need direct database access
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Get an update record for a specific package
    pub async fn get_update_record(&self, attr_path: &str) -> Result<Option<UpdateRecord>> {
        let row = sqlx::query(
            r#"
            SELECT attr_path, last_attempted, next_attempt, current_version,
                   proposed_version, latest_upstream_version
            FROM updates
            WHERE attr_path = ?
            "#,
        )
        .bind(attr_path)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("get update record for {attr_path}"))?;

        row.map(|row| {
            let last_attempted: Option<String> = row.try_get("last_attempted")?;
            let next_attempt: Option<String> = row.try_get("next_attempt")?;
            Ok(UpdateRecord {
                last_attempted: parse_rfc3339(last_attempted),
                next_attempt: parse_rfc3339(next_attempt),
                proposed_version: row.try_get("proposed_version")?,
            })
        })
        .transpose()
    }

    /// Check if a package should be checked for updates
    /// Returns true if:
    /// - No record exists (first check)
    /// - next_attempt is null
    /// - next_attempt is in the past
    pub async fn should_check_update(&self, attr_path: &str) -> Result<bool> {
        let Some(record) = self.get_update_record(attr_path).await? else {
            debug!("{}: No record found, should check", attr_path);
            return Ok(true);
        };

        let Some(next_attempt) = record.next_attempt else {
            debug!("{}: No next_attempt set, should check", attr_path);
            return Ok(true);
        };

        let now = Utc::now();
        let should_check = next_attempt <= now;
        if should_check {
            debug!(
                "{}: next_attempt ({}) is in the past, should check",
                attr_path, next_attempt
            );
        } else {
            debug!(
                "{}: next_attempt ({}) is in the future, skip",
                attr_path, next_attempt
            );
        }
        Ok(should_check)
    }

    /// Record that no update was available for a package
    /// Implements backoff: 2 days -> 4 days -> 6 days (max)
    pub async fn record_no_update(
        &self,
        attr_path: &str,
        current_version: &str,
        latest_upstream_version: &str,
    ) -> Result<()> {
        let now = Utc::now();
        let record = self.get_update_record(attr_path).await?;

        // Calculate next backoff: first failure -> FIRST, recent retry -> SECOND,
        // any older retry -> MAX.
        let backoff_days = match record.as_ref().and_then(|r| r.last_attempted) {
            None => FIRST_BACKOFF_DAYS,
            Some(last) => {
                let days_since = (now - last).num_days();
                if days_since <= FIRST_BACKOFF_DAYS {
                    SECOND_BACKOFF_DAYS
                } else {
                    MAX_BACKOFF_DAYS
                }
            },
        };

        let next_attempt = now + Duration::days(backoff_days);

        debug!(
            "{}: No update available, setting next_attempt to {} ({} days)",
            attr_path,
            next_attempt.to_rfc3339(),
            backoff_days
        );

        sqlx::query(
            r#"
            INSERT INTO updates (attr_path, last_attempted, next_attempt, current_version,
                                proposed_version, latest_upstream_version)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(attr_path) DO UPDATE SET
                last_attempted = excluded.last_attempted,
                next_attempt = excluded.next_attempt,
                current_version = excluded.current_version,
                latest_upstream_version = excluded.latest_upstream_version
            "#,
        )
        .bind(attr_path)
        .bind(now.to_rfc3339())
        .bind(next_attempt.to_rfc3339())
        .bind(current_version)
        .bind(record.and_then(|r| r.proposed_version)) // Keep existing proposed_version
        .bind(latest_upstream_version)
        .execute(&self.pool)
        .await
        .context("Failed to record no update")?;

        Ok(())
    }

    /// Record a successful update
    /// Resets backoff to 2 days
    ///
    /// Convenience wrapper around [`Self::record_successful_update_with_rebuild_count`]
    /// for callers that do not have a rebuild-impact figure to record.
    #[allow(dead_code)] // public API; retained for callers that don't need rebuild count
    pub async fn record_successful_update(
        &self,
        attr_path: &str,
        old_version: &str,
        new_version: &str,
    ) -> Result<()> {
        self.record_successful_update_with_rebuild_count(attr_path, old_version, new_version, None)
            .await
    }

    /// Record a successful update with optional rebuild count
    /// Resets backoff to 2 days
    pub async fn record_successful_update_with_rebuild_count(
        &self,
        attr_path: &str,
        old_version: &str,
        new_version: &str,
        rebuild_count: Option<usize>,
    ) -> Result<()> {
        let now = Utc::now();
        let next_attempt = now + Duration::days(2);

        if let Some(count) = rebuild_count {
            info!(
                "{}: Successful update from {} to {} ({} rebuilds), resetting next_attempt to 2 \
                 days",
                attr_path, old_version, new_version, count
            );
        } else {
            info!(
                "{}: Successful update from {} to {}, resetting next_attempt to 2 days",
                attr_path, old_version, new_version
            );
        }

        let rebuild_count_i64 = rebuild_count
            .map(i64::try_from)
            .transpose()
            .context("rebuild_count overflowed i64")?;

        sqlx::query(
            r#"
            INSERT INTO updates (attr_path, last_attempted, next_attempt, current_version,
                                proposed_version, latest_upstream_version, rebuild_count)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(attr_path) DO UPDATE SET
                last_attempted = excluded.last_attempted,
                next_attempt = excluded.next_attempt,
                current_version = excluded.current_version,
                proposed_version = NULL,
                latest_upstream_version = excluded.latest_upstream_version,
                rebuild_count = excluded.rebuild_count
            "#,
        )
        .bind(attr_path)
        .bind(now.to_rfc3339())
        .bind(next_attempt.to_rfc3339())
        .bind(new_version)
        .bind(new_version)
        .bind(rebuild_count_i64)
        .execute(&self.pool)
        .await
        .context("Failed to record successful update")?;

        Ok(())
    }

    /// Record PR information for a successful update
    pub async fn record_pr_info(
        &self,
        attr_path: &str,
        pr_url: &str,
        pr_number: i64,
    ) -> Result<()> {
        info!("{}: Recording PR #{} ({})", attr_path, pr_number, pr_url);

        sqlx::query(
            r#"
            UPDATE updates
            SET pr_url = ?, pr_number = ?
            WHERE attr_path = ?
            "#,
        )
        .bind(pr_url)
        .bind(pr_number)
        .bind(attr_path)
        .execute(&self.pool)
        .await
        .context("Failed to record PR info")?;

        Ok(())
    }

    /// Record a failed update attempt with error log
    pub async fn record_failed_update(
        &self,
        drv_path: &str,
        attr_path: &str,
        error_log: &str,
        old_version: Option<&str>,
        new_version: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now();

        debug!(
            "{}: Recording failed update attempt for drv {}",
            attr_path, drv_path
        );

        sqlx::query(
            r#"
            INSERT INTO update_logs (drv_path, attr_path, timestamp, status, error_log,
                                    old_version, new_version)
            VALUES (?, ?, ?, 'failed', ?, ?, ?)
            ON CONFLICT(drv_path) DO UPDATE SET
                timestamp = excluded.timestamp,
                error_log = excluded.error_log,
                old_version = excluded.old_version,
                new_version = excluded.new_version
            "#,
        )
        .bind(drv_path)
        .bind(attr_path)
        .bind(now.to_rfc3339())
        .bind(error_log)
        .bind(old_version)
        .bind(new_version)
        .execute(&self.pool)
        .await
        .context("Failed to record failed update")?;

        Ok(())
    }

    /// Get a log entry by drv_path (supports both full path and hash-name format)
    pub async fn get_log_by_drv(&self, drv_identifier: &str) -> Result<Option<UpdateLog>> {
        // Try exact match first
        let mut log = sqlx::query_as::<_, UpdateLog>(
            r#"
            SELECT drv_path, attr_path, timestamp, status, error_log, old_version, new_version
            FROM update_logs
            WHERE drv_path = ?
            "#,
        )
        .bind(drv_identifier)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("get log by drv {drv_identifier}"))?;

        // If no exact match and identifier doesn't start with /nix/store/,
        // try matching the end of drv_path
        if log.is_none() && !drv_identifier.starts_with("/nix/store/") {
            log = sqlx::query_as::<_, UpdateLog>(
                r#"
                SELECT drv_path, attr_path, timestamp, status, error_log, old_version, new_version
                FROM update_logs
                WHERE drv_path LIKE ?
                "#,
            )
            .bind(format!("%/{drv_identifier}"))
            .fetch_optional(&self.pool)
            .await
            .with_context(|| format!("get log by drv suffix {drv_identifier}"))?;
        }

        Ok(log)
    }

    /// Get all failed logs for an attr_path, ordered by most recent
    pub async fn get_all_failed_logs_by_attr(&self, attr_path: &str) -> Result<Vec<UpdateLog>> {
        let logs = sqlx::query_as::<_, UpdateLog>(
            r#"
            SELECT drv_path, attr_path, timestamp, status, error_log, old_version, new_version
            FROM update_logs
            WHERE attr_path = ?
            ORDER BY timestamp DESC
            "#,
        )
        .bind(attr_path)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("get failed logs for {attr_path}"))?;

        Ok(logs)
    }

    /// Get rebuild count for a specific package (most recent update)
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_rebuild_count(&self, attr_path: &str) -> Result<Option<i64>> {
        let count: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT rebuild_count
            FROM updates
            WHERE attr_path = ?
            "#,
        )
        .bind(attr_path)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("get rebuild count for {attr_path}"))?
        .flatten();

        Ok(count)
    }

    /// Get packages with highest rebuild impact
    ///
    /// Returns packages sorted by rebuild count (descending), including only
    /// those with non-null rebuild counts.
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_high_impact_packages(&self, limit: i64) -> Result<Vec<(String, i64)>> {
        let results: Vec<(String, i64)> = sqlx::query_as(
            r#"
            SELECT attr_path, rebuild_count
            FROM updates
            WHERE rebuild_count IS NOT NULL
            ORDER BY rebuild_count DESC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("get high impact packages")?;

        Ok(results)
    }

    /// Get average rebuild count across all packages
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_average_rebuild_count(&self) -> Result<Option<f64>> {
        let avg: Option<f64> = sqlx::query_scalar(
            r#"
            SELECT AVG(rebuild_count)
            FROM updates
            WHERE rebuild_count IS NOT NULL
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("get average rebuild count")?
        .flatten();

        Ok(avg)
    }

    /// Get rebuild count distribution (histogram buckets)
    ///
    /// Returns counts of packages in different rebuild impact ranges:
    /// - 0-10 rebuilds
    /// - 11-50 rebuilds
    /// - 51-100 rebuilds
    /// - 101+ rebuilds
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_rebuild_distribution(&self) -> Result<Vec<(String, i64)>> {
        // Build SQL with bucket boundaries from named constants. The
        // bucket-label strings in the inner and outer `CASE` must match,
        // so we render them once via `RebuildBucket::label`.
        let small_label = RebuildBucket::Small.label();
        let medium_label = RebuildBucket::Medium.label();
        let large_label = RebuildBucket::Large.label();
        let huge_label = RebuildBucket::Huge.label();
        let sql = format!(
            r#"
            SELECT
                CASE
                    WHEN rebuild_count <= {small_max} THEN '{small_label}'
                    WHEN rebuild_count <= {medium_max} THEN '{medium_label}'
                    WHEN rebuild_count <= {large_max} THEN '{large_label}'
                    ELSE '{huge_label}'
                END as range,
                COUNT(*) as count
            FROM updates
            WHERE rebuild_count IS NOT NULL
            GROUP BY range
            ORDER BY
                CASE range
                    WHEN '{small_label}' THEN 1
                    WHEN '{medium_label}' THEN 2
                    WHEN '{large_label}' THEN 3
                    ELSE 4
                END
            "#,
            small_max = REBUILD_BUCKET_SMALL_MAX,
            medium_max = REBUILD_BUCKET_MEDIUM_MAX,
            large_max = REBUILD_BUCKET_LARGE_MAX,
        );
        let results: Vec<(String, i64)> = sqlx::query_as(&sql)
            .fetch_all(&self.pool)
            .await
            .context("get rebuild distribution")?;

        Ok(results)
    }

    /// Get packages with rebuild counts in a specific range
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_packages_by_rebuild_range(
        &self,
        min: i64,
        max: i64,
    ) -> Result<Vec<(String, i64, String)>> {
        let results: Vec<(String, i64, String)> = sqlx::query_as(
            r#"
            SELECT attr_path, rebuild_count, current_version
            FROM updates
            WHERE rebuild_count IS NOT NULL
                AND rebuild_count >= ?
                AND rebuild_count <= ?
            ORDER BY rebuild_count DESC
            "#,
        )
        .bind(min)
        .bind(max)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("get packages by rebuild range [{min}, {max}]"))?;

        Ok(results)
    }

    /// Get total number of packages with rebuild count data
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_packages_with_rebuild_data_count(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM updates
            WHERE rebuild_count IS NOT NULL
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .context("count packages with rebuild data")?;

        Ok(count)
    }
}
