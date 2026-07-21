//! Database queue operations for the autofix pipeline.
//!
//! Provides methods on [`Database`] for managing the `autofix_queue` and
//! `autofix_attempts` tables.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::database::Database;

// ── Types ────────────────────────────────────────────────────────────────────

/// A single item in the autofix queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutofixQueueItem {
    pub id: i64,
    pub attr_path: String,
    pub session_id: String,
    pub error_type: String,
    pub failed_phase: String,
    pub status: String,
    pub priority: i64,
    pub attempts: i64,
    pub max_attempts: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub fixed_at: Option<DateTime<Utc>>,
    pub artifacts_path: Option<String>,
}

/// Record of a single LLM fix attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutofixAttemptRecord {
    pub id: i64,
    pub queue_id: i64,
    pub attempt_number: i64,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub prompt_text: Option<String>,
    pub response_text: Option<String>,
    pub changes_json: Option<String>,
    pub changes_applied: bool,
    pub build_success: Option<bool>,
    pub build_stderr: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
}

/// Aggregate statistics for the autofix queue.
#[derive(Debug, Default, Serialize)]
pub struct AutofixQueueStats {
    pub queued: i64,
    pub processing: i64,
    pub fixed: i64,
    pub escalated: i64,
    pub skipped: i64,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn parse_optional_rfc3339(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|v| parse_rfc3339(&v))
}

// ── Database methods ─────────────────────────────────────────────────────────

impl Database {
    /// Add a failed update to the autofix queue.
    ///
    /// Returns the queue item ID. If the (attr_path, session_id) pair already
    /// exists, the existing row is left unchanged and its ID is returned.
    pub async fn enqueue_autofix(
        &self,
        attr_path: &str,
        session_id: &str,
        error_type: &str,
        failed_phase: &str,
        artifacts_path: &str,
        max_attempts: i64,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();

        let result = sqlx::query(
            "INSERT INTO autofix_queue
                (attr_path, session_id, error_type, failed_phase,
                 artifacts_path, max_attempts, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(attr_path, session_id) DO NOTHING",
        )
        .bind(attr_path)
        .bind(session_id)
        .bind(error_type)
        .bind(failed_phase)
        .bind(artifacts_path)
        .bind(max_attempts)
        .bind(&now)
        .bind(&now)
        .execute(self.pool())
        .await
        .context("enqueue autofix item")?;

        // If we inserted, return the new row ID. Otherwise look it up.
        if result.rows_affected() > 0 {
            Ok(result.last_insert_rowid())
        } else {
            let id: i64 = sqlx::query_scalar(
                "SELECT id FROM autofix_queue WHERE attr_path = ? AND session_id = ?",
            )
            .bind(attr_path)
            .bind(session_id)
            .fetch_one(self.pool())
            .await
            .context("fetch existing autofix queue id")?;
            Ok(id)
        }
    }

    /// Atomically dequeue the next item: select the highest-priority `queued`
    /// item and transition it to `processing`.
    ///
    /// Returns `None` when the queue is empty.
    pub async fn dequeue_next_autofix(&self) -> Result<Option<AutofixQueueItem>> {
        let now = Utc::now().to_rfc3339();

        // SQLite doesn't have UPDATE ... RETURNING in all versions, so use a
        // two-step approach within a transaction-like sequence. Because we
        // are the only writer (serial processing), this is safe.
        let row = sqlx::query(
            "SELECT id, attr_path, session_id, error_type, failed_phase,
                    status, priority, attempts, max_attempts,
                    created_at, updated_at, fixed_at, artifacts_path
             FROM autofix_queue
             WHERE status = 'queued'
             ORDER BY priority DESC, created_at ASC
             LIMIT 1",
        )
        .fetch_optional(self.pool())
        .await
        .context("dequeue autofix item: select")?;

        let Some(row) = row else {
            return Ok(None);
        };

        let id: i64 = row.get("id");

        sqlx::query("UPDATE autofix_queue SET status = 'processing', updated_at = ? WHERE id = ?")
            .bind(&now)
            .bind(id)
            .execute(self.pool())
            .await
            .context("dequeue autofix item: update status")?;

        let item = AutofixQueueItem {
            id,
            attr_path: row.get("attr_path"),
            session_id: row.get("session_id"),
            error_type: row.get("error_type"),
            failed_phase: row.get("failed_phase"),
            status: "processing".to_owned(),
            priority: row.get("priority"),
            attempts: row.get("attempts"),
            max_attempts: row.get("max_attempts"),
            created_at: parse_rfc3339(row.get("created_at")).unwrap_or_else(Utc::now),
            updated_at: parse_rfc3339(&now).unwrap_or_else(Utc::now),
            fixed_at: parse_optional_rfc3339(row.get("fixed_at")),
            artifacts_path: row.get("artifacts_path"),
        };

        Ok(Some(item))
    }

    /// Update the status of a queue item.
    ///
    /// When transitioning back to `queued` (retry), the `attempts` counter is
    /// incremented. When transitioning to `fixed`, `fixed_at` is set.
    pub async fn update_autofix_status(&self, id: i64, status: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();

        match status {
            "queued" => {
                // Re-queue: increment attempts counter
                sqlx::query(
                    "UPDATE autofix_queue
                     SET status = 'queued', attempts = attempts + 1, updated_at = ?
                     WHERE id = ?",
                )
                .bind(&now)
                .bind(id)
                .execute(self.pool())
                .await
                .context("update autofix status to queued")?;
            },
            "fixed" => {
                sqlx::query(
                    "UPDATE autofix_queue
                     SET status = 'fixed', attempts = attempts + 1, updated_at = ?, fixed_at = ?
                     WHERE id = ?",
                )
                .bind(&now)
                .bind(&now)
                .bind(id)
                .execute(self.pool())
                .await
                .context("update autofix status to fixed")?;
            },
            _ => {
                sqlx::query(
                    "UPDATE autofix_queue SET status = ?, updated_at = ? WHERE id = ?",
                )
                .bind(status)
                .bind(&now)
                .bind(id)
                .execute(self.pool())
                .await
                .with_context(|| format!("update autofix status to {status}"))?;
            },
        }

        Ok(())
    }

    /// Record a single LLM fix attempt.
    ///
    /// Call this after the LLM responds (or fails). Returns the attempt row ID.
    pub async fn record_autofix_attempt(
        &self,
        queue_id: i64,
        attempt_number: i64,
        prompt_text: Option<&str>,
        response_text: Option<&str>,
        changes_json: Option<&str>,
        changes_applied: bool,
        build_success: Option<bool>,
        build_stderr: Option<&str>,
        status: &str,
        error_message: Option<&str>,
        prompt_tokens: Option<i64>,
        completion_tokens: Option<i64>,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();

        let result = sqlx::query(
            "INSERT INTO autofix_attempts
                (queue_id, attempt_number, started_at, completed_at,
                 prompt_tokens, completion_tokens,
                 prompt_text, response_text, changes_json,
                 changes_applied, build_success, build_stderr,
                 status, error_message)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(queue_id)
        .bind(attempt_number)
        .bind(&now)
        .bind(&now)
        .bind(prompt_tokens)
        .bind(completion_tokens)
        .bind(prompt_text)
        .bind(response_text)
        .bind(changes_json)
        .bind(changes_applied)
        .bind(build_success)
        .bind(build_stderr)
        .bind(status)
        .bind(error_message)
        .execute(self.pool())
        .await
        .context("record autofix attempt")?;

        Ok(result.last_insert_rowid())
    }

    /// Get all attempts for a queue item, ordered by attempt number.
    pub async fn get_autofix_attempts(&self, queue_id: i64) -> Result<Vec<AutofixAttemptRecord>> {
        let rows = sqlx::query(
            "SELECT id, queue_id, attempt_number, started_at, completed_at,
                    prompt_tokens, completion_tokens,
                    prompt_text, response_text, changes_json,
                    changes_applied, build_success, build_stderr,
                    status, error_message
             FROM autofix_attempts
             WHERE queue_id = ?
             ORDER BY attempt_number ASC",
        )
        .bind(queue_id)
        .fetch_all(self.pool())
        .await
        .context("get autofix attempts")?;

        let attempts = rows
            .into_iter()
            .filter_map(|row| {
                Some(AutofixAttemptRecord {
                    id: row.get("id"),
                    queue_id: row.get("queue_id"),
                    attempt_number: row.get("attempt_number"),
                    started_at: parse_rfc3339(row.get("started_at"))?,
                    completed_at: parse_optional_rfc3339(row.get("completed_at")),
                    prompt_tokens: row.get("prompt_tokens"),
                    completion_tokens: row.get("completion_tokens"),
                    prompt_text: row.get("prompt_text"),
                    response_text: row.get("response_text"),
                    changes_json: row.get("changes_json"),
                    changes_applied: row.get::<i64, _>("changes_applied") != 0,
                    build_success: row.get::<Option<i64>, _>("build_success").map(|v| v != 0),
                    build_stderr: row.get("build_stderr"),
                    status: row.get("status"),
                    error_message: row.get("error_message"),
                })
            })
            .collect();

        Ok(attempts)
    }

    /// Aggregate queue statistics.
    pub async fn get_autofix_queue_stats(&self) -> Result<AutofixQueueStats> {
        let rows = sqlx::query(
            "SELECT status, COUNT(*) as cnt FROM autofix_queue GROUP BY status",
        )
        .fetch_all(self.pool())
        .await
        .context("get autofix queue stats")?;

        let mut stats = AutofixQueueStats::default();
        for row in rows {
            let status: String = row.get("status");
            let count: i64 = row.get("cnt");
            match status.as_str() {
                "queued" => stats.queued = count,
                "processing" => stats.processing = count,
                "fixed" => stats.fixed = count,
                "escalated" => stats.escalated = count,
                "skipped" => stats.skipped = count,
                _ => {},
            }
        }

        Ok(stats)
    }

    /// Get queue items with optional filtering by attr_path pattern.
    pub async fn get_autofix_history(
        &self,
        attr_path: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<AutofixQueueItem>> {
        let mut sql = String::from(
            "SELECT id, attr_path, session_id, error_type, failed_phase,
                    status, priority, attempts, max_attempts,
                    created_at, updated_at, fixed_at, artifacts_path
             FROM autofix_queue",
        );

        if attr_path.is_some() {
            sql.push_str(" WHERE attr_path LIKE ?");
        }

        sql.push_str(" ORDER BY updated_at DESC");

        if let Some(lim) = limit {
            sql.push_str(&format!(" LIMIT {lim}"));
        }

        let mut q = sqlx::query(&sql);
        if let Some(pattern) = attr_path {
            q = q.bind(format!("%{pattern}%"));
        }

        let rows = q
            .fetch_all(self.pool())
            .await
            .context("get autofix history")?;

        let items = rows
            .into_iter()
            .filter_map(|row| {
                Some(AutofixQueueItem {
                    id: row.get("id"),
                    attr_path: row.get("attr_path"),
                    session_id: row.get("session_id"),
                    error_type: row.get("error_type"),
                    failed_phase: row.get("failed_phase"),
                    status: row.get("status"),
                    priority: row.get("priority"),
                    attempts: row.get("attempts"),
                    max_attempts: row.get("max_attempts"),
                    created_at: parse_rfc3339(row.get("created_at"))?,
                    updated_at: parse_rfc3339(row.get("updated_at"))?,
                    fixed_at: parse_optional_rfc3339(row.get("fixed_at")),
                    artifacts_path: row.get("artifacts_path"),
                })
            })
            .collect();

        Ok(items)
    }

    /// Check if an attr_path is already in the queue (any non-terminal status).
    pub async fn is_autofix_queued(&self, attr_path: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM autofix_queue
             WHERE attr_path = ? AND status IN ('queued', 'processing')",
        )
        .bind(attr_path)
        .fetch_one(self.pool())
        .await
        .context("check if autofix queued")?;

        Ok(count > 0)
    }

    /// Reset items stuck in `processing` status back to `queued`.
    ///
    /// This handles crash recovery: if the process was killed while processing
    /// an item, it would be stuck in `processing` forever.
    pub async fn reset_stale_autofix_processing(&self) -> Result<u64> {
        let now = Utc::now().to_rfc3339();

        let result = sqlx::query(
            "UPDATE autofix_queue SET status = 'queued', updated_at = ?
             WHERE status = 'processing'",
        )
        .bind(&now)
        .execute(self.pool())
        .await
        .context("reset stale autofix processing")?;

        Ok(result.rows_affected())
    }
}
