//! Watch-mode bookkeeping: watched repositories (`watched_repos`), the
//! poll/index cycle log (`watch_cycles`) and per-source poll health
//! (`upstream_sources`).
//!
//! These tables are written by `ekapkgs-update watch` and read by the web
//! dashboard's Watch page.

use anyhow::{Context, Result};
use chrono::{Duration, Utc};

use super::Database;

/// Static configuration of a watched repository.
#[derive(Debug, Clone, Default)]
pub struct WatchedRepoInfo<'a> {
    /// `''` for legacy single-repo mode (current directory).
    pub name: &'a str,
    /// Upstream URL from the config (`None` in legacy mode).
    pub upstream: Option<&'a str>,
    /// Local checkout.
    pub path: &'a str,
    /// Absolute eval entry point.
    pub entrypoint: &'a str,
    pub base_branch: Option<&'a str>,
    pub fork_remote: Option<&'a str>,
}

/// Result of syncing a repository checkout with its upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoSyncStatus {
    /// Fetched and fast-forwarded (or already up to date).
    Ok,
    /// Local branch has diverged from upstream; the stale tree is kept.
    Diverged,
    /// Fetch or merge failed.
    Error,
    /// Not managed by watch (legacy mode): never fetched or merged.
    Unmanaged,
}

impl RepoSyncStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Diverged => "diverged",
            Self::Error => "error",
            Self::Unmanaged => "unmanaged",
        }
    }
}

/// Outcome of building a repository's upstream index.
#[derive(Debug, Clone, Default)]
pub struct IndexOutcome {
    /// `Some` when the index build failed (e.g. nix-eval-jobs crashed).
    pub error: Option<String>,
    pub duration_ms: i64,
    /// Derivations produced by nix-eval-jobs.
    pub evaluated: usize,
    /// Packages with a resolvable upstream source.
    pub indexed: usize,
    /// skip=true / no upstream / pinned / metadata errors.
    pub skipped: usize,
    /// Evaluation errors.
    pub errors: usize,
}

/// Kind of a `watch_cycles` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchCycleKind {
    Poll,
    Index,
}

impl WatchCycleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Poll => "poll",
            Self::Index => "index",
        }
    }
}

/// Counters written when a watch cycle finishes.
#[derive(Debug, Clone, Default)]
pub struct WatchCycleStats {
    /// Poll: at least one package baseline was recorded.
    pub bootstrap: bool,
    pub sources_polled: usize,
    pub sources_failed: usize,
    /// Poll: packages compared against upstream. Index: packages indexed.
    pub packages_observed: usize,
    /// Poll: sources whose latest version changed.
    pub new_releases: usize,
    pub dispatched: usize,
    pub updated: usize,
    pub failed: usize,
    /// Poll: session created because updates were dispatched.
    pub session_id: Option<String>,
}

/// Result of polling one upstream source.
#[derive(Debug, Clone)]
pub enum SourcePoll<'a> {
    Success {
        /// Newest stable release (ignoring per-package strategies), if any.
        latest_version: Option<&'a str>,
        latest_tag: Option<&'a str>,
        release_count: usize,
    },
    Failure {
        error: &'a str,
    },
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl Database {
    // ── watched_repos ────────────────────────────────────────────────

    /// Insert or refresh the static configuration of a watched repository.
    /// Sync/index state columns are left untouched.
    pub async fn upsert_watched_repo(&self, repo: &WatchedRepoInfo<'_>) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO watched_repos
                (name, upstream, path, entrypoint, base_branch, fork_remote, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(name) DO UPDATE SET
                upstream = excluded.upstream,
                path = excluded.path,
                entrypoint = excluded.entrypoint,
                base_branch = excluded.base_branch,
                fork_remote = excluded.fork_remote,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(repo.name)
        .bind(repo.upstream)
        .bind(repo.path)
        .bind(repo.entrypoint)
        .bind(repo.base_branch)
        .bind(repo.fork_remote)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .with_context(|| format!("upsert watched repo '{}'", repo.name))?;
        Ok(())
    }

    /// Forget repositories that are no longer configured: their
    /// `watched_repos` rows and `upstream_index` entries are deleted (their
    /// `updates` history is kept). Returns the number of repos removed.
    pub async fn retain_watched_repos(&self, names: &[&str]) -> Result<u64> {
        let keep = serde_json::to_string(names).context("encode repo names")?;
        let removed = sqlx::query(
            "DELETE FROM watched_repos WHERE name NOT IN (SELECT value FROM json_each(?))",
        )
        .bind(&keep)
        .execute(&self.pool)
        .await
        .context("prune watched repos")?;
        sqlx::query(
            "DELETE FROM upstream_index WHERE repo_name NOT IN (SELECT value FROM json_each(?))",
        )
        .bind(&keep)
        .execute(&self.pool)
        .await
        .context("prune upstream index of removed repos")?;
        Ok(removed.rows_affected())
    }

    /// Record the result of syncing a repository checkout.
    pub async fn record_repo_sync(
        &self,
        name: &str,
        status: RepoSyncStatus,
        error: Option<&str>,
        head_commit: Option<&str>,
        head_subject: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            UPDATE watched_repos SET
                sync_status = ?,
                sync_error = ?,
                head_commit = COALESCE(?, head_commit),
                head_subject = COALESCE(?, head_subject),
                last_synced_at = ?,
                updated_at = ?
            WHERE name = ?
            "#,
        )
        .bind(status.as_str())
        .bind(error)
        .bind(head_commit)
        .bind(head_subject)
        .bind(&now)
        .bind(&now)
        .bind(name)
        .execute(&self.pool)
        .await
        .with_context(|| format!("record sync of watched repo '{name}'"))?;
        Ok(())
    }

    /// Mark a repository as currently being indexed.
    pub async fn record_repo_index_start(&self, name: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE watched_repos SET index_status = 'indexing', index_error = NULL, \
             updated_at = ? WHERE name = ?",
        )
        .bind(&now)
        .bind(name)
        .execute(&self.pool)
        .await
        .with_context(|| format!("record index start of watched repo '{name}'"))?;
        Ok(())
    }

    /// Record the result of an index build.
    pub async fn record_repo_index_result(&self, name: &str, outcome: &IndexOutcome) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let status = if outcome.error.is_some() {
            "error"
        } else {
            "ok"
        };
        sqlx::query(
            r#"
            UPDATE watched_repos SET
                index_status = ?,
                index_error = ?,
                last_indexed_at = ?,
                index_duration_ms = ?,
                evaluated_count = ?,
                indexed_count = ?,
                index_skipped = ?,
                index_errors = ?,
                updated_at = ?
            WHERE name = ?
            "#,
        )
        .bind(status)
        .bind(outcome.error.as_deref())
        .bind(&now)
        .bind(outcome.duration_ms)
        .bind(to_i64(outcome.evaluated))
        .bind(to_i64(outcome.indexed))
        .bind(to_i64(outcome.skipped))
        .bind(to_i64(outcome.errors))
        .bind(&now)
        .bind(name)
        .execute(&self.pool)
        .await
        .with_context(|| format!("record index result of watched repo '{name}'"))?;
        Ok(())
    }

    /// Record the first completed poll for a repository (baselines recorded).
    /// Later calls keep the original timestamp.
    pub async fn mark_repo_bootstrapped(&self, name: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE watched_repos SET bootstrapped_at = COALESCE(bootstrapped_at, ?), \
             updated_at = ? WHERE name = ?",
        )
        .bind(&now)
        .bind(&now)
        .bind(name)
        .execute(&self.pool)
        .await
        .with_context(|| format!("mark watched repo '{name}' bootstrapped"))?;
        Ok(())
    }

    /// Close out state left behind by a watch process that was killed:
    /// running cycles become failed, repos stuck in 'indexing' become
    /// 'error' and running watch sessions are cancelled. Returns the number
    /// of cycles closed.
    pub async fn recover_interrupted_watch_state(&self) -> Result<u64> {
        let now = Utc::now().to_rfc3339();
        let cycles = sqlx::query(
            "UPDATE watch_cycles SET status = 'failed', completed_at = ?, \
             error = COALESCE(error, 'interrupted') WHERE status = 'running'",
        )
        .bind(&now)
        .execute(&self.pool)
        .await
        .context("close interrupted watch cycles")?;
        sqlx::query(
            "UPDATE watched_repos SET index_status = 'error', \
             index_error = COALESCE(index_error, 'interrupted'), updated_at = ? \
             WHERE index_status = 'indexing'",
        )
        .bind(&now)
        .execute(&self.pool)
        .await
        .context("reset interrupted repo indexing")?;
        sqlx::query(
            "UPDATE update_phases SET status = 'skipped', completed_at = ? \
             WHERE status = 'running' AND session_id IN \
             (SELECT id FROM update_sessions WHERE mode = 'watch' AND status = 'running')",
        )
        .bind(&now)
        .execute(&self.pool)
        .await
        .context("close interrupted watch phases")?;
        sqlx::query(
            "UPDATE update_sessions SET status = 'cancelled', completed_at = ?, updated_at = ? \
             WHERE mode = 'watch' AND status = 'running'",
        )
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .context("cancel interrupted watch sessions")?;
        Ok(cycles.rows_affected())
    }

    // ── watch_cycles ─────────────────────────────────────────────────

    /// Start a poll or index cycle. Returns the cycle id.
    pub async fn start_watch_cycle(&self, kind: WatchCycleKind, repo: Option<&str>) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let id = sqlx::query(
            "INSERT INTO watch_cycles (kind, repo_name, started_at, status) \
             VALUES (?, ?, ?, 'running')",
        )
        .bind(kind.as_str())
        .bind(repo)
        .bind(&now)
        .execute(&self.pool)
        .await
        .context("start watch cycle")?
        .last_insert_rowid();
        Ok(id)
    }

    /// Finish a cycle. `error = Some` marks it failed.
    pub async fn finish_watch_cycle(
        &self,
        id: i64,
        stats: &WatchCycleStats,
        error: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now();
        let status = if error.is_some() {
            "failed"
        } else {
            "completed"
        };
        sqlx::query(
            r#"
            UPDATE watch_cycles SET
                completed_at = ?,
                duration_ms = CAST(ROUND((julianday(?) - julianday(started_at)) * 86400000) AS INTEGER),
                status = ?,
                bootstrap = ?,
                sources_polled = ?,
                sources_failed = ?,
                packages_observed = ?,
                new_releases = ?,
                dispatched = ?,
                updated = ?,
                failed = ?,
                session_id = ?,
                error = ?
            WHERE id = ?
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(status)
        .bind(stats.bootstrap)
        .bind(to_i64(stats.sources_polled))
        .bind(to_i64(stats.sources_failed))
        .bind(to_i64(stats.packages_observed))
        .bind(to_i64(stats.new_releases))
        .bind(to_i64(stats.dispatched))
        .bind(to_i64(stats.updated))
        .bind(to_i64(stats.failed))
        .bind(stats.session_id.as_deref())
        .bind(error)
        .bind(id)
        .execute(&self.pool)
        .await
        .with_context(|| format!("finish watch cycle {id}"))?;
        Ok(())
    }

    /// Delete uneventful poll cycles (nothing dispatched, no error) older
    /// than `keep_days`. Index cycles and cycles that did work are kept.
    pub async fn prune_watch_cycles(&self, keep_days: i64) -> Result<u64> {
        let cutoff = (Utc::now() - Duration::days(keep_days)).to_rfc3339();
        let result = sqlx::query(
            "DELETE FROM watch_cycles WHERE kind = 'poll' AND status = 'completed' \
             AND dispatched = 0 AND bootstrap = 0 AND started_at < ?",
        )
        .bind(&cutoff)
        .execute(&self.pool)
        .await
        .context("prune watch cycles")?;
        Ok(result.rows_affected())
    }

    // ── upstream_sources ─────────────────────────────────────────────

    /// Record the result of polling an upstream source.
    ///
    /// Returns `true` when a successful poll found a different latest version
    /// than the one previously recorded (the first observation of a source
    /// is a baseline, not a new release).
    pub async fn record_source_poll(
        &self,
        upstream_type: &str,
        upstream_key: &str,
        instance: Option<&str>,
        poll: &SourcePoll<'_>,
    ) -> Result<bool> {
        let instance = instance.unwrap_or("");
        let now = Utc::now().to_rfc3339();
        let previous: Option<Option<String>> = sqlx::query_scalar(
            "SELECT latest_version FROM upstream_sources \
             WHERE upstream_type = ? AND upstream_key = ? AND instance = ?",
        )
        .bind(upstream_type)
        .bind(upstream_key)
        .bind(instance)
        .fetch_optional(&self.pool)
        .await
        .context("read upstream source")?;
        let previous = previous.flatten();

        match *poll {
            SourcePoll::Success {
                latest_version,
                latest_tag,
                release_count,
            } => {
                sqlx::query(
                    r#"
                    INSERT INTO upstream_sources
                        (upstream_type, upstream_key, instance, latest_version, latest_tag,
                         release_count, latest_changed_at, first_seen_at, last_polled_at,
                         last_success_at, last_error, consecutive_failures)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 0)
                    ON CONFLICT(upstream_type, upstream_key, instance) DO UPDATE SET
                        latest_changed_at = CASE
                            WHEN excluded.latest_version IS NULL
                                 OR upstream_sources.latest_version IS excluded.latest_version
                            THEN upstream_sources.latest_changed_at
                            ELSE excluded.latest_changed_at
                        END,
                        latest_version = COALESCE(excluded.latest_version, upstream_sources.latest_version),
                        latest_tag = COALESCE(excluded.latest_tag, upstream_sources.latest_tag),
                        release_count = excluded.release_count,
                        last_polled_at = excluded.last_polled_at,
                        last_success_at = excluded.last_success_at,
                        last_error = NULL,
                        consecutive_failures = 0
                    "#,
                )
                .bind(upstream_type)
                .bind(upstream_key)
                .bind(instance)
                .bind(latest_version)
                .bind(latest_tag)
                .bind(to_i64(release_count))
                .bind(latest_version.map(|_| now.as_str()))
                .bind(&now)
                .bind(&now)
                .bind(&now)
                .execute(&self.pool)
                .await
                .context("record successful source poll")?;

                Ok(matches!(
                    (previous.as_deref(), latest_version),
                    (Some(prev), Some(new)) if prev != new
                ))
            },
            SourcePoll::Failure { error } => {
                sqlx::query(
                    r#"
                    INSERT INTO upstream_sources
                        (upstream_type, upstream_key, instance, first_seen_at, last_polled_at,
                         last_error, consecutive_failures)
                    VALUES (?, ?, ?, ?, ?, ?, 1)
                    ON CONFLICT(upstream_type, upstream_key, instance) DO UPDATE SET
                        last_polled_at = excluded.last_polled_at,
                        last_error = excluded.last_error,
                        consecutive_failures = upstream_sources.consecutive_failures + 1
                    "#,
                )
                .bind(upstream_type)
                .bind(upstream_key)
                .bind(instance)
                .bind(&now)
                .bind(&now)
                .bind(error)
                .execute(&self.pool)
                .await
                .context("record failed source poll")?;
                Ok(false)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Row;

    use super::*;

    async fn setup_db() -> Database {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        Database::from(pool)
    }

    #[tokio::test]
    async fn watched_repo_lifecycle() {
        let db = setup_db().await;
        let info = WatchedRepoInfo {
            name: "core",
            upstream: Some("https://github.com/o/r"),
            path: "/var/lib/repos/core",
            entrypoint: "/var/lib/repos/core/default.nix",
            base_branch: Some("master"),
            fork_remote: Some("fork"),
        };
        db.upsert_watched_repo(&info).await.unwrap();
        db.upsert_watched_repo(&info).await.unwrap();
        db.record_repo_sync(
            "core",
            RepoSyncStatus::Diverged,
            Some("not a fast-forward"),
            Some("abc123"),
            Some("subject"),
        )
        .await
        .unwrap();
        db.record_repo_index_start("core").await.unwrap();
        db.record_repo_index_result(
            "core",
            &IndexOutcome {
                error: None,
                duration_ms: 1500,
                evaluated: 10,
                indexed: 7,
                skipped: 2,
                errors: 1,
            },
        )
        .await
        .unwrap();
        db.mark_repo_bootstrapped("core").await.unwrap();

        let row = sqlx::query("SELECT * FROM watched_repos WHERE name = 'core'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("sync_status"), "diverged");
        assert_eq!(row.get::<String, _>("head_commit"), "abc123");
        assert_eq!(row.get::<String, _>("index_status"), "ok");
        assert_eq!(row.get::<i64, _>("indexed_count"), 7);
        assert_eq!(row.get::<i64, _>("index_errors"), 1);
        let first: String = row.get("bootstrapped_at");

        // Removing a repo from the config forgets it and its index.
        db.upsert_indexed_package(&crate::database::IndexedPackage {
            repo_name: "core",
            attr_path: "hello",
            upstream_type: "github",
            upstream_key: "o/hello",
            current_version: "1.0",
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(db.retain_watched_repos(&["core"]).await.unwrap(), 0);
        assert_eq!(db.upstream_index_count(Some("core")).await.unwrap(), 1);

        db.mark_repo_bootstrapped("core").await.unwrap();
        let again: String =
            sqlx::query_scalar("SELECT bootstrapped_at FROM watched_repos WHERE name = 'core'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(first, again);

        assert_eq!(db.retain_watched_repos(&["other"]).await.unwrap(), 1);
        assert_eq!(db.upstream_index_count(Some("core")).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn cycles_record_and_prune() {
        let db = setup_db().await;
        let busy = db
            .start_watch_cycle(WatchCycleKind::Poll, None)
            .await
            .unwrap();
        let idle = db
            .start_watch_cycle(WatchCycleKind::Poll, None)
            .await
            .unwrap();
        let index = db
            .start_watch_cycle(WatchCycleKind::Index, Some("core"))
            .await
            .unwrap();
        db.finish_watch_cycle(
            busy,
            &WatchCycleStats {
                sources_polled: 3,
                dispatched: 1,
                updated: 1,
                session_id: Some("s1".into()),
                ..WatchCycleStats::default()
            },
            None,
        )
        .await
        .unwrap();
        db.finish_watch_cycle(idle, &WatchCycleStats::default(), None)
            .await
            .unwrap();

        // `index` is still running; recovering closes it out.
        assert_eq!(db.recover_interrupted_watch_state().await.unwrap(), 1);
        let status: String = sqlx::query_scalar("SELECT status FROM watch_cycles WHERE id = ?")
            .bind(index)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(status, "failed");

        // Nothing is older than the cutoff yet.
        assert_eq!(db.prune_watch_cycles(1).await.unwrap(), 0);
        // With a cutoff in the future only the idle poll cycle goes.
        assert_eq!(db.prune_watch_cycles(-1).await.unwrap(), 1);
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM watch_cycles")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(remaining, 2);

        let busy_row = sqlx::query("SELECT * FROM watch_cycles WHERE id = ?")
            .bind(busy)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(busy_row.get::<String, _>("status"), "completed");
        assert_eq!(busy_row.get::<i64, _>("dispatched"), 1);
        assert_eq!(busy_row.get::<String, _>("session_id"), "s1");
        assert!(busy_row.get::<Option<i64>, _>("duration_ms").is_some());
    }

    #[tokio::test]
    async fn source_poll_detects_changes() {
        let db = setup_db().await;
        let ok = |v: &'static str| SourcePoll::Success {
            latest_version: Some(v),
            latest_tag: Some(v),
            release_count: 5,
        };

        // First observation is a baseline, not a new release.
        assert!(
            !db.record_source_poll("github", "o/r", None, &ok("1.0"))
                .await
                .unwrap()
        );
        assert!(
            !db.record_source_poll("github", "o/r", None, &ok("1.0"))
                .await
                .unwrap()
        );
        // Failures do not lose the known version.
        assert!(
            !db.record_source_poll("github", "o/r", None, &SourcePoll::Failure { error: "503" })
                .await
                .unwrap()
        );
        assert!(
            !db.record_source_poll("github", "o/r", None, &SourcePoll::Failure { error: "503" })
                .await
                .unwrap()
        );
        let row = sqlx::query("SELECT * FROM upstream_sources WHERE upstream_key = 'o/r'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(row.get::<i64, _>("consecutive_failures"), 2);
        assert_eq!(row.get::<String, _>("last_error"), "503");
        assert_eq!(row.get::<String, _>("latest_version"), "1.0");

        assert!(
            db.record_source_poll("github", "o/r", None, &ok("1.1"))
                .await
                .unwrap()
        );
        let row = sqlx::query("SELECT * FROM upstream_sources WHERE upstream_key = 'o/r'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(row.get::<i64, _>("consecutive_failures"), 0);
        assert_eq!(row.get::<Option<String>, _>("last_error"), None);
        assert_eq!(row.get::<String, _>("latest_version"), "1.1");

        // Instances are distinct sources.
        assert!(
            !db.record_source_poll("gitlab", "o/r", Some("gitlab.gnome.org"), &ok("2.0"))
                .await
                .unwrap()
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM upstream_sources")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 2);
    }
}
