//! Session and phase tracking for supervised updates
//!
//! This module provides functionality to track update sessions (entire runs)
//! and phases (individual steps within updates) for debugging, the web
//! dashboard and LLM-assisted remediation.
//!
//! Session counters are updated live while a session runs:
//! `packages_attempted` is bumped when an update starts and exactly one of
//! `packages_succeeded` / `packages_failed` / `packages_skipped` when it ends,
//! so "in flight" = attempted − (succeeded + failed + skipped).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use uuid::Uuid;

use super::{Database, parse_rfc3339};
use crate::commands::update::errors::UpdateError;
use crate::commands::update::types::UpdatePhase;

/// Status of an update session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which command created a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    /// `ekapkgs-update run` (batch checker)
    Run,
    /// `ekapkgs-update watch` (one session per poll cycle that dispatched updates)
    Watch,
}

impl SessionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Watch => "watch",
        }
    }
}

/// Live session counter to increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCounter {
    Attempted,
    Succeeded,
    Failed,
    Skipped,
}

impl SessionCounter {
    fn column(self) -> &'static str {
        match self {
            Self::Attempted => "packages_attempted",
            Self::Succeeded => "packages_succeeded",
            Self::Failed => "packages_failed",
            Self::Skipped => "packages_skipped",
        }
    }
}

/// Information about an update session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSession {
    pub id: String,
    pub mode: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub status: SessionStatus,
    pub packages_evaluated: usize,
    pub packages_backoff: usize,
    pub packages_attempted: usize,
    pub packages_succeeded: usize,
    pub packages_failed: usize,
    pub packages_skipped: usize,
    pub config_json: Option<String>,
}

/// Information about a single phase execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseRecord {
    pub id: i64,
    pub session_id: String,
    pub repo_name: String,
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

const SESSION_COLUMNS: &str = "id, mode, started_at, completed_at, updated_at, status, \
                               packages_evaluated, packages_backoff, packages_attempted, \
                               packages_succeeded, packages_failed, packages_skipped, config_json";

const PHASE_COLUMNS: &str = "id, session_id, repo_name, attr_path, phase, started_at, \
                             completed_at, duration_ms, status, error_type, error_details, \
                             artifacts_path";

fn count(row: &SqliteRow, column: &str) -> usize {
    row.get::<Option<i64>, _>(column)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
}

fn session_from_row(row: &SqliteRow) -> Option<UpdateSession> {
    Some(UpdateSession {
        id: row.get("id"),
        mode: row.get("mode"),
        started_at: parse_rfc3339(row.get("started_at"))?,
        completed_at: parse_rfc3339(row.get("completed_at")),
        updated_at: parse_rfc3339(row.get("updated_at")),
        status: SessionStatus::parse(row.get("status"))?,
        packages_evaluated: count(row, "packages_evaluated"),
        packages_backoff: count(row, "packages_backoff"),
        packages_attempted: count(row, "packages_attempted"),
        packages_succeeded: count(row, "packages_succeeded"),
        packages_failed: count(row, "packages_failed"),
        packages_skipped: count(row, "packages_skipped"),
        config_json: row.get("config_json"),
    })
}

fn phase_from_row(row: &SqliteRow) -> Option<PhaseRecord> {
    Some(PhaseRecord {
        id: row.get("id"),
        session_id: row.get("session_id"),
        repo_name: row.get("repo_name"),
        attr_path: row.get("attr_path"),
        phase: row.get("phase"),
        started_at: parse_rfc3339(row.get("started_at"))?,
        completed_at: parse_rfc3339(row.get("completed_at")),
        duration_ms: row.get("duration_ms"),
        status: row.get("status"),
        error_type: row.get("error_type"),
        error_details: row.get("error_details"),
        artifacts_path: row.get("artifacts_path"),
    })
}

fn duration_ms(duration: std::time::Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl Database {
    /// Create a new update session
    ///
    /// Returns the session ID (UUID) for tracking this run.
    pub async fn create_session(
        &self,
        mode: SessionMode,
        config_json: Option<String>,
    ) -> anyhow::Result<String> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO update_sessions (id, mode, started_at, updated_at, status, config_json)
             VALUES (?, ?, ?, ?, 'running', ?)",
        )
        .bind(&id)
        .bind(mode.as_str())
        .bind(&now)
        .bind(&now)
        .bind(config_json)
        .execute(&self.pool)
        .await?;

        Ok(id)
    }

    /// Increment one live session counter (and the heartbeat).
    pub async fn increment_session_counter(
        &self,
        session_id: &str,
        counter: SessionCounter,
    ) -> anyhow::Result<()> {
        let column = counter.column();
        sqlx::query(&format!(
            "UPDATE update_sessions
             SET {column} = COALESCE({column}, 0) + 1, updated_at = ?
             WHERE id = ?"
        ))
        .bind(Utc::now().to_rfc3339())
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record checker progress (packages evaluated / skipped by backoff).
    pub async fn set_session_progress(
        &self,
        session_id: &str,
        packages_evaluated: usize,
        packages_backoff: usize,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE update_sessions
             SET packages_evaluated = ?, packages_backoff = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(to_i64(packages_evaluated))
        .bind(to_i64(packages_backoff))
        .bind(Utc::now().to_rfc3339())
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update session status
    #[allow(dead_code)] // public API (tooling / web)
    pub async fn update_session_status(
        &self,
        session_id: &str,
        status: SessionStatus,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE update_sessions
             SET status = ?, completed_at = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(&now)
        .bind(&now)
        .bind(session_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Finalize a session: set its final status, completion time and the
    /// checker totals. Outcome counters are maintained live and left as-is.
    pub async fn finalize_session(
        &self,
        session_id: &str,
        status: SessionStatus,
        packages_evaluated: usize,
        packages_backoff: usize,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE update_sessions
             SET status = ?,
                 completed_at = ?,
                 updated_at = ?,
                 packages_evaluated = ?,
                 packages_backoff = ?
             WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(&now)
        .bind(&now)
        .bind(to_i64(packages_evaluated))
        .bind(to_i64(packages_backoff))
        .bind(session_id)
        .execute(&self.pool)
        .await?;

        // Phases left running belong to tasks that never finished (e.g. a
        // panic); don't leave them spinning in the dashboard forever.
        sqlx::query(
            "UPDATE update_phases SET status = 'skipped', completed_at = ?
             WHERE session_id = ? AND status = 'running'",
        )
        .bind(&now)
        .bind(session_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Record the start of a phase
    ///
    /// Returns the phase ID for later updates.
    pub async fn record_phase_start(
        &self,
        session_id: &str,
        repo_name: &str,
        attr_path: &str,
        phase: UpdatePhase,
    ) -> anyhow::Result<i64> {
        let now = Utc::now().to_rfc3339();

        let result = sqlx::query(
            "INSERT INTO update_phases
             (session_id, repo_name, attr_path, phase, started_at, status)
             VALUES (?, ?, ?, ?, ?, 'running')",
        )
        .bind(session_id)
        .bind(repo_name)
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
        self.finish_phase(phase_id, "success", duration).await
    }

    /// Record a phase that started but turned out to be not applicable.
    pub async fn record_phase_skipped(
        &self,
        phase_id: i64,
        duration: std::time::Duration,
    ) -> anyhow::Result<()> {
        self.finish_phase(phase_id, "skipped", duration).await
    }

    async fn finish_phase(
        &self,
        phase_id: i64,
        status: &str,
        duration: std::time::Duration,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE update_phases
             SET status = ?, completed_at = ?, duration_ms = ?
             WHERE id = ?",
        )
        .bind(status)
        .bind(Utc::now().to_rfc3339())
        .bind(duration_ms(duration))
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
        .bind(duration_ms(duration))
        .bind(error_type)
        .bind(&error_json)
        .bind(artifacts_path)
        .bind(phase_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Attach a preserved-artifacts path to the most recent failed phase of
    /// a package in a session.
    pub async fn set_failed_phase_artifacts(
        &self,
        session_id: &str,
        repo_name: &str,
        attr_path: &str,
        artifacts_path: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE update_phases SET artifacts_path = ?
             WHERE id = (
                 SELECT MAX(id) FROM update_phases
                 WHERE session_id = ? AND repo_name = ? AND attr_path = ? AND status = 'failed'
             )",
        )
        .bind(artifacts_path)
        .bind(session_id)
        .bind(repo_name)
        .bind(attr_path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get all phases for a specific attribute path (any repo)
    pub async fn get_phases_by_attr(&self, attr_path: &str) -> anyhow::Result<Vec<PhaseRecord>> {
        let rows = sqlx::query(&format!(
            "SELECT {PHASE_COLUMNS} FROM update_phases
             WHERE attr_path = ?
             ORDER BY started_at DESC, id DESC"
        ))
        .bind(attr_path)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().filter_map(phase_from_row).collect())
    }

    /// Get session by ID
    #[allow(dead_code)] // public API (tooling / web)
    pub async fn get_session(&self, session_id: &str) -> anyhow::Result<Option<UpdateSession>> {
        let row = sqlx::query(&format!(
            "SELECT {SESSION_COLUMNS} FROM update_sessions WHERE id = ?"
        ))
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.as_ref().and_then(session_from_row))
    }

    /// Get sessions by status
    #[allow(dead_code)] // public API (tooling / web)
    pub async fn get_sessions_by_status(
        &self,
        status: SessionStatus,
    ) -> anyhow::Result<Vec<UpdateSession>> {
        let rows = sqlx::query(&format!(
            "SELECT {SESSION_COLUMNS} FROM update_sessions
             WHERE status = ?
             ORDER BY started_at DESC"
        ))
        .bind(status.as_str())
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().filter_map(session_from_row).collect())
    }

    /// Get recent sessions (up to limit)
    pub async fn get_recent_sessions(&self, limit: usize) -> anyhow::Result<Vec<UpdateSession>> {
        let rows = sqlx::query(&format!(
            "SELECT {SESSION_COLUMNS} FROM update_sessions
             ORDER BY started_at DESC
             LIMIT ?"
        ))
        .bind(to_i64(limit))
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().filter_map(session_from_row).collect())
    }

    /// Get currently running phases for a session
    pub async fn get_running_phases(&self, session_id: &str) -> anyhow::Result<Vec<PhaseRecord>> {
        let rows = sqlx::query(&format!(
            "SELECT {PHASE_COLUMNS} FROM update_phases
             WHERE session_id = ? AND status = 'running'
             ORDER BY started_at DESC"
        ))
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().filter_map(phase_from_row).collect())
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
        let mut query = format!("SELECT {PHASE_COLUMNS} FROM update_phases WHERE 1=1");

        if error_type.is_some() {
            query.push_str(" AND error_type = ?");
        }
        if phase.is_some() {
            query.push_str(" AND phase = ?");
        }
        if status.is_some() {
            query.push_str(" AND status = ?");
        }
        if package_pattern.is_some() {
            query.push_str(" AND attr_path LIKE ?");
        }
        if since.is_some() {
            query.push_str(" AND started_at >= ?");
        }

        query.push_str(" ORDER BY started_at DESC");

        if let Some(lim) = limit {
            query.push_str(&format!(" LIMIT {lim}"));
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
            sqlx_query = sqlx_query.bind(format!("%{pp}%"));
        }
        if let Some(s) = since {
            sqlx_query = sqlx_query.bind(s.to_rfc3339());
        }

        let rows = sqlx_query.fetch_all(&self.pool).await?;

        Ok(rows.iter().filter_map(phase_from_row).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> Database {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        Database::from(pool)
    }

    #[tokio::test]
    async fn live_counters_and_finalize() {
        let db = setup_db().await;
        let id = db.create_session(SessionMode::Watch, None).await.unwrap();

        for counter in [
            SessionCounter::Attempted,
            SessionCounter::Attempted,
            SessionCounter::Succeeded,
            SessionCounter::Failed,
        ] {
            db.increment_session_counter(&id, counter).await.unwrap();
        }
        let phase = db
            .record_phase_start(&id, "r", "hello", UpdatePhase::Build)
            .await
            .unwrap();

        let s = db.get_session(&id).await.unwrap().unwrap();
        assert_eq!(s.mode, "watch");
        assert_eq!(s.status, SessionStatus::Running);
        assert_eq!(s.packages_attempted, 2);
        assert_eq!(s.packages_succeeded, 1);
        assert_eq!(s.packages_failed, 1);

        db.finalize_session(&id, SessionStatus::Failed, 10, 3)
            .await
            .unwrap();
        let s = db.get_session(&id).await.unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::Failed);
        assert_eq!(s.packages_attempted, 2);
        assert_eq!(s.packages_evaluated, 10);
        assert_eq!(s.packages_backoff, 3);
        assert!(s.completed_at.is_some());

        // Dangling running phases are closed out on finalize.
        let phases = db.get_phases_by_attr("hello").await.unwrap();
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].id, phase);
        assert_eq!(phases[0].repo_name, "r");
        assert_eq!(phases[0].status, "skipped");
    }

    #[tokio::test]
    async fn failed_phase_artifacts() {
        let db = setup_db().await;
        let id = db.create_session(SessionMode::Run, None).await.unwrap();
        let phase = db
            .record_phase_start(&id, "", "hello", UpdatePhase::Build)
            .await
            .unwrap();
        let err = UpdateError::BuildError {
            phase: UpdatePhase::Build,
            exit_code: 1,
            stderr: "boom".into(),
            suspected_cause: None,
        };
        db.record_phase_failure(phase, std::time::Duration::from_millis(5), &err, None)
            .await
            .unwrap();
        db.set_failed_phase_artifacts(&id, "", "hello", "/tmp/artifacts")
            .await
            .unwrap();

        let phases = db.get_phases_by_attr("hello").await.unwrap();
        assert_eq!(phases[0].status, "failed");
        assert_eq!(phases[0].error_type.as_deref(), Some("BuildError"));
        assert_eq!(phases[0].artifacts_path.as_deref(), Some("/tmp/artifacts"));
    }
}
