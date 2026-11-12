//! Per-package update state (`updates`) and failure logs (`update_logs`).
//!
//! Every package row is scoped by `repo_name` (`''` for the `run` command and
//! legacy single-repo watch mode), so the same attr_path can be tracked
//! independently in several watched repositories.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use sqlx::Row;
use tracing::{debug, info};

use super::types::{
    FailedUpdate, PackageInfo, UpdateLog, UpdateRecord, UpdateStatus, summarize_error,
};
use super::{Database, FIRST_BACKOFF_DAYS, MAX_BACKOFF_DAYS, SECOND_BACKOFF_DAYS, parse_rfc3339};
use crate::vcs_sources::compare_version_components;

/// Columns selected for [`UpdateLog`] rows.
const LOG_COLUMNS: &str = "drv_path, attr_path, timestamp, status, error_log, old_version, \
                           new_version, repo_name, session_id, phase, error_type";

/// `a >= b` using the same component-wise comparison as release selection.
fn version_at_least(a: &str, b: &str) -> bool {
    a == b || compare_version_components(a, b).is_ge()
}

/// Backoff applied after a check that found nothing to do.
///
/// First check → 2 days; re-checked soon after the previous window → 4 days;
/// otherwise → 6 days (max).
pub(crate) fn check_backoff_days(last_checked: Option<DateTime<Utc>>, now: DateTime<Utc>) -> i64 {
    match last_checked {
        None => FIRST_BACKOFF_DAYS,
        Some(last) if (now - last).num_days() <= FIRST_BACKOFF_DAYS => SECOND_BACKOFF_DAYS,
        Some(_) => MAX_BACKOFF_DAYS,
    }
}

/// Backoff applied after `failure_count` consecutive failed attempts:
/// 2 days → 4 days → 6 days (max).
pub(crate) fn failure_backoff_days(failure_count: i64) -> i64 {
    match failure_count {
        ..=1 => FIRST_BACKOFF_DAYS,
        2 => SECOND_BACKOFF_DAYS,
        _ => MAX_BACKOFF_DAYS,
    }
}

/// State of a row after reconciling a fresh observation with what was stored.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reconciled {
    status: UpdateStatus,
    latest: Option<String>,
    proposed: Option<String>,
    failed_version: Option<String>,
    failure_count: i64,
    last_error: Option<String>,
}

/// Merge an observation (`current`, best compatible `latest`) into the stored
/// record.
///
/// - A proposed version that the tree has caught up with (merged) is cleared.
/// - Failure bookkeeping for a version the tree has caught up with is cleared.
/// - `latest = None` means upstream could not be queried: the previously known
///   latest version is kept and the status becomes `upstream_error`.
fn reconcile(
    prev: Option<&UpdateRecord>,
    current: &str,
    latest: Option<&str>,
    upstream_error: Option<&str>,
) -> Reconciled {
    let proposed = prev
        .and_then(|r| r.proposed_version.clone())
        .filter(|p| !version_at_least(current, p));
    let failed_version = prev
        .and_then(|r| r.failed_version.clone())
        .filter(|f| !version_at_least(current, f));
    let (failure_count, prev_error) = if failed_version.is_some() {
        (
            prev.map_or(0, |r| r.failure_count),
            prev.and_then(|r| r.last_error.clone()),
        )
    } else {
        (0, None)
    };

    match latest {
        Some(latest) => {
            let status = UpdateStatus::observed(
                current,
                latest,
                proposed.as_deref(),
                failed_version.as_deref(),
            );
            let last_error = if status == UpdateStatus::Failed {
                prev_error
            } else {
                None
            };
            Reconciled {
                status,
                latest: Some(latest.to_owned()),
                proposed,
                failed_version,
                failure_count,
                last_error,
            }
        },
        None => Reconciled {
            status: UpdateStatus::UpstreamError,
            latest: prev.and_then(|r| r.latest_upstream_version.clone()),
            proposed,
            failed_version,
            failure_count,
            last_error: upstream_error.map(summarize_error).or(prev_error),
        },
    }
}

impl Database {
    /// Get the update record for a package in a repo (`''` = default repo).
    pub async fn get_update_record(
        &self,
        repo_name: &str,
        attr_path: &str,
    ) -> Result<Option<UpdateRecord>> {
        let row = sqlx::query(
            r#"
            SELECT status, current_version, latest_upstream_version, proposed_version,
                   baseline_version, failed_version, failure_count, last_error,
                   last_checked, last_attempted, next_attempt
            FROM updates
            WHERE repo_name = ? AND attr_path = ?
            "#,
        )
        .bind(repo_name)
        .bind(attr_path)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("get update record for {repo_name}:{attr_path}"))?;

        row.map(|row| {
            let status: String = row.try_get("status")?;
            Ok(UpdateRecord {
                status: UpdateStatus::parse(&status).unwrap_or(UpdateStatus::Pending),
                current_version: row.try_get("current_version")?,
                latest_upstream_version: row.try_get("latest_upstream_version")?,
                proposed_version: row.try_get("proposed_version")?,
                baseline_version: row.try_get("baseline_version")?,
                failed_version: row.try_get("failed_version")?,
                failure_count: row.try_get("failure_count")?,
                last_error: row.try_get("last_error")?,
                last_checked: parse_rfc3339(row.try_get("last_checked")?),
                last_attempted: parse_rfc3339(row.try_get("last_attempted")?),
                next_attempt: parse_rfc3339(row.try_get("next_attempt")?),
            })
        })
        .transpose()
    }

    /// Check if a package should be checked for updates.
    ///
    /// Returns true if there is no record, no `next_attempt`, or `next_attempt`
    /// is in the past.
    pub async fn should_check_update(&self, repo_name: &str, attr_path: &str) -> Result<bool> {
        let Some(record) = self.get_update_record(repo_name, attr_path).await? else {
            debug!("{}: No record found, should check", attr_path);
            return Ok(true);
        };
        let should_check = !record.in_backoff(Utc::now());
        debug!(
            "{}: next_attempt {:?}, should check: {}",
            attr_path, record.next_attempt, should_check
        );
        Ok(should_check)
    }

    /// Register a package (or refresh its metadata) without touching its
    /// upstream/backoff state. New rows start as `pending`.
    ///
    /// When the tree's `current_version` has caught up with a proposed
    /// version, the proposal is considered merged.
    pub async fn upsert_package_info(
        &self,
        repo_name: &str,
        attr_path: &str,
        current_version: &str,
        info: &PackageInfo,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO updates (repo_name, attr_path, status, current_version, pname,
                                 description, homepage, upstream_type, upstream_key,
                                 upstream_instance, semver_strategy)
            VALUES (?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(repo_name, attr_path) DO UPDATE SET
                status = CASE
                    WHEN updates.status = 'proposed'
                         AND updates.proposed_version = excluded.current_version
                    THEN 'up_to_date' ELSE updates.status END,
                proposed_version = CASE
                    WHEN updates.proposed_version = excluded.current_version
                    THEN NULL ELSE updates.proposed_version END,
                current_version = excluded.current_version,
                pname = excluded.pname,
                description = excluded.description,
                homepage = excluded.homepage,
                upstream_type = excluded.upstream_type,
                upstream_key = excluded.upstream_key,
                upstream_instance = excluded.upstream_instance,
                semver_strategy = excluded.semver_strategy
            "#,
        )
        .bind(repo_name)
        .bind(attr_path)
        .bind(current_version)
        .bind(&info.pname)
        .bind(&info.description)
        .bind(&info.homepage)
        .bind(&info.upstream_type)
        .bind(&info.upstream_key)
        .bind(&info.upstream_instance)
        .bind(&info.semver_strategy)
        .execute(&self.pool)
        .await
        .with_context(|| format!("upsert package info for {repo_name}:{attr_path}"))?;
        Ok(())
    }

    /// Write a reconciled observation. `next_attempt` is written verbatim.
    async fn write_observation(
        &self,
        repo_name: &str,
        attr_path: &str,
        current_version: &str,
        state: &Reconciled,
        baseline: Option<&str>,
        next_attempt: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO updates (repo_name, attr_path, status, current_version,
                                 latest_upstream_version, proposed_version, baseline_version,
                                 failed_version, failure_count, last_error, last_checked,
                                 next_attempt)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(repo_name, attr_path) DO UPDATE SET
                status = excluded.status,
                current_version = excluded.current_version,
                latest_upstream_version = excluded.latest_upstream_version,
                proposed_version = excluded.proposed_version,
                baseline_version = excluded.baseline_version,
                failed_version = excluded.failed_version,
                failure_count = excluded.failure_count,
                last_error = excluded.last_error,
                last_checked = excluded.last_checked,
                next_attempt = excluded.next_attempt
            "#,
        )
        .bind(repo_name)
        .bind(attr_path)
        .bind(state.status.as_str())
        .bind(current_version)
        .bind(&state.latest)
        .bind(&state.proposed)
        .bind(baseline)
        .bind(&state.failed_version)
        .bind(state.failure_count)
        .bind(&state.last_error)
        .bind(&now)
        .bind(next_attempt.map(|t| t.to_rfc3339()))
        .execute(&self.pool)
        .await
        .with_context(|| format!("record observation for {repo_name}:{attr_path}"))?;
        Ok(())
    }

    /// Record a check that will not dispatch an update (already up to date,
    /// already proposed, or upstream could not be queried).
    ///
    /// Applies the check backoff: 2 days → 4 days → 6 days (max).
    /// `latest = None` records an upstream error (with `error` as the reason).
    pub async fn record_no_update(
        &self,
        repo_name: &str,
        attr_path: &str,
        current_version: &str,
        latest_upstream_version: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now();
        let prev = self.get_update_record(repo_name, attr_path).await?;
        let backoff_days = check_backoff_days(
            prev.as_ref()
                .and_then(|r| r.last_checked.or(r.last_attempted)),
            now,
        );
        let next_attempt = now + Duration::days(backoff_days);
        debug!(
            "{}: No update to dispatch, next check {} ({} days)",
            attr_path,
            next_attempt.to_rfc3339(),
            backoff_days
        );

        let state = reconcile(
            prev.as_ref(),
            current_version,
            latest_upstream_version,
            error,
        );
        let baseline = prev.as_ref().and_then(|r| r.baseline_version.as_deref());
        self.write_observation(
            repo_name,
            attr_path,
            current_version,
            &state,
            baseline,
            Some(next_attempt),
        )
        .await
    }

    /// Record that the checker found a newer compatible release and is about
    /// to dispatch an update. Leaves backoff untouched (the attempt sets it).
    pub async fn record_update_available(
        &self,
        repo_name: &str,
        attr_path: &str,
        current_version: &str,
        latest_upstream_version: &str,
    ) -> Result<()> {
        let prev = self.get_update_record(repo_name, attr_path).await?;
        let state = reconcile(
            prev.as_ref(),
            current_version,
            Some(latest_upstream_version),
            None,
        );
        self.write_observation(
            repo_name,
            attr_path,
            current_version,
            &state,
            prev.as_ref().and_then(|r| r.baseline_version.as_deref()),
            prev.as_ref().and_then(|r| r.next_attempt),
        )
        .await
    }

    /// Record a `watch` observation of the best compatible upstream release.
    ///
    /// The first observation of a package records `latest` as its
    /// `baseline_version` (observe-only bootstrap). Returns the record as it
    /// was *before* this observation so the caller can decide whether to
    /// dispatch. Backoff is left untouched.
    pub async fn record_observation(
        &self,
        repo_name: &str,
        attr_path: &str,
        current_version: &str,
        latest_upstream_version: &str,
    ) -> Result<Option<UpdateRecord>> {
        let prev = self.get_update_record(repo_name, attr_path).await?;
        let state = reconcile(
            prev.as_ref(),
            current_version,
            Some(latest_upstream_version),
            None,
        );
        let baseline = prev
            .as_ref()
            .and_then(|r| r.baseline_version.as_deref())
            .unwrap_or(latest_upstream_version);
        self.write_observation(
            repo_name,
            attr_path,
            current_version,
            &state,
            Some(baseline),
            prev.as_ref().and_then(|r| r.next_attempt),
        )
        .await?;
        Ok(prev)
    }

    /// Record a successful update (built and committed / PR opened).
    ///
    /// Sets the status to `proposed`, clears failure bookkeeping and resets the
    /// backoff to 2 days. PR details are recorded afterwards via
    /// [`Self::record_pr_info`].
    pub async fn record_successful_update(
        &self,
        repo_name: &str,
        attr_path: &str,
        old_version: &str,
        new_version: &str,
        rebuild_count: Option<usize>,
        session_id: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now();
        let next_attempt = now + Duration::days(FIRST_BACKOFF_DAYS);

        match rebuild_count {
            Some(count) => info!(
                "{}: Successful update from {} to {} ({} rebuilds)",
                attr_path, old_version, new_version, count
            ),
            None => info!(
                "{}: Successful update from {} to {}",
                attr_path, old_version, new_version
            ),
        }

        let rebuild_count = rebuild_count
            .map(i64::try_from)
            .transpose()
            .context("rebuild_count overflowed i64")?;

        sqlx::query(
            r#"
            INSERT INTO updates (repo_name, attr_path, status, current_version,
                                 latest_upstream_version, proposed_version, failure_count,
                                 last_attempted, next_attempt, rebuild_count, last_session_id)
            VALUES (?, ?, 'proposed', ?, ?, ?, 0, ?, ?, ?, ?)
            ON CONFLICT(repo_name, attr_path) DO UPDATE SET
                status = 'proposed',
                current_version = excluded.current_version,
                latest_upstream_version = excluded.latest_upstream_version,
                proposed_version = excluded.proposed_version,
                failed_version = NULL,
                failure_count = 0,
                last_error = NULL,
                last_attempted = excluded.last_attempted,
                next_attempt = excluded.next_attempt,
                rebuild_count = COALESCE(excluded.rebuild_count, updates.rebuild_count),
                last_session_id = excluded.last_session_id,
                pr_url = NULL,
                pr_number = NULL
            "#,
        )
        .bind(repo_name)
        .bind(attr_path)
        .bind(old_version)
        .bind(new_version)
        .bind(new_version)
        .bind(now.to_rfc3339())
        .bind(next_attempt.to_rfc3339())
        .bind(rebuild_count)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .context("Failed to record successful update")?;

        Ok(())
    }

    /// Record PR information for a successful update
    pub async fn record_pr_info(
        &self,
        repo_name: &str,
        attr_path: &str,
        pr_url: &str,
        pr_number: i64,
    ) -> Result<()> {
        info!("{}: Recording PR #{} ({})", attr_path, pr_number, pr_url);

        sqlx::query(
            r#"
            UPDATE updates
            SET pr_url = ?, pr_number = ?
            WHERE repo_name = ? AND attr_path = ?
            "#,
        )
        .bind(pr_url)
        .bind(pr_number)
        .bind(repo_name)
        .bind(attr_path)
        .execute(&self.pool)
        .await
        .context("Failed to record PR info")?;

        Ok(())
    }

    /// Record the CVE delta of a proposed update (shown on the dashboard).
    pub async fn record_cve_counts(
        &self,
        repo_name: &str,
        attr_path: &str,
        resolved: usize,
        introduced: usize,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE updates SET cve_resolved_count = ?, cve_introduced_count = ? \
             WHERE repo_name = ? AND attr_path = ?",
        )
        .bind(i64::try_from(resolved).unwrap_or(i64::MAX))
        .bind(i64::try_from(introduced).unwrap_or(i64::MAX))
        .bind(repo_name)
        .bind(attr_path)
        .execute(&self.pool)
        .await
        .with_context(|| format!("record CVE counts for {repo_name}:{attr_path}"))?;
        Ok(())
    }

    /// Record a failed update attempt.
    ///
    /// Writes the full error log to `update_logs` and marks the package as
    /// `failed` with a stepped backoff based on consecutive failures:
    /// 2 days → 4 days → 6 days (max).
    pub async fn record_failed_update(&self, failure: &FailedUpdate<'_>) -> Result<()> {
        let now = Utc::now();
        let key = failure.log_key();
        debug!(
            "{}: Recording failed update attempt ({})",
            failure.attr_path, key
        );

        sqlx::query(
            r#"
            INSERT INTO update_logs (drv_path, attr_path, timestamp, status, error_log,
                                     old_version, new_version, repo_name, session_id,
                                     phase, error_type)
            VALUES (?, ?, ?, 'failed', ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(drv_path) DO UPDATE SET
                timestamp = excluded.timestamp,
                error_log = excluded.error_log,
                old_version = excluded.old_version,
                new_version = excluded.new_version,
                repo_name = excluded.repo_name,
                session_id = excluded.session_id,
                phase = excluded.phase,
                error_type = excluded.error_type
            "#,
        )
        .bind(&key)
        .bind(failure.attr_path)
        .bind(now.to_rfc3339())
        .bind(failure.error_log)
        .bind(failure.old_version)
        .bind(failure.new_version)
        .bind(failure.repo_name)
        .bind(failure.session_id)
        .bind(failure.phase)
        .bind(failure.error_type)
        .execute(&self.pool)
        .await
        .context("Failed to record failed update log")?;

        let summary = {
            let line = summarize_error(failure.error_log);
            match failure.phase {
                Some(phase) if !line.is_empty() => format!("{phase}: {line}"),
                Some(phase) => format!("{phase} failed"),
                None => line,
            }
        };
        let backoff_at =
            |count: i64| (now + Duration::days(failure_backoff_days(count))).to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO updates (repo_name, attr_path, status, current_version, failed_version,
                                 failure_count, last_error, last_session_id, last_attempted,
                                 next_attempt)
            VALUES (?, ?, 'failed', ?, ?, 1, ?, ?, ?, ?)
            ON CONFLICT(repo_name, attr_path) DO UPDATE SET
                status = 'failed',
                current_version = COALESCE(excluded.current_version, updates.current_version),
                failed_version = excluded.failed_version,
                failure_count = updates.failure_count + 1,
                last_error = excluded.last_error,
                last_session_id = excluded.last_session_id,
                last_attempted = excluded.last_attempted,
                next_attempt = CASE updates.failure_count + 1
                    WHEN 1 THEN ? WHEN 2 THEN ? ELSE ? END
            "#,
        )
        .bind(failure.repo_name)
        .bind(failure.attr_path)
        .bind(failure.old_version)
        .bind(failure.new_version)
        .bind(&summary)
        .bind(failure.session_id)
        .bind(now.to_rfc3339())
        .bind(backoff_at(1))
        .bind(backoff_at(1))
        .bind(backoff_at(2))
        .bind(backoff_at(3))
        .execute(&self.pool)
        .await
        .context("Failed to record failed update state")?;

        Ok(())
    }

    /// Get a log entry by drv_path (supports both full path and hash-name format)
    pub async fn get_log_by_drv(&self, drv_identifier: &str) -> Result<Option<UpdateLog>> {
        let mut log = sqlx::query_as::<_, UpdateLog>(&format!(
            "SELECT {LOG_COLUMNS} FROM update_logs WHERE drv_path = ?"
        ))
        .bind(drv_identifier)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("get log by drv {drv_identifier}"))?;

        // If no exact match and identifier doesn't start with /nix/store/,
        // try matching the end of drv_path
        if log.is_none() && !drv_identifier.starts_with("/nix/store/") {
            log = sqlx::query_as::<_, UpdateLog>(&format!(
                "SELECT {LOG_COLUMNS} FROM update_logs WHERE drv_path LIKE ? \
                 ORDER BY timestamp DESC LIMIT 1"
            ))
            .bind(format!("%/{drv_identifier}"))
            .fetch_optional(&self.pool)
            .await
            .with_context(|| format!("get log by drv suffix {drv_identifier}"))?;
        }

        Ok(log)
    }

    /// Get all failed logs for an attr_path (any repo), most recent first
    pub async fn get_all_failed_logs_by_attr(&self, attr_path: &str) -> Result<Vec<UpdateLog>> {
        let logs = sqlx::query_as::<_, UpdateLog>(&format!(
            "SELECT {LOG_COLUMNS} FROM update_logs WHERE attr_path = ? ORDER BY timestamp DESC"
        ))
        .bind(attr_path)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("get failed logs for {attr_path}"))?;

        Ok(logs)
    }

    /// Query failed update logs with optional filtering
    pub async fn query_failed_logs(
        &self,
        package_pattern: Option<&str>,
        since: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Result<Vec<UpdateLog>> {
        let mut query = format!("SELECT {LOG_COLUMNS} FROM update_logs WHERE status = 'failed'");

        if package_pattern.is_some() {
            query.push_str(" AND attr_path LIKE ?");
        }
        if since.is_some() {
            query.push_str(" AND timestamp >= ?");
        }

        query.push_str(" ORDER BY timestamp DESC");

        if let Some(lim) = limit {
            query.push_str(&format!(" LIMIT {lim}"));
        }

        let mut q = sqlx::query_as::<_, UpdateLog>(&query);

        if let Some(pattern) = package_pattern {
            q = q.bind(pattern.to_owned());
        }
        if let Some(since_dt) = since {
            q = q.bind(since_dt.to_rfc3339());
        }

        let logs = q
            .fetch_all(&self.pool)
            .await
            .context("query failed update logs")?;

        Ok(logs)
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

    fn record(proposed: Option<&str>, failed: Option<&str>, failures: i64) -> UpdateRecord {
        UpdateRecord {
            status: UpdateStatus::Pending,
            current_version: Some("1.0".into()),
            latest_upstream_version: Some("1.1".into()),
            proposed_version: proposed.map(Into::into),
            baseline_version: None,
            failed_version: failed.map(Into::into),
            failure_count: failures,
            last_error: failed.map(|_| "build: boom".into()),
            last_checked: None,
            last_attempted: None,
            next_attempt: None,
        }
    }

    #[test]
    fn backoff_steps() {
        let now = Utc::now();
        assert_eq!(check_backoff_days(None, now), 2);
        assert_eq!(check_backoff_days(Some(now - Duration::days(2)), now), 4);
        assert_eq!(check_backoff_days(Some(now - Duration::days(4)), now), 6);
        assert_eq!(failure_backoff_days(1), 2);
        assert_eq!(failure_backoff_days(2), 4);
        assert_eq!(failure_backoff_days(3), 6);
        assert_eq!(failure_backoff_days(10), 6);
    }

    #[test]
    fn reconcile_clears_merged_proposal_and_stale_failure() {
        // Tree caught up with the proposed version: proposal is merged.
        let prev = record(Some("1.1"), None, 0);
        let r = reconcile(Some(&prev), "1.1", Some("1.1"), None);
        assert_eq!(r.status, UpdateStatus::UpToDate);
        assert_eq!(r.proposed, None);

        // Tree moved past the failed version: failure bookkeeping is dropped.
        let prev = record(None, Some("1.1"), 2);
        let r = reconcile(Some(&prev), "1.2", Some("1.2"), None);
        assert_eq!(r.failed_version, None);
        assert_eq!(r.failure_count, 0);
        assert_eq!(r.last_error, None);

        // Same failed version still latest: keep failed status and error.
        let prev = record(None, Some("1.1"), 2);
        let r = reconcile(Some(&prev), "1.0", Some("1.1"), None);
        assert_eq!(r.status, UpdateStatus::Failed);
        assert_eq!(r.failure_count, 2);
        assert_eq!(r.last_error.as_deref(), Some("build: boom"));

        // Upstream error keeps the last known latest version.
        let prev = record(None, None, 0);
        let r = reconcile(Some(&prev), "1.0", None, Some("HTTP 500"));
        assert_eq!(r.status, UpdateStatus::UpstreamError);
        assert_eq!(r.latest.as_deref(), Some("1.1"));
        assert_eq!(r.last_error.as_deref(), Some("HTTP 500"));
    }

    #[tokio::test]
    async fn success_records_proposed_version_and_rebuilds() {
        let db = setup_db().await;
        db.record_successful_update("", "hello", "1.0", "1.1", Some(42), Some("s1"))
            .await
            .unwrap();
        db.record_pr_info("", "hello", "https://example/pr/1", 1)
            .await
            .unwrap();

        let rec = db.get_update_record("", "hello").await.unwrap().unwrap();
        assert_eq!(rec.status, UpdateStatus::Proposed);
        assert_eq!(rec.current_version.as_deref(), Some("1.0"));
        assert_eq!(rec.proposed_version.as_deref(), Some("1.1"));
        assert_eq!(rec.latest_upstream_version.as_deref(), Some("1.1"));
        assert!(rec.in_backoff(Utc::now()));

        let (rebuilds, pr): (Option<i64>, Option<String>) =
            sqlx::query_as("SELECT rebuild_count, pr_url FROM updates WHERE attr_path = 'hello'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(rebuilds, Some(42));
        assert_eq!(pr.as_deref(), Some("https://example/pr/1"));

        // Checker sees the proposal is still pending merge: stays proposed.
        db.record_no_update("", "hello", "1.0", Some("1.1"), None)
            .await
            .unwrap();
        let rec = db.get_update_record("", "hello").await.unwrap().unwrap();
        assert_eq!(rec.status, UpdateStatus::Proposed);

        // After merge the tree reports 1.1: up to date, proposal cleared.
        db.record_no_update("", "hello", "1.1", Some("1.1"), None)
            .await
            .unwrap();
        let rec = db.get_update_record("", "hello").await.unwrap().unwrap();
        assert_eq!(rec.status, UpdateStatus::UpToDate);
        assert_eq!(rec.proposed_version, None);
    }

    #[tokio::test]
    async fn failures_step_backoff_and_are_repo_scoped() {
        let db = setup_db().await;
        let failure = FailedUpdate {
            repo_name: "a",
            attr_path: "hello",
            old_version: Some("1.0"),
            new_version: Some("1.1"),
            error_log: "line\nerror: builder failed\n",
            phase: Some("build"),
            error_type: Some("BuildError"),
            ..FailedUpdate::default()
        };
        let now = Utc::now();
        db.record_failed_update(&failure).await.unwrap();
        let rec = db.get_update_record("a", "hello").await.unwrap().unwrap();
        assert_eq!(rec.status, UpdateStatus::Failed);
        assert_eq!(rec.failure_count, 1);
        assert_eq!(rec.failed_version.as_deref(), Some("1.1"));
        assert_eq!(
            rec.last_error.as_deref(),
            Some("build: error: builder failed")
        );
        let days = (rec.next_attempt.unwrap() - now).num_hours();
        assert!((47..=48).contains(&days), "expected ~2 days, got {days}h");

        db.record_failed_update(&failure).await.unwrap();
        let rec = db.get_update_record("a", "hello").await.unwrap().unwrap();
        assert_eq!(rec.failure_count, 2);
        let hours = (rec.next_attempt.unwrap() - now).num_hours();
        assert!((95..=96).contains(&hours), "expected ~4 days, got {hours}h");

        // Other repos are unaffected.
        assert!(db.get_update_record("", "hello").await.unwrap().is_none());
        assert!(db.should_check_update("", "hello").await.unwrap());
        assert!(!db.should_check_update("a", "hello").await.unwrap());

        // Synthetic log key and metadata columns.
        let log = db.get_log_by_drv("a:hello@1.1").await.unwrap().unwrap();
        assert_eq!(log.repo_name, "a");
        assert_eq!(log.phase.as_deref(), Some("build"));
        assert_eq!(log.error_type.as_deref(), Some("BuildError"));

        // Success resets the failure counter.
        db.record_successful_update("a", "hello", "1.0", "1.1", None, None)
            .await
            .unwrap();
        let rec = db.get_update_record("a", "hello").await.unwrap().unwrap();
        assert_eq!(rec.failure_count, 0);
        assert_eq!(rec.failed_version, None);
    }

    #[tokio::test]
    async fn observation_sets_baseline_once() {
        let db = setup_db().await;
        db.upsert_package_info("w", "jq", "1.6", &PackageInfo::default())
            .await
            .unwrap();
        let rec = db.get_update_record("w", "jq").await.unwrap().unwrap();
        assert_eq!(rec.status, UpdateStatus::Pending);
        assert_eq!(rec.baseline_version, None);

        let prev = db
            .record_observation("w", "jq", "1.6", "1.7")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prev.baseline_version, None);

        let prev = db
            .record_observation("w", "jq", "1.6", "1.8")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prev.baseline_version.as_deref(), Some("1.7"));
        let rec = db.get_update_record("w", "jq").await.unwrap().unwrap();
        assert_eq!(rec.baseline_version.as_deref(), Some("1.7"));
        assert_eq!(rec.latest_upstream_version.as_deref(), Some("1.8"));
        assert_eq!(rec.status, UpdateStatus::Outdated);
        // Observations never set a backoff.
        assert!(rec.next_attempt.is_none());
    }

    #[tokio::test]
    async fn upstream_error_is_recorded() {
        let db = setup_db().await;
        db.record_no_update("", "foo", "1.0", None, Some("rate limited"))
            .await
            .unwrap();
        let rec = db.get_update_record("", "foo").await.unwrap().unwrap();
        assert_eq!(rec.status, UpdateStatus::UpstreamError);
        assert_eq!(rec.last_error.as_deref(), Some("rate limited"));
        assert!(!db.should_check_update("", "foo").await.unwrap());
    }
}
