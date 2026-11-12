//! Watch mode: `watched_repos`, `watch_cycles`, `upstream_sources`.

use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

/// One row of `watched_repos`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct RepoRow {
    pub name: String,
    pub upstream: Option<String>,
    pub path: String,
    pub entrypoint: String,
    pub base_branch: Option<String>,
    pub fork_remote: Option<String>,
    pub head_commit: Option<String>,
    pub head_subject: Option<String>,
    pub sync_status: Option<String>,
    pub sync_error: Option<String>,
    pub last_synced_at: Option<String>,
    pub index_status: Option<String>,
    pub index_error: Option<String>,
    pub last_indexed_at: Option<String>,
    pub index_duration_ms: Option<i64>,
    pub evaluated_count: Option<i64>,
    pub indexed_count: Option<i64>,
    pub index_skipped: Option<i64>,
    pub index_errors: Option<i64>,
    pub bootstrapped_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Backlog packages in this repo (computed).
    pub backlog_count: i64,
    /// Distinct attributes in `upstream_index` for this repo (computed).
    pub index_rows: i64,
}

/// All watched repositories.
pub async fn repos(pool: &SqlitePool) -> sqlx::Result<Vec<RepoRow>> {
    let sql = format!(
        "SELECT r.name, r.upstream, r.path, r.entrypoint, r.base_branch, r.fork_remote, \
         r.head_commit, r.head_subject, r.sync_status, r.sync_error, r.last_synced_at, \
         r.index_status, r.index_error, r.last_indexed_at, r.index_duration_ms, \
         r.evaluated_count, r.indexed_count, r.index_skipped, r.index_errors, \
         r.bootstrapped_at, r.created_at, r.updated_at, \
         (SELECT COUNT(*) FROM updates u WHERE u.repo_name = r.name AND {}) AS backlog_count, \
         (SELECT COUNT(DISTINCT i.attr_path) FROM upstream_index i \
          WHERE i.repo_name = r.name) AS index_rows \
         FROM watched_repos r ORDER BY r.name",
        backlog_sql!()
    );
    sqlx::query_as(&sql).fetch_all(pool).await
}

/// One row of `watch_cycles`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct CycleRow {
    pub id: i64,
    pub kind: String,
    pub repo_name: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub status: String,
    pub bootstrap: bool,
    pub sources_polled: i64,
    pub sources_failed: i64,
    pub packages_observed: i64,
    pub new_releases: i64,
    pub dispatched: i64,
    pub updated: i64,
    pub failed: i64,
    pub session_id: Option<String>,
    pub error: Option<String>,
}

/// One page of cycles (newest first), optionally only one kind.
pub async fn cycles(
    pool: &SqlitePool,
    kind: Option<&str>,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<CycleRow>> {
    let sql = format!(
        "SELECT {} FROM watch_cycles c WHERE (?1 IS NULL OR c.kind = ?1) \
         ORDER BY c.id DESC LIMIT ?2 OFFSET ?3",
        cycle_columns!()
    );
    sqlx::query_as(&sql)
        .bind(kind)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}

/// Number of cycles, optionally only one kind.
pub async fn cycle_count(pool: &SqlitePool, kind: Option<&str>) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM watch_cycles WHERE (?1 IS NULL OR kind = ?1)")
        .bind(kind)
        .fetch_one(pool)
        .await
}

/// The most recent poll cycle.
pub async fn last_poll(pool: &SqlitePool) -> sqlx::Result<Option<CycleRow>> {
    let sql = format!(
        "SELECT {} FROM watch_cycles c WHERE c.kind = 'poll' ORDER BY c.id DESC LIMIT 1",
        cycle_columns!()
    );
    sqlx::query_as(&sql).fetch_optional(pool).await
}

/// The poll cycle that created a session, if any.
pub async fn cycle_for_session(
    pool: &SqlitePool,
    session_id: &str,
) -> sqlx::Result<Option<CycleRow>> {
    let sql = format!(
        "SELECT {} FROM watch_cycles c WHERE c.session_id = ?1 ORDER BY c.id DESC LIMIT 1",
        cycle_columns!()
    );
    sqlx::query_as(&sql)
        .bind(session_id)
        .fetch_optional(pool)
        .await
}

/// One row of `upstream_sources` plus the number of packages using it.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SourceRow {
    pub upstream_type: String,
    pub upstream_key: String,
    pub instance: String,
    pub latest_version: Option<String>,
    pub latest_tag: Option<String>,
    pub release_count: Option<i64>,
    pub latest_changed_at: Option<String>,
    pub first_seen_at: String,
    pub last_polled_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub consecutive_failures: i64,
    pub package_count: i64,
}

/// Sources with consecutive poll failures, worst first.
pub async fn failing_sources(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<SourceRow>> {
    let sql = format!(
        "SELECT {} FROM upstream_sources s WHERE s.consecutive_failures > 0 \
         ORDER BY s.consecutive_failures DESC, s.last_polled_at DESC LIMIT ?1",
        source_columns!()
    );
    sqlx::query_as(&sql).bind(limit).fetch_all(pool).await
}

/// Sources whose latest release changed most recently.
pub async fn recent_releases(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<SourceRow>> {
    let sql = format!(
        "SELECT {} FROM upstream_sources s WHERE s.latest_changed_at IS NOT NULL \
         ORDER BY s.latest_changed_at DESC LIMIT ?1",
        source_columns!()
    );
    sqlx::query_as(&sql).bind(limit).fetch_all(pool).await
}

/// Source health per upstream type.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SourceTypeCount {
    pub upstream_type: String,
    pub total: i64,
    pub failing: i64,
    pub never_polled: i64,
}

/// Source counts grouped by type, largest first.
pub async fn source_type_counts(pool: &SqlitePool) -> sqlx::Result<Vec<SourceTypeCount>> {
    sqlx::query_as(
        "SELECT upstream_type, COUNT(*) AS total, \
         COALESCE(SUM(CASE WHEN consecutive_failures > 0 THEN 1 ELSE 0 END), 0) AS failing, \
         COALESCE(SUM(CASE WHEN last_polled_at IS NULL THEN 1 ELSE 0 END), 0) AS never_polled \
         FROM upstream_sources GROUP BY upstream_type ORDER BY total DESC, upstream_type",
    )
    .fetch_all(pool)
    .await
}
