//! Aggregates for `/analytics`.
//!
//! Time-scoped queries take a `cutoff` RFC 3339 string; an empty cutoff
//! means "all time" (every timestamp compares `>= ''`).

use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use super::packages::PackageRow;

/// A label with a count.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct LabelCount {
    pub label: String,
    pub count: i64,
}

/// Failed phases per `error_type`.
pub async fn error_types(pool: &SqlitePool, cutoff: &str) -> sqlx::Result<Vec<LabelCount>> {
    sqlx::query_as(
        "SELECT error_type AS label, COUNT(*) AS count FROM update_phases \
         WHERE error_type IS NOT NULL AND started_at >= ?1 \
         GROUP BY error_type ORDER BY count DESC, label",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
}

/// Per-phase counts and durations.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct PhaseStatRow {
    pub phase: String,
    pub total: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub skipped: i64,
    pub running: i64,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<i64>,
}

pub async fn phase_stats(pool: &SqlitePool, cutoff: &str) -> sqlx::Result<Vec<PhaseStatRow>> {
    sqlx::query_as(
        "SELECT phase, COUNT(*) AS total, \
         COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) AS succeeded, \
         COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) AS failed, \
         COALESCE(SUM(CASE WHEN status = 'skipped' THEN 1 ELSE 0 END), 0) AS skipped, \
         COALESCE(SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END), 0) AS running, \
         AVG(duration_ms) AS avg_ms, MAX(duration_ms) AS max_ms \
         FROM update_phases WHERE started_at >= ?1 GROUP BY phase",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
}

/// p50 and p90 duration per phase, as `(phase, p50_ms, p90_ms)`.
pub async fn phase_percentiles(
    pool: &SqlitePool,
    cutoff: &str,
) -> sqlx::Result<Vec<(String, Option<i64>, Option<i64>)>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT phase, rn, cnt, duration_ms FROM ( \
             SELECT phase, duration_ms, \
                    ROW_NUMBER() OVER (PARTITION BY phase ORDER BY duration_ms) AS rn, \
                    COUNT(*) OVER (PARTITION BY phase) AS cnt \
             FROM update_phases WHERE duration_ms IS NOT NULL AND started_at >= ?1) \
         WHERE rn = (cnt + 1) / 2 OR rn = MAX(1, (cnt * 9 + 9) / 10)",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;

    let mut out: Vec<(String, Option<i64>, Option<i64>)> = Vec::new();
    for (phase, rn, cnt, ms) in rows {
        let idx = match out.iter().position(|(p, _, _)| *p == phase) {
            Some(i) => i,
            None => {
                out.push((phase, None, None));
                out.len() - 1
            },
        };
        if rn == (cnt + 1) / 2 {
            out[idx].1 = Some(ms);
        }
        if rn == ((cnt * 9 + 9) / 10).max(1) {
            out[idx].2 = Some(ms);
        }
    }
    Ok(out)
}

/// Outcomes per day of finished sessions that attempted something.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct DailyOutcome {
    pub day: String,
    pub sessions: i64,
    pub succeeded: i64,
    pub failed: i64,
}

pub async fn daily_outcomes(pool: &SqlitePool, cutoff: &str) -> sqlx::Result<Vec<DailyOutcome>> {
    sqlx::query_as(
        "SELECT date(started_at) AS day, COUNT(*) AS sessions, \
         COALESCE(SUM(packages_succeeded), 0) AS succeeded, \
         COALESCE(SUM(packages_failed), 0) AS failed \
         FROM update_sessions \
         WHERE status != 'running' AND COALESCE(packages_attempted, 0) > 0 \
           AND started_at >= ?1 AND date(started_at) IS NOT NULL \
         GROUP BY day ORDER BY day",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
}

/// `(repo_name, status, count, backlog)` for every repo/status pair.
pub async fn status_by_repo(pool: &SqlitePool) -> sqlx::Result<Vec<(String, String, i64, i64)>> {
    let sql = format!(
        "SELECT u.repo_name, u.status, COUNT(*), \
         COALESCE(SUM(CASE WHEN {} THEN 1 ELSE 0 END), 0) \
         FROM updates u GROUP BY u.repo_name, u.status ORDER BY u.repo_name",
        backlog_sql!()
    );
    sqlx::query_as(&sql).fetch_all(pool).await
}

/// Tracked packages per upstream type.
pub async fn upstream_types(pool: &SqlitePool) -> sqlx::Result<Vec<LabelCount>> {
    sqlx::query_as(
        "SELECT COALESCE(upstream_type, 'unknown') AS label, COUNT(*) AS count \
         FROM updates GROUP BY label ORDER BY count DESC, label",
    )
    .fetch_all(pool)
    .await
}

/// Rebuild-count buckets, in display order.
pub const REBUILD_BUCKETS: [&str; 6] = [
    "0",
    "1–10",
    "11–100",
    "101–1,000",
    "1,001–10,000",
    "10,001+",
];

/// Packages per rebuild bucket (`unknown` for packages never measured).
pub async fn rebuild_buckets(pool: &SqlitePool) -> sqlx::Result<Vec<LabelCount>> {
    sqlx::query_as(
        "SELECT CASE WHEN rebuild_count IS NULL THEN 'unknown' \
                     WHEN rebuild_count <= 0 THEN '0' \
                     WHEN rebuild_count <= 10 THEN '1–10' \
                     WHEN rebuild_count <= 100 THEN '11–100' \
                     WHEN rebuild_count <= 1000 THEN '101–1,000' \
                     WHEN rebuild_count <= 10000 THEN '1,001–10,000' \
                     ELSE '10,001+' END AS label, COUNT(*) AS count \
         FROM updates GROUP BY label",
    )
    .fetch_all(pool)
    .await
}

/// Packages with the largest rebuild impact.
pub async fn top_rebuilds(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<PackageRow>> {
    let sql = format!(
        "SELECT {} FROM updates u WHERE u.rebuild_count IS NOT NULL \
         ORDER BY u.rebuild_count DESC, u.attr_path LIMIT ?1",
        package_columns!()
    );
    sqlx::query_as(&sql).bind(limit).fetch_all(pool).await
}

/// Autofix queue entries per status, created since the cutoff.
pub async fn autofix_queue(pool: &SqlitePool, cutoff: &str) -> sqlx::Result<Vec<LabelCount>> {
    sqlx::query_as(
        "SELECT status AS label, COUNT(*) AS count FROM autofix_queue \
         WHERE created_at >= ?1 GROUP BY status ORDER BY count DESC, label",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
}

/// Autofix attempts per status with token usage.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AttemptOutcome {
    pub status: String,
    pub count: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

pub async fn autofix_attempts(
    pool: &SqlitePool,
    cutoff: &str,
) -> sqlx::Result<Vec<AttemptOutcome>> {
    sqlx::query_as(
        "SELECT status, COUNT(*) AS count, \
         COALESCE(SUM(prompt_tokens), 0) AS prompt_tokens, \
         COALESCE(SUM(completion_tokens), 0) AS completion_tokens \
         FROM autofix_attempts WHERE started_at >= ?1 \
         GROUP BY status ORDER BY count DESC, status",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
}

/// A frequently failing package.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct FailingPackage {
    pub repo_name: String,
    pub attr_path: String,
    pub failures: i64,
    pub last_failure: String,
    pub consecutive: Option<i64>,
    pub last_error_type: Option<String>,
}

/// Packages with the most failure logs since the cutoff.
pub async fn top_failing(
    pool: &SqlitePool,
    cutoff: &str,
    limit: i64,
) -> sqlx::Result<Vec<FailingPackage>> {
    sqlx::query_as(
        "SELECT l.repo_name, l.attr_path, COUNT(*) AS failures, MAX(l.timestamp) AS last_failure, \
         (SELECT u.failure_count FROM updates u WHERE u.repo_name = l.repo_name \
            AND u.attr_path = l.attr_path) AS consecutive, \
         (SELECT l2.error_type FROM update_logs l2 WHERE l2.repo_name = l.repo_name \
            AND l2.attr_path = l.attr_path ORDER BY l2.timestamp DESC LIMIT 1) AS last_error_type \
         FROM update_logs l WHERE l.timestamp >= ?1 \
         GROUP BY l.repo_name, l.attr_path ORDER BY failures DESC, last_failure DESC LIMIT ?2",
    )
    .bind(cutoff)
    .bind(limit)
    .fetch_all(pool)
    .await
}
