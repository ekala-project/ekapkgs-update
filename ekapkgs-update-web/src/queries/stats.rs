//! Headline numbers for the dashboard and `/api/stats`.

use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

/// Update outcomes summed over sessions started since a cutoff.
#[derive(Debug, Clone, Default, Serialize, FromRow)]
pub struct OutcomeWindow {
    pub sessions: i64,
    pub attempted: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub skipped: i64,
}

impl OutcomeWindow {
    /// Succeeded / (succeeded + failed), in percent.
    pub fn success_rate(&self) -> Option<f64> {
        crate::params::ratio_pct(self.succeeded, self.succeeded + self.failed)
    }
}

/// Outcomes of sessions that attempted something and started in
/// `[from, to)` (RFC 3339; an empty `to` means "until now").
pub async fn outcomes_between(
    pool: &SqlitePool,
    from: &str,
    to: &str,
) -> sqlx::Result<OutcomeWindow> {
    sqlx::query_as(
        "SELECT COUNT(*) AS sessions, \
         COALESCE(SUM(packages_attempted), 0) AS attempted, \
         COALESCE(SUM(packages_succeeded), 0) AS succeeded, \
         COALESCE(SUM(packages_failed), 0) AS failed, \
         COALESCE(SUM(packages_skipped), 0) AS skipped \
         FROM update_sessions WHERE started_at >= ?1 AND (?2 = '' OR started_at < ?2) \
         AND COALESCE(packages_attempted, 0) > 0",
    )
    .bind(from)
    .bind(to)
    .fetch_one(pool)
    .await
}

/// Total number of sessions (excluding legacy empty watch sessions).
pub async fn session_total(pool: &SqlitePool) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM update_sessions s WHERE NOT (s.mode = 'watch' \
         AND s.status != 'running' AND COALESCE(s.packages_attempted, 0) = 0)",
    )
    .fetch_one(pool)
    .await
}
