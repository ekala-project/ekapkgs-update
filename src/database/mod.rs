use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use tracing::{debug, info};

/// Represents a package update record in the database
#[derive(Debug, Clone)]
pub struct UpdateRecord {
    pub _attr_path: String,
    pub last_attempted: Option<DateTime<Utc>>,
    pub next_attempt: Option<DateTime<Utc>>,
    pub _current_version: Option<String>,
    pub proposed_version: Option<String>,
    pub _latest_upstream_version: Option<String>,
}

/// Represents a failed update log entry in the database
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UpdateLog {
    pub drv_path: String,
    pub attr_path: String,
    pub timestamp: String,
    pub status: String,
    pub error_log: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
}

impl UpdateLog {
    /// Parse the timestamp string as a DateTime<Utc>
    pub fn timestamp_as_datetime(&self) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&self.timestamp)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now())
    }
}

/// Database connection wrapper for tracking package updates
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Initialize the database connection and create tables if needed
    pub async fn new(db_path: &str) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = Path::new(db_path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("Failed to create database directory")?;
        }

        // Create connection options
        let options = SqliteConnectOptions::from_str(db_path)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        // Connect to database
        let pool = SqlitePool::connect_with(options)
            .await
            .context("Failed to connect to database")?;

        info!("Connected to database at {}", db_path);

        // Run migrations
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("Failed to run database migrations")?;

        debug!("Database migrations completed");

        Ok(Self { pool })
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
        .await?;

        match row {
            Some(row) => {
                let last_attempted: Option<String> = row.try_get("last_attempted")?;
                let next_attempt: Option<String> = row.try_get("next_attempt")?;

                Ok(Some(UpdateRecord {
                    _attr_path: row.try_get("attr_path")?,
                    last_attempted: last_attempted
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    next_attempt: next_attempt
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    _current_version: row.try_get("current_version")?,
                    proposed_version: row.try_get("proposed_version")?,
                    _latest_upstream_version: row.try_get("latest_upstream_version")?,
                }))
            },
            None => Ok(None),
        }
    }

    /// Check if a package should be checked for updates
    /// Returns true if:
    /// - No record exists (first check)
    /// - next_attempt is null
    /// - next_attempt is in the past
    pub async fn should_check_update(&self, attr_path: &str) -> Result<bool> {
        let record = self.get_update_record(attr_path).await?;

        match record {
            None => {
                debug!("{}: No record found, should check", attr_path);
                Ok(true)
            },
            Some(record) => match record.next_attempt {
                None => {
                    debug!("{}: No next_attempt set, should check", attr_path);
                    Ok(true)
                },
                Some(next_attempt) => {
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
                },
            },
        }
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

        // Calculate next backoff
        let backoff_days = match record {
            None => 2, // First failed check: 2 days
            Some(ref rec) => {
                // Calculate days since last attempt
                match rec.last_attempted {
                    None => 2,
                    Some(last) => {
                        let days_since = (now - last).num_days();
                        // Increment backoff: 2 -> 4 -> 6 (max)
                        match days_since {
                            0..=2 => 4,
                            3..=4 => 6,
                            _ => 6, // Max at 6 days
                        }
                    },
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
    pub async fn record_successful_update(
        &self,
        attr_path: &str,
        old_version: &str,
        new_version: &str,
    ) -> Result<()> {
        let now = Utc::now();
        let next_attempt = now + Duration::days(2);

        info!(
            "{}: Successful update from {} to {}, resetting next_attempt to 2 days",
            attr_path, old_version, new_version
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
                proposed_version = NULL,
                latest_upstream_version = excluded.latest_upstream_version
            "#,
        )
        .bind(attr_path)
        .bind(now.to_rfc3339())
        .bind(next_attempt.to_rfc3339())
        .bind(new_version)
        .bind(new_version)
        .execute(&self.pool)
        .await
        .context("Failed to record successful update")?;

        Ok(())
    }

    /// Record a proposed update (update was made but not yet merged)
    pub async fn _record_proposed_update(
        &self,
        attr_path: &str,
        current_version: &str,
        proposed_version: &str,
        latest_upstream_version: &str,
    ) -> Result<()> {
        let now = Utc::now();
        let next_attempt = now + Duration::days(2);

        debug!(
            "{}: Recording proposed update from {} to {}",
            attr_path, current_version, proposed_version
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
                proposed_version = excluded.proposed_version,
                latest_upstream_version = excluded.latest_upstream_version
            "#,
        )
        .bind(attr_path)
        .bind(now.to_rfc3339())
        .bind(next_attempt.to_rfc3339())
        .bind(current_version)
        .bind(proposed_version)
        .bind(latest_upstream_version)
        .execute(&self.pool)
        .await
        .context("Failed to record proposed update")?;

        Ok(())
    }

    /// Get statistics about tracked packages
    pub async fn _get_statistics(&self) -> Result<_DatabaseStatistics> {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM updates")
            .fetch_one(&self.pool)
            .await?;

        let with_proposed: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM updates WHERE proposed_version IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;

        let in_backoff: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM updates WHERE next_attempt > datetime('now')")
                .fetch_one(&self.pool)
                .await?;

        Ok(_DatabaseStatistics {
            total_packages: total,
            packages_with_proposed_updates: with_proposed,
            packages_in_backoff: in_backoff,
        })
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
        .await?;

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
            .bind(format!("%/{}", drv_identifier))
            .fetch_optional(&self.pool)
            .await?;
        }

        Ok(log)
    }

    /// Get the most recent failed log for an attr_path
    pub async fn _get_latest_failed_log_by_attr(
        &self,
        attr_path: &str,
    ) -> Result<Option<UpdateLog>> {
        let log = sqlx::query_as::<_, UpdateLog>(
            r#"
            SELECT drv_path, attr_path, timestamp, status, error_log, old_version, new_version
            FROM update_logs
            WHERE attr_path = ?
            ORDER BY timestamp DESC
            LIMIT 1
            "#,
        )
        .bind(attr_path)
        .fetch_optional(&self.pool)
        .await?;

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
        .await?;

        Ok(logs)
    }
}

#[derive(Debug)]
pub struct _DatabaseStatistics {
    pub total_packages: i64,
    pub packages_with_proposed_updates: i64,
    pub packages_in_backoff: i64,
}
