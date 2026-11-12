//! SQLite database interface for tracking package update attempts and history.
//!
//! ## Schema
//!
//! - **`updates`**: Per-package state, keyed by `(repo_name, attr_path)`.
//!   `repo_name` is `''` for the `run` command and legacy single-repo watch
//!   mode. Holds the lifecycle [`UpdateStatus`], current/latest/proposed
//!   versions, the watch bootstrap `baseline_version`, failure bookkeeping,
//!   backoff timestamps, PR details and cached package metadata.
//! - **`update_logs`**: Full error logs of failed attempts, keyed by derivation
//!   path (or a synthetic `<repo>:<attr>@<version>` key).
//! - **`update_sessions`** / **`update_phases`**: One session per `run` (or per
//!   watch poll cycle that dispatched updates) and one row per update phase,
//!   with live counters (see [`sessions`]).
//! - **`watched_repos`**, **`watch_cycles`**, **`upstream_sources`**: Watch
//!   mode repository state, poll/index cycle log and per-source poll health
//!   (see [`watch`]).
//! - **`upstream_index`**: Reverse index from upstream source to attr_path.
//!
//! ## Backoff Semantics
//!
//! Checks that find nothing to do back off 2 days → 4 days → 6 days (max).
//! Failed attempts back off by consecutive failure count: 2 days → 4 days →
//! 6 days (max). Successful updates reset the backoff to 2 days and clear the
//! failure counter.
//!
//! ## Connection Pool and Cleanup
//!
//! [`Database::new`] establishes a connection pool and runs schema migrations via
//! `sqlx::migrate!`. After migrations, it opportunistically cleans up expired cache
//! entries for both CVE and Repology data by calling their respective `cleanup_cache`
//! functions. Cleanup failures are logged but do not fail database initialization.
//!
//! The connection pool remains alive as long as the `Database` struct is in scope.
//! `Database` is `Clone`, so multiple handles can share the same underlying pool.

mod sessions;
mod types;
mod updates;
mod watch;

use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
#[allow(unused_imports)] // Some re-exports are only used by ekapkgs-update-web / tests
pub use sessions::{PhaseRecord, SessionCounter, SessionMode, SessionStatus, UpdateSession};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use tracing::{debug, info};
#[allow(unused_imports)] // Some re-exports are only used by ekapkgs-update-web / tests
pub use types::{FailedUpdate, PackageInfo, UpdateLog, UpdateRecord, UpdateStatus};
#[allow(unused_imports)] // Some re-exports are only used by ekapkgs-update-web / tests
pub use watch::{
    IndexOutcome, RepoSyncStatus, SourcePoll, WatchCycleKind, WatchCycleStats, WatchedRepoInfo,
};

/// Parse an optional RFC 3339 timestamp string into UTC, ignoring parse errors.
///
/// Used by row-mapping code where invalid stored timestamps degrade to `None`
/// rather than aborting record loading.
fn parse_rfc3339(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// First backoff step (days): first check, or first failed attempt.
const FIRST_BACKOFF_DAYS: i64 = 2;
/// Second backoff step (days): re-check shortly after the previous window,
/// or second consecutive failure.
const SECOND_BACKOFF_DAYS: i64 = 4;
/// Maximum backoff (days).
const MAX_BACKOFF_DAYS: i64 = 6;

/// Upper bound (inclusive) of the "small" rebuild bucket.
const REBUILD_BUCKET_SMALL_MAX: i64 = 10;
/// Upper bound (inclusive) of the "medium" rebuild bucket.
const REBUILD_BUCKET_MEDIUM_MAX: i64 = 50;
/// Upper bound (inclusive) of the "large" rebuild bucket.
const REBUILD_BUCKET_LARGE_MAX: i64 = 100;

/// Histogram bucket for `rebuild_count` distributions.
///
/// The label values returned by [`RebuildBucket::label`] match exactly the
/// strings the rebuild-distribution SQL `CASE` expression emits, so they are
/// also used as map keys when consumers need a stable ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RebuildBucket {
    /// `0..=REBUILD_BUCKET_SMALL_MAX`
    Small,
    /// `(REBUILD_BUCKET_SMALL_MAX..=REBUILD_BUCKET_MEDIUM_MAX]`
    Medium,
    /// `(REBUILD_BUCKET_MEDIUM_MAX..=REBUILD_BUCKET_LARGE_MAX]`
    Large,
    /// `> REBUILD_BUCKET_LARGE_MAX`
    Huge,
}

impl RebuildBucket {
    /// Render the human-readable bucket label (e.g. `"0-10"`, `"101+"`).
    ///
    /// The string matches the SQL `CASE` branches in
    /// [`Database::get_rebuild_distribution`].
    pub fn label(self) -> String {
        match self {
            RebuildBucket::Small => format!("0-{REBUILD_BUCKET_SMALL_MAX}"),
            RebuildBucket::Medium => format!(
                "{}-{REBUILD_BUCKET_MEDIUM_MAX}",
                REBUILD_BUCKET_SMALL_MAX + 1
            ),
            RebuildBucket::Large => format!(
                "{}-{REBUILD_BUCKET_LARGE_MAX}",
                REBUILD_BUCKET_MEDIUM_MAX + 1
            ),
            RebuildBucket::Huge => format!("{}+", REBUILD_BUCKET_LARGE_MAX + 1),
        }
    }
}

impl std::fmt::Display for RebuildBucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// Database connection wrapper for tracking package updates
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl From<SqlitePool> for Database {
    /// Construct a `Database` from a pre-built `SqlitePool`.
    ///
    /// This is primarily useful for tests, which want to bind to
    /// `SqlitePool::connect("sqlite::memory:")` and skip
    /// [`Database::new`]'s schema-bootstrap steps when the test fixture
    /// has already run them.
    fn from(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl Database {
    /// Initialize the database connection and create tables if needed
    pub async fn new(db_path: &str) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = Path::new(db_path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }

        // Create connection options
        let options = SqliteConnectOptions::from_str(db_path)
            .with_context(|| format!("parse sqlite URL {db_path}"))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        // Connect to database
        let pool = SqlitePool::connect_with(options)
            .await
            .with_context(|| format!("connect to sqlite database {db_path}"))?;

        info!("Connected to database at {}", db_path);

        // Run migrations
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("Failed to run database migrations")?;

        debug!("Database migrations completed");

        // Cleanup expired CVE cache entries
        if let Err(e) = crate::cve::cleanup_cache(&pool).await {
            debug!("Failed to cleanup CVE cache: {}", e);
        }

        // Cleanup expired Repology cache entries
        if let Err(e) = crate::repology::cleanup_cache(&pool).await {
            debug!("Failed to cleanup Repology cache: {}", e);
        }

        Ok(Self { pool })
    }

    /// Get a reference to the database connection pool
    ///
    /// This is used by other modules (like CVE) that need direct database access
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Get rebuild count for a specific package (most recent update)
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_rebuild_count(&self, attr_path: &str) -> Result<Option<i64>> {
        let count: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT rebuild_count
            FROM updates
            WHERE attr_path = ?
            "#,
        )
        .bind(attr_path)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("get rebuild count for {attr_path}"))?
        .flatten();

        Ok(count)
    }

    /// Get packages with highest rebuild impact
    ///
    /// Returns packages sorted by rebuild count (descending), including only
    /// those with non-null rebuild counts.
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_high_impact_packages(&self, limit: i64) -> Result<Vec<(String, i64)>> {
        let results: Vec<(String, i64)> = sqlx::query_as(
            r#"
            SELECT attr_path, rebuild_count
            FROM updates
            WHERE rebuild_count IS NOT NULL
            ORDER BY rebuild_count DESC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("get high impact packages")?;

        Ok(results)
    }

    /// Get average rebuild count across all packages
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_average_rebuild_count(&self) -> Result<Option<f64>> {
        let avg: Option<f64> = sqlx::query_scalar(
            r#"
            SELECT AVG(rebuild_count)
            FROM updates
            WHERE rebuild_count IS NOT NULL
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .context("get average rebuild count")?
        .flatten();

        Ok(avg)
    }

    /// Get rebuild count distribution (histogram buckets)
    ///
    /// Returns counts of packages in different rebuild impact ranges:
    /// - 0-10 rebuilds
    /// - 11-50 rebuilds
    /// - 51-100 rebuilds
    /// - 101+ rebuilds
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_rebuild_distribution(&self) -> Result<Vec<(String, i64)>> {
        // Build SQL with bucket boundaries from named constants. The
        // bucket-label strings in the inner and outer `CASE` must match,
        // so we render them once via `RebuildBucket::label`.
        let small_label = RebuildBucket::Small.label();
        let medium_label = RebuildBucket::Medium.label();
        let large_label = RebuildBucket::Large.label();
        let huge_label = RebuildBucket::Huge.label();
        let sql = format!(
            r#"
            SELECT
                CASE
                    WHEN rebuild_count <= {small_max} THEN '{small_label}'
                    WHEN rebuild_count <= {medium_max} THEN '{medium_label}'
                    WHEN rebuild_count <= {large_max} THEN '{large_label}'
                    ELSE '{huge_label}'
                END as range,
                COUNT(*) as count
            FROM updates
            WHERE rebuild_count IS NOT NULL
            GROUP BY range
            ORDER BY
                CASE range
                    WHEN '{small_label}' THEN 1
                    WHEN '{medium_label}' THEN 2
                    WHEN '{large_label}' THEN 3
                    ELSE 4
                END
            "#,
            small_max = REBUILD_BUCKET_SMALL_MAX,
            medium_max = REBUILD_BUCKET_MEDIUM_MAX,
            large_max = REBUILD_BUCKET_LARGE_MAX,
        );
        let results: Vec<(String, i64)> = sqlx::query_as(&sql)
            .fetch_all(&self.pool)
            .await
            .context("get rebuild distribution")?;

        Ok(results)
    }

    /// Get packages with rebuild counts in a specific range
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_packages_by_rebuild_range(
        &self,
        min: i64,
        max: i64,
    ) -> Result<Vec<(String, i64, String)>> {
        let results: Vec<(String, i64, String)> = sqlx::query_as(
            r#"
            SELECT attr_path, rebuild_count, current_version
            FROM updates
            WHERE rebuild_count IS NOT NULL
                AND rebuild_count >= ?
                AND rebuild_count <= ?
            ORDER BY rebuild_count DESC
            "#,
        )
        .bind(min)
        .bind(max)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("get packages by rebuild range [{min}, {max}]"))?;

        Ok(results)
    }

    /// Get total number of packages with rebuild count data
    #[allow(dead_code)] // planned API: reporting/stats commands
    pub async fn get_packages_with_rebuild_data_count(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM updates
            WHERE rebuild_count IS NOT NULL
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .context("count packages with rebuild data")?;

        Ok(count)
    }

    // ── Upstream index (watch command) ──────────────────────────────

    /// Insert or update an upstream index entry mapping an attr_path to its
    /// upstream source (without version-selection settings).
    #[allow(dead_code)] // used by integration tests; the indexer uses upsert_indexed_package
    pub async fn upsert_upstream_index(
        &self,
        repo_name: &str,
        attr_path: &str,
        upstream_type: &str,
        upstream_key: &str,
        instance: Option<&str>,
        current_version: &str,
        src_url: Option<&str>,
    ) -> Result<()> {
        self.upsert_indexed_package(&IndexedPackage {
            repo_name,
            attr_path,
            upstream_type,
            upstream_key,
            instance,
            current_version,
            src_url,
            ..IndexedPackage::default()
        })
        .await
    }

    /// Insert or update an upstream index entry, including the per-package
    /// version-selection settings used by the watch poller.
    pub async fn upsert_indexed_package(&self, pkg: &IndexedPackage<'_>) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO upstream_index
                (repo_name, attr_path, upstream_type, upstream_key, instance, current_version,
                 src_url, indexed_at, pname, semver_strategy, version_regex, include_prereleases)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(repo_name, attr_path, upstream_type, upstream_key)
            DO UPDATE SET
                instance = excluded.instance,
                current_version = excluded.current_version,
                src_url = excluded.src_url,
                indexed_at = excluded.indexed_at,
                pname = excluded.pname,
                semver_strategy = excluded.semver_strategy,
                version_regex = excluded.version_regex,
                include_prereleases = excluded.include_prereleases
            "#,
        )
        .bind(pkg.repo_name)
        .bind(pkg.attr_path)
        .bind(pkg.upstream_type)
        .bind(pkg.upstream_key)
        .bind(pkg.instance)
        .bind(pkg.current_version)
        .bind(pkg.src_url)
        .bind(&now)
        .bind(pkg.pname)
        .bind(pkg.semver_strategy)
        .bind(pkg.version_regex)
        .bind(pkg.include_prereleases)
        .execute(&self.pool)
        .await
        .context("upsert upstream_index")?;

        Ok(())
    }

    /// Look up all attr_paths that track a given upstream source.
    pub async fn get_attr_paths_for_source(
        &self,
        upstream_type: &str,
        upstream_key: &str,
        instance: Option<&str>,
    ) -> Result<Vec<UpstreamIndexEntry>> {
        let rows = sqlx::query(
            r#"
            SELECT repo_name, attr_path, upstream_type, upstream_key, instance, current_version,
                   src_url, pname, semver_strategy, version_regex, include_prereleases
            FROM upstream_index
            WHERE upstream_type = ? AND upstream_key = ?
              AND (instance IS ? OR (instance IS NOT NULL AND instance = ?))
            ORDER BY repo_name, attr_path
            "#,
        )
        .bind(upstream_type)
        .bind(upstream_key)
        .bind(instance)
        .bind(instance)
        .fetch_all(&self.pool)
        .await
        .context("get attr_paths for source")?;

        Ok(rows.iter().map(UpstreamIndexEntry::from_row).collect())
    }

    /// Get all unique upstream sources from the index, for feed subscription.
    pub async fn get_all_upstream_sources(&self) -> Result<Vec<UpstreamIndexEntry>> {
        let rows = sqlx::query(
            r#"
            SELECT upstream_type, upstream_key, instance,
                   MIN(repo_name) AS repo_name, MIN(attr_path) AS attr_path,
                   MIN(current_version) AS current_version,
                   MIN(src_url) AS src_url, MIN(pname) AS pname,
                   MIN(semver_strategy) AS semver_strategy,
                   MIN(version_regex) AS version_regex,
                   MAX(include_prereleases) AS include_prereleases
            FROM upstream_index
            GROUP BY upstream_type, upstream_key, instance
            ORDER BY upstream_type, upstream_key
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("get all upstream sources")?;

        Ok(rows.iter().map(UpstreamIndexEntry::from_row).collect())
    }

    /// Remove entries from the upstream index before a re-index.
    ///
    /// When `repo_name` is `Some`, only entries for that repo are cleared.
    /// When `None`, all entries are cleared (full re-index).
    #[allow(dead_code)] // the indexer prunes stale rows instead; kept for tooling/tests
    pub async fn clear_upstream_index(&self, repo_name: Option<&str>) -> Result<()> {
        match repo_name {
            Some(name) => {
                sqlx::query("DELETE FROM upstream_index WHERE repo_name = ?")
                    .bind(name)
                    .execute(&self.pool)
                    .await
                    .context("clear upstream_index for repo")?;
            },
            None => {
                sqlx::query("DELETE FROM upstream_index")
                    .execute(&self.pool)
                    .await
                    .context("clear upstream_index")?;
            },
        }
        Ok(())
    }

    /// Delete a repo's index entries that were not refreshed since `since`
    /// (i.e. packages that disappeared from the repo). Returns rows removed.
    pub async fn prune_upstream_index(&self, repo_name: &str, since: &str) -> Result<u64> {
        let result =
            sqlx::query("DELETE FROM upstream_index WHERE repo_name = ? AND indexed_at < ?")
                .bind(repo_name)
                .bind(since)
                .execute(&self.pool)
                .await
                .context("prune upstream_index")?;
        Ok(result.rows_affected())
    }

    /// Count entries in the upstream index (all repos, or one repo).
    pub async fn upstream_index_count(&self, repo_name: Option<&str>) -> Result<i64> {
        let count: i64 = match repo_name {
            Some(name) => {
                sqlx::query_scalar("SELECT COUNT(*) FROM upstream_index WHERE repo_name = ?")
                    .bind(name)
                    .fetch_one(&self.pool)
                    .await
            },
            None => {
                sqlx::query_scalar("SELECT COUNT(*) FROM upstream_index")
                    .fetch_one(&self.pool)
                    .await
            },
        }
        .context("count upstream_index")?;
        Ok(count)
    }
}

/// A package to write into the `upstream_index` table.
#[derive(Debug, Clone, Default)]
pub struct IndexedPackage<'a> {
    pub repo_name: &'a str,
    pub attr_path: &'a str,
    pub upstream_type: &'a str,
    pub upstream_key: &'a str,
    pub instance: Option<&'a str>,
    pub current_version: &'a str,
    pub src_url: Option<&'a str>,
    pub pname: Option<&'a str>,
    /// Effective semver strategy (`SemverStrategy` display form).
    pub semver_strategy: Option<&'a str>,
    pub version_regex: Option<&'a str>,
    pub include_prereleases: bool,
}

/// A row from the `upstream_index` table.
#[derive(Debug, Clone)]
pub struct UpstreamIndexEntry {
    /// Name of the source repository (empty string for single-repo legacy mode).
    pub repo_name: String,
    pub attr_path: String,
    pub upstream_type: String,
    pub upstream_key: String,
    pub instance: Option<String>,
    pub current_version: Option<String>,
    #[allow(dead_code)] // informational; read by tests and library users
    pub src_url: Option<String>,
    #[allow(dead_code)] // informational; read by tests and library users
    pub pname: Option<String>,
    /// Effective semver strategy recorded at index time (`None` = latest).
    pub semver_strategy: Option<String>,
    pub version_regex: Option<String>,
    pub include_prereleases: bool,
}

impl UpstreamIndexEntry {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Self {
        Self {
            repo_name: row.get("repo_name"),
            attr_path: row.get("attr_path"),
            upstream_type: row.get("upstream_type"),
            upstream_key: row.get("upstream_key"),
            instance: row.get("instance"),
            current_version: row.get("current_version"),
            src_url: row.get("src_url"),
            pname: row.get("pname"),
            semver_strategy: row.get("semver_strategy"),
            version_regex: row.get("version_regex"),
            include_prereleases: row
                .get::<Option<i64>, _>("include_prereleases")
                .unwrap_or(0)
                != 0,
        }
    }

    /// The effective semver strategy for release selection.
    pub fn strategy(&self) -> crate::vcs_sources::SemverStrategy {
        self.semver_strategy
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(crate::vcs_sources::SemverStrategy::Latest)
    }
}
