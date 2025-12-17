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
    pub attr_path: String,
    pub last_attempted: Option<DateTime<Utc>>,
    pub next_attempt: Option<DateTime<Utc>>,
    pub current_version: Option<String>,
    pub proposed_version: Option<String>,
    pub latest_upstream_version: Option<String>,
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

        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS updates (
                attr_path TEXT PRIMARY KEY,
                last_attempted TEXT,
                next_attempt TEXT,
                current_version TEXT,
                proposed_version TEXT,
                latest_upstream_version TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .context("Failed to create updates table")?;

        debug!("Database tables initialized");

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
                    attr_path: row.try_get("attr_path")?,
                    last_attempted: last_attempted
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    next_attempt: next_attempt
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    current_version: row.try_get("current_version")?,
                    proposed_version: row.try_get("proposed_version")?,
                    latest_upstream_version: row.try_get("latest_upstream_version")?,
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
    pub async fn record_proposed_update(
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
    pub async fn get_statistics(&self) -> Result<DatabaseStatistics> {
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

        Ok(DatabaseStatistics {
            total_packages: total,
            packages_with_proposed_updates: with_proposed,
            packages_in_backoff: in_backoff,
        })
    }
}

#[derive(Debug)]
pub struct DatabaseStatistics {
    pub total_packages: i64,
    pub packages_with_proposed_updates: i64,
    pub packages_in_backoff: i64,
}
