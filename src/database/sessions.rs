//! Session and phase tracking for supervised updates
//!
//! This module provides functionality to track update sessions (entire runs)
//! and phases (individual steps within updates) for debugging and LLM-assisted
//! remediation.

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::commands::update::errors::UpdateError;
use crate::commands::update::types::UpdatePhase;

use super::Database;

/// Status of an update session
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Session is currently running
    Running,
    /// Session completed successfully
    Completed,
    /// Session failed
    Failed,
    /// Session was cancelled
    Cancelled,
}

impl SessionStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// Information about an update session
#[derive(Debug, Clone)]
pub struct UpdateSession {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: SessionStatus,
    pub packages_attempted: usize,
    pub packages_succeeded: usize,
    pub packages_failed: usize,
    pub packages_skipped: usize,
    pub config_json: Option<String>,
}

/// Information about a single phase execution
#[derive(Debug, Clone)]
pub struct PhaseRecord {
    pub id: i64,
    pub session_id: String,
    pub attr_path: String,
    pub phase: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub status: String,
    pub error_type: Option<String>,
    pub error_details: Option<String>,
    pub artifacts_path: Option<String>,
}

impl Database {
    /// Create a new update session
    ///
    /// Returns the session ID (UUID) for tracking this run.
    pub async fn create_session(&self, config_json: Option<String>) -> anyhow::Result<String> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO update_sessions (id, started_at, status, config_json)
             VALUES (?, ?, 'running', ?)",
        )
        .bind(&id)
        .bind(&now)
        .bind(config_json)
        .execute(&self.pool)
        .await?;

        Ok(id)
    }

    /// Update session status
    pub async fn update_session_status(
        &self,
        session_id: &str,
        status: SessionStatus,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE update_sessions
             SET status = ?, completed_at = ?
             WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(&now)
        .bind(session_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Increment session counters
    pub async fn increment_session_counter(
        &self,
        session_id: &str,
        counter: &str,
    ) -> anyhow::Result<()> {
        let query = match counter {
            "attempted" => "UPDATE update_sessions SET packages_attempted = packages_attempted + 1 WHERE id = ?",
            "succeeded" => "UPDATE update_sessions SET packages_succeeded = packages_succeeded + 1 WHERE id = ?",
            "failed" => "UPDATE update_sessions SET packages_failed = packages_failed + 1 WHERE id = ?",
            "skipped" => "UPDATE update_sessions SET packages_skipped = packages_skipped + 1 WHERE id = ?",
            _ => return Err(anyhow::anyhow!("Invalid counter: {}", counter)),
        };

        sqlx::query(query).bind(session_id).execute(&self.pool).await?;

        Ok(())
    }

    /// Record the start of a phase
    ///
    /// Returns the phase ID for later updates.
    pub async fn record_phase_start(
        &self,
        session_id: &str,
        attr_path: &str,
        phase: UpdatePhase,
    ) -> anyhow::Result<i64> {
        let now = Utc::now().to_rfc3339();

        let result = sqlx::query(
            "INSERT INTO update_phases
             (session_id, attr_path, phase, started_at, status)
             VALUES (?, ?, ?, ?, 'running')",
        )
        .bind(session_id)
        .bind(attr_path)
        .bind(phase.as_str())
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Record successful completion of a phase
    pub async fn record_phase_success(
        &self,
        phase_id: i64,
        duration: std::time::Duration,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        let duration_ms = duration.as_millis() as i64;

        sqlx::query(
            "UPDATE update_phases
             SET status = 'success',
                 completed_at = ?,
                 duration_ms = ?
             WHERE id = ?",
        )
        .bind(&now)
        .bind(duration_ms)
        .bind(phase_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Record failure of a phase with error details
    pub async fn record_phase_failure(
        &self,
        phase_id: i64,
        duration: std::time::Duration,
        error: &UpdateError,
        artifacts_path: Option<String>,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        let duration_ms = duration.as_millis() as i64;
        let error_type = error.error_type_name();
        let error_json = serde_json::to_string(error)?;

        sqlx::query(
            "UPDATE update_phases
             SET status = 'failed',
                 completed_at = ?,
                 duration_ms = ?,
                 error_type = ?,
                 error_details = ?,
                 artifacts_path = ?
             WHERE id = ?",
        )
        .bind(&now)
        .bind(duration_ms)
        .bind(error_type)
        .bind(&error_json)
        .bind(artifacts_path)
        .bind(phase_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get all phases for a specific attribute path
    pub async fn get_phases_by_attr(&self, attr_path: &str) -> anyhow::Result<Vec<PhaseRecord>> {
        let rows = sqlx::query(
            "SELECT id, session_id, attr_path, phase, started_at, completed_at,
                    duration_ms, status, error_type, error_details, artifacts_path
             FROM update_phases
             WHERE attr_path = ?
             ORDER BY started_at DESC",
        )
        .bind(attr_path)
        .fetch_all(&self.pool)
        .await?;

        let phases = rows
            .into_iter()
            .filter_map(|row| {
                Some(PhaseRecord {
                    id: row.get("id"),
                    session_id: row.get("session_id"),
                    attr_path: row.get("attr_path"),
                    phase: row.get("phase"),
                    started_at: DateTime::parse_from_rfc3339(row.get("started_at"))
                        .ok()?
                        .with_timezone(&Utc),
                    completed_at: row
                        .get::<Option<String>, _>("completed_at")
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    duration_ms: row.get("duration_ms"),
                    status: row.get("status"),
                    error_type: row.get("error_type"),
                    error_details: row.get("error_details"),
                    artifacts_path: row.get("artifacts_path"),
                })
            })
            .collect();

        Ok(phases)
    }

    /// Get session by ID
    pub async fn get_session(&self, session_id: &str) -> anyhow::Result<Option<UpdateSession>> {
        let row = sqlx::query(
            "SELECT id, started_at, completed_at, status, packages_attempted,
                    packages_succeeded, packages_failed, packages_skipped, config_json
             FROM update_sessions
             WHERE id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.and_then(|row| {
            Some(UpdateSession {
                id: row.get("id"),
                started_at: DateTime::parse_from_rfc3339(row.get("started_at"))
                    .ok()?
                    .with_timezone(&Utc),
                completed_at: row
                    .get::<Option<String>, _>("completed_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                status: SessionStatus::from_str(row.get("status"))?,
                packages_attempted: row.get::<i64, _>("packages_attempted") as usize,
                packages_succeeded: row.get::<i64, _>("packages_succeeded") as usize,
                packages_failed: row.get::<i64, _>("packages_failed") as usize,
                packages_skipped: row.get::<i64, _>("packages_skipped") as usize,
                config_json: row.get("config_json"),
            })
        }))
    }

    /// Get sessions by status
    pub async fn get_sessions_by_status(
        &self,
        status: SessionStatus,
    ) -> anyhow::Result<Vec<UpdateSession>> {
        let rows = sqlx::query(
            "SELECT id, started_at, completed_at, status, packages_attempted,
                    packages_succeeded, packages_failed, packages_skipped, config_json
             FROM update_sessions
             WHERE status = ?
             ORDER BY started_at DESC",
        )
        .bind(status.as_str())
        .fetch_all(&self.pool)
        .await?;

        let sessions = rows
            .into_iter()
            .filter_map(|row| {
                Some(UpdateSession {
                    id: row.get("id"),
                    started_at: DateTime::parse_from_rfc3339(row.get("started_at"))
                        .ok()?
                        .with_timezone(&Utc),
                    completed_at: row
                        .get::<Option<String>, _>("completed_at")
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    status: SessionStatus::from_str(row.get("status"))?,
                    packages_attempted: row.get::<i64, _>("packages_attempted") as usize,
                    packages_succeeded: row.get::<i64, _>("packages_succeeded") as usize,
                    packages_failed: row.get::<i64, _>("packages_failed") as usize,
                    packages_skipped: row.get::<i64, _>("packages_skipped") as usize,
                    config_json: row.get("config_json"),
                })
            })
            .collect();

        Ok(sessions)
    }

    /// Get recent sessions (up to limit)
    pub async fn get_recent_sessions(&self, limit: usize) -> anyhow::Result<Vec<UpdateSession>> {
        let rows = sqlx::query(
            "SELECT id, started_at, completed_at, status, packages_attempted,
                    packages_succeeded, packages_failed, packages_skipped, config_json
             FROM update_sessions
             ORDER BY started_at DESC
             LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let sessions = rows
            .into_iter()
            .filter_map(|row| {
                Some(UpdateSession {
                    id: row.get("id"),
                    started_at: DateTime::parse_from_rfc3339(row.get("started_at"))
                        .ok()?
                        .with_timezone(&Utc),
                    completed_at: row
                        .get::<Option<String>, _>("completed_at")
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    status: SessionStatus::from_str(row.get("status"))?,
                    packages_attempted: row.get::<i64, _>("packages_attempted") as usize,
                    packages_succeeded: row.get::<i64, _>("packages_succeeded") as usize,
                    packages_failed: row.get::<i64, _>("packages_failed") as usize,
                    packages_skipped: row.get::<i64, _>("packages_skipped") as usize,
                    config_json: row.get("config_json"),
                })
            })
            .collect();

        Ok(sessions)
    }

    /// Get currently running phases for a session
    pub async fn get_running_phases(&self, session_id: &str) -> anyhow::Result<Vec<PhaseRecord>> {
        let rows = sqlx::query(
            "SELECT id, session_id, attr_path, phase, started_at, completed_at,
                    duration_ms, status, error_type, error_details, artifacts_path
             FROM update_phases
             WHERE session_id = ? AND status = 'running'
             ORDER BY started_at DESC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        let phases = rows
            .into_iter()
            .filter_map(|row| {
                Some(PhaseRecord {
                    id: row.get("id"),
                    session_id: row.get("session_id"),
                    attr_path: row.get("attr_path"),
                    phase: row.get("phase"),
                    started_at: DateTime::parse_from_rfc3339(row.get("started_at"))
                        .ok()?
                        .with_timezone(&Utc),
                    completed_at: row
                        .get::<Option<String>, _>("completed_at")
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    duration_ms: row.get("duration_ms"),
                    status: row.get("status"),
                    error_type: row.get("error_type"),
                    error_details: row.get("error_details"),
                    artifacts_path: row.get("artifacts_path"),
                })
            })
            .collect();

        Ok(phases)
    }

    /// Query phases with flexible filtering
    pub async fn query_phases(
        &self,
        error_type: Option<&str>,
        phase: Option<&str>,
        status: Option<&str>,
        package_pattern: Option<&str>,
        since: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<PhaseRecord>> {
        // Build dynamic query
        let mut query = String::from(
            "SELECT id, session_id, attr_path, phase, started_at, completed_at,
                    duration_ms, status, error_type, error_details, artifacts_path
             FROM update_phases
             WHERE 1=1",
        );
        let mut conditions = Vec::new();

        if error_type.is_some() {
            query.push_str(" AND error_type = ?");
            conditions.push(error_type);
        }
        if phase.is_some() {
            query.push_str(" AND phase = ?");
            conditions.push(phase);
        }
        if status.is_some() {
            query.push_str(" AND status = ?");
            conditions.push(status);
        }
        if package_pattern.is_some() {
            query.push_str(" AND attr_path LIKE ?");
        }
        if since.is_some() {
            query.push_str(" AND started_at >= ?");
        }

        query.push_str(" ORDER BY started_at DESC");

        if let Some(lim) = limit {
            query.push_str(&format!(" LIMIT {}", lim));
        }

        let mut sqlx_query = sqlx::query(&query);

        // Bind parameters in order
        if let Some(et) = error_type {
            sqlx_query = sqlx_query.bind(et);
        }
        if let Some(p) = phase {
            sqlx_query = sqlx_query.bind(p);
        }
        if let Some(s) = status {
            sqlx_query = sqlx_query.bind(s);
        }
        if let Some(pp) = package_pattern {
            sqlx_query = sqlx_query.bind(format!("%{}%", pp));
        }
        if let Some(s) = since {
            sqlx_query = sqlx_query.bind(s.to_rfc3339());
        }

        let rows = sqlx_query.fetch_all(&self.pool).await?;

        let phases = rows
            .into_iter()
            .filter_map(|row| {
                Some(PhaseRecord {
                    id: row.get("id"),
                    session_id: row.get("session_id"),
                    attr_path: row.get("attr_path"),
                    phase: row.get("phase"),
                    started_at: DateTime::parse_from_rfc3339(row.get("started_at"))
                        .ok()?
                        .with_timezone(&Utc),
                    completed_at: row
                        .get::<Option<String>, _>("completed_at")
                        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc)),
                    duration_ms: row.get("duration_ms"),
                    status: row.get("status"),
                    error_type: row.get("error_type"),
                    error_details: row.get("error_details"),
                    artifacts_path: row.get("artifacts_path"),
                })
            })
            .collect();

        Ok(phases)
    }
}
