//! `update_sessions` and `update_phases`.

use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

/// One row of `update_sessions` (without `config_json`).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SessionRow {
    pub id: String,
    pub mode: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    /// Heartbeat, touched whenever a counter changes.
    pub updated_at: Option<String>,
    pub packages_attempted: i64,
    pub packages_succeeded: i64,
    pub packages_failed: i64,
    pub packages_skipped: i64,
    pub packages_evaluated: i64,
    pub packages_backoff: i64,
}

impl SessionRow {
    /// Updates started but not yet finished.
    pub fn in_flight(&self) -> i64 {
        (self.packages_attempted
            - self.packages_succeeded
            - self.packages_failed
            - self.packages_skipped)
            .max(0)
    }
}

/// One row of `update_phases`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct PhaseRow {
    pub id: i64,
    pub session_id: String,
    pub repo_name: String,
    pub attr_path: String,
    pub phase: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub status: String,
    pub error_type: Option<String>,
    pub error_details: Option<String>,
    pub artifacts_path: Option<String>,
}

/// Filters for the session list.
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    pub status: Option<String>,
    pub mode: Option<String>,
    /// Include finished watch sessions that attempted nothing (legacy rows).
    pub show_empty: bool,
}

/// ?1 status, ?2 mode, ?3 show-empty.
const FILTER_SQL: &str = " WHERE (?1 IS NULL OR s.status = ?1) AND (?2 IS NULL OR s.mode = ?2) \
     AND (?3 = 1 OR NOT (s.mode = 'watch' AND s.status != 'running' \
                          AND COALESCE(s.packages_attempted, 0) = 0))";

/// One page of sessions, newest first.
pub async fn list(
    pool: &SqlitePool,
    filter: &SessionFilter,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<SessionRow>> {
    let sql = format!(
        "SELECT {} FROM update_sessions s{FILTER_SQL} ORDER BY s.started_at DESC \
         LIMIT ?4 OFFSET ?5",
        session_columns!()
    );
    sqlx::query_as(&sql)
        .bind(filter.status.as_deref())
        .bind(filter.mode.as_deref())
        .bind(i64::from(filter.show_empty))
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}

/// Number of sessions matching `filter`.
pub async fn count(pool: &SqlitePool, filter: &SessionFilter) -> sqlx::Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM update_sessions s{FILTER_SQL}");
    sqlx::query_scalar(&sql)
        .bind(filter.status.as_deref())
        .bind(filter.mode.as_deref())
        .bind(i64::from(filter.show_empty))
        .fetch_one(pool)
        .await
}

/// Number of empty watch sessions the default filter hides.
pub async fn hidden_empty_count(pool: &SqlitePool, filter: &SessionFilter) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM update_sessions s WHERE (?1 IS NULL OR s.status = ?1) \
         AND (?2 IS NULL OR s.mode = ?2) AND s.mode = 'watch' AND s.status != 'running' \
         AND COALESCE(s.packages_attempted, 0) = 0",
    )
    .bind(filter.status.as_deref())
    .bind(filter.mode.as_deref())
    .fetch_one(pool)
    .await
}

/// Session counts by status and mode (for filter chips).
pub async fn facet_counts(pool: &SqlitePool) -> sqlx::Result<Vec<(String, String, i64)>> {
    sqlx::query_as(
        "SELECT status, mode, COUNT(*) FROM update_sessions s \
         WHERE NOT (s.mode = 'watch' AND s.status != 'running' \
                    AND COALESCE(s.packages_attempted, 0) = 0) \
         GROUP BY status, mode",
    )
    .fetch_all(pool)
    .await
}

/// A session with its stored configuration.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SessionDetailRow {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub session: SessionRow,
    pub config_json: Option<String>,
}

/// Look up one session.
pub async fn get(pool: &SqlitePool, id: &str) -> sqlx::Result<Option<SessionDetailRow>> {
    let sql = format!(
        "SELECT {}, s.config_json FROM update_sessions s WHERE s.id = ?1",
        session_columns!()
    );
    sqlx::query_as(&sql).bind(id).fetch_optional(pool).await
}

/// Running sessions, newest first.
pub async fn running(pool: &SqlitePool) -> sqlx::Result<Vec<SessionRow>> {
    let sql = format!(
        "SELECT {} FROM update_sessions s WHERE s.status = 'running' ORDER BY s.started_at DESC",
        session_columns!()
    );
    sqlx::query_as(&sql).fetch_all(pool).await
}

/// All phases of a session in execution order.
pub async fn phases(pool: &SqlitePool, session_id: &str) -> sqlx::Result<Vec<PhaseRow>> {
    let sql = format!(
        "SELECT {} FROM update_phases p WHERE p.session_id = ?1 ORDER BY p.id ASC",
        phase_columns!()
    );
    sqlx::query_as(&sql).bind(session_id).fetch_all(pool).await
}

/// Phases currently running inside running sessions (newest first).
pub async fn running_phases(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<PhaseRow>> {
    let sql = format!(
        "SELECT {} FROM update_phases p WHERE p.status = 'running' AND p.session_id IN \
         (SELECT id FROM update_sessions WHERE status = 'running') ORDER BY p.id DESC LIMIT ?1",
        phase_columns!()
    );
    sqlx::query_as(&sql).bind(limit).fetch_all(pool).await
}

/// A recent phase row joined with its session mode and package state.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ActivityRow {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub phase: PhaseRow,
    pub session_mode: Option<String>,
    pub session_status: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub current_version: Option<String>,
    pub proposed_version: Option<String>,
    pub failed_version: Option<String>,
}

/// The newest `limit` phase rows (by id), joined with package state.
pub async fn recent_phases(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<ActivityRow>> {
    let sql = format!(
        "SELECT {}, s.mode AS session_mode, s.status AS session_status, u.pr_url, u.pr_number, u.current_version, \
         u.proposed_version, u.failed_version \
         FROM (SELECT * FROM update_phases ORDER BY id DESC LIMIT ?1) p \
         LEFT JOIN update_sessions s ON s.id = p.session_id \
         LEFT JOIN updates u ON u.repo_name = p.repo_name AND u.attr_path = p.attr_path \
         ORDER BY p.id DESC",
        phase_columns!()
    );
    sqlx::query_as(&sql).bind(limit).fetch_all(pool).await
}
