//! Cheap change fingerprint for the `/ws/live` push channel.

use sqlx::SqlitePool;

/// Everything here is an indexed lookup or an aggregate over the (small)
/// set of running sessions, so polling it every couple of seconds is cheap.
const FINGERPRINT_SQL: &str = "SELECT \
    COALESCE((SELECT MAX(id) FROM update_phases), 0) || '|' || \
    COALESCE((SELECT COUNT(*) FROM update_phases WHERE status = 'running'), 0) || '|' || \
    COALESCE((SELECT id || ':' || status FROM watch_cycles ORDER BY id DESC LIMIT 1), '') \
        || '|' || \
    COALESCE((SELECT COUNT(*) || ':' || \
        COALESCE(SUM(COALESCE(packages_attempted, 0) + COALESCE(packages_succeeded, 0) \
            + COALESCE(packages_failed, 0) + COALESCE(packages_skipped, 0) \
            + COALESCE(packages_evaluated, 0) + COALESCE(packages_backoff, 0)), 0) \
        || ':' || COALESCE(MAX(updated_at), '') \
        FROM update_sessions WHERE status = 'running'), '') || '|' || \
    COALESCE((SELECT MAX(started_at) FROM update_sessions), '') || '|' || \
    COALESCE((SELECT MAX(updated_at) FROM watched_repos), '')";

/// A string that changes whenever something the live panels show changes.
pub async fn fingerprint(pool: &SqlitePool) -> sqlx::Result<String> {
    sqlx::query_scalar(FINGERPRINT_SQL).fetch_one(pool).await
}
