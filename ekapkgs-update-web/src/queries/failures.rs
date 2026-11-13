//! `update_logs`: recorded update failures.

use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use super::like_pattern;

/// One row of `update_logs`, with only the tail of the log text.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct LogRow {
    /// Primary key; synthetic (`<repo>:<attr>@<version>`) for watch.
    pub drv_path: String,
    pub repo_name: String,
    pub attr_path: String,
    pub timestamp: String,
    pub status: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub session_id: Option<String>,
    pub phase: Option<String>,
    pub error_type: Option<String>,
    /// Last 6,000 characters of `error_log`.
    pub log_tail: String,
    /// Full length of `error_log` in characters.
    pub log_len: i64,
}

/// Filters for `/failures`.
#[derive(Debug, Clone, Default)]
pub struct FailureFilter {
    pub q: Option<String>,
    pub repo: Option<String>,
    pub phase: Option<String>,
    pub error_type: Option<String>,
}

/// ?1 like-pattern, ?2 repo, ?3 phase, ?4 error type.
const FILTER_SQL: &str = " WHERE (?1 IS NULL OR l.attr_path LIKE ?1 ESCAPE '\\') \
     AND (?2 IS NULL OR l.repo_name = ?2) AND (?3 IS NULL OR l.phase = ?3) \
     AND (?4 IS NULL OR l.error_type = ?4)";

/// One page of failures, newest first.
pub async fn list(
    pool: &SqlitePool,
    filter: &FailureFilter,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<LogRow>> {
    let sql = format!(
        "SELECT {} FROM update_logs l{FILTER_SQL} ORDER BY l.timestamp DESC LIMIT ?5 OFFSET ?6",
        log_columns!()
    );
    sqlx::query_as(&sql)
        .bind(filter.q.as_deref().map(like_pattern))
        .bind(filter.repo.as_deref())
        .bind(filter.phase.as_deref())
        .bind(filter.error_type.as_deref())
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}

/// Number of failures matching `filter`.
pub async fn count(pool: &SqlitePool, filter: &FailureFilter) -> sqlx::Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM update_logs l{FILTER_SQL}");
    sqlx::query_scalar(&sql)
        .bind(filter.q.as_deref().map(like_pattern))
        .bind(filter.repo.as_deref())
        .bind(filter.phase.as_deref())
        .bind(filter.error_type.as_deref())
        .fetch_one(pool)
        .await
}

/// Distinct values for the filter dropdowns: (repos, phases, error types).
pub async fn facets(pool: &SqlitePool) -> sqlx::Result<(Vec<String>, Vec<String>, Vec<String>)> {
    let repos = sqlx::query_scalar("SELECT DISTINCT repo_name FROM update_logs ORDER BY 1")
        .fetch_all(pool)
        .await?;
    let phases = sqlx::query_scalar(
        "SELECT DISTINCT phase FROM update_logs WHERE phase IS NOT NULL ORDER BY 1",
    )
    .fetch_all(pool)
    .await?;
    let error_types = sqlx::query_scalar(
        "SELECT DISTINCT error_type FROM update_logs WHERE error_type IS NOT NULL ORDER BY 1",
    )
    .fetch_all(pool)
    .await?;
    Ok((repos, phases, error_types))
}

/// Failure logs recorded for a session.
pub async fn for_session(
    pool: &SqlitePool,
    session_id: &str,
    limit: i64,
) -> sqlx::Result<Vec<LogRow>> {
    let sql = format!(
        "SELECT {} FROM update_logs l WHERE l.session_id = ?1 ORDER BY l.timestamp DESC LIMIT ?2",
        log_columns!()
    );
    sqlx::query_as(&sql)
        .bind(session_id)
        .bind(limit)
        .fetch_all(pool)
        .await
}
