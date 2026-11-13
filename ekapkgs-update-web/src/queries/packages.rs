//! `updates` table: package list, filters and per-package detail.

use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use super::failures::LogRow;
use super::sessions::PhaseRow;
use super::watch::SourceRow;
use super::{PACKAGE_STATUSES, like_pattern};

/// One row of `updates`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct PackageRow {
    pub repo_name: String,
    pub attr_path: String,
    pub status: String,
    pub current_version: Option<String>,
    pub latest_upstream_version: Option<String>,
    pub proposed_version: Option<String>,
    pub baseline_version: Option<String>,
    pub failed_version: Option<String>,
    pub failure_count: i64,
    pub last_error: Option<String>,
    pub last_session_id: Option<String>,
    pub last_checked: Option<String>,
    pub last_attempted: Option<String>,
    pub next_attempt: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub rebuild_count: Option<i64>,
    pub cve_resolved_count: i64,
    pub cve_introduced_count: i64,
    pub pname: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub upstream_type: Option<String>,
    pub upstream_key: Option<String>,
    pub upstream_instance: Option<String>,
    pub semver_strategy: Option<String>,
    /// Outdated only because of a watch bootstrap baseline (left for `run`).
    pub is_backlog: bool,
}

/// Filters shared by `/packages` and `/api/packages`.
#[derive(Debug, Clone, Default)]
pub struct PackageFilter {
    /// Substring of `attr_path` or `pname`.
    pub q: Option<String>,
    /// Database `repo_name` (`""` = default repo).
    pub repo: Option<String>,
    /// An `updates.status` value, or the pseudo-status `backlog`.
    pub status: Option<String>,
    /// `updates.upstream_type`.
    pub upstream: Option<String>,
}

impl PackageFilter {
    fn status_db(&self) -> Option<&str> {
        match self.status.as_deref() {
            Some("backlog") => Some("outdated"),
            other => other,
        }
    }

    fn backlog_only(&self) -> bool {
        self.status.as_deref() == Some("backlog")
    }
}

/// Sort orders for the package list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PackageSort {
    #[default]
    Attr,
    Status,
    Checked,
    Rebuilds,
    Failures,
}

impl PackageSort {
    pub const ALL: [Self; 5] = [
        Self::Attr,
        Self::Status,
        Self::Checked,
        Self::Rebuilds,
        Self::Failures,
    ];

    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("status") => Self::Status,
            Some("checked") => Self::Checked,
            Some("rebuilds") => Self::Rebuilds,
            Some("failures") => Self::Failures,
            _ => Self::Attr,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Attr => "attr",
            Self::Status => "status",
            Self::Checked => "checked",
            Self::Rebuilds => "rebuilds",
            Self::Failures => "failures",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Attr => "Attribute",
            Self::Status => "Status",
            Self::Checked => "Last checked",
            Self::Rebuilds => "Rebuilds",
            Self::Failures => "Failures",
        }
    }

    /// `ORDER BY` clause. Only static strings: never user input.
    fn order_by(self) -> &'static str {
        match self {
            Self::Attr => "u.attr_path ASC, u.repo_name ASC",
            Self::Status => {
                "CASE u.status WHEN 'failed' THEN 0 WHEN 'upstream_error' THEN 1 \
                 WHEN 'outdated' THEN 2 WHEN 'proposed' THEN 3 WHEN 'pending' THEN 4 \
                 WHEN 'up_to_date' THEN 5 ELSE 6 END, u.attr_path ASC, u.repo_name ASC"
            },
            Self::Checked => "u.last_checked IS NULL, u.last_checked DESC, u.attr_path ASC",
            Self::Rebuilds => "u.rebuild_count IS NULL, u.rebuild_count DESC, u.attr_path ASC",
            Self::Failures => "u.failure_count DESC, u.last_attempted DESC, u.attr_path ASC",
        }
    }
}

/// `WHERE` clause shared by list/count. Parameters:
/// ?1 like-pattern, ?2 repo, ?3 status, ?4 backlog-only, ?5 upstream type.
const FILTER_SQL: &str = concat!(
    " WHERE (?1 IS NULL OR u.attr_path LIKE ?1 ESCAPE '\\' OR u.pname LIKE ?1 ESCAPE '\\')",
    " AND (?2 IS NULL OR u.repo_name = ?2)",
    " AND (?3 IS NULL OR u.status = ?3)",
    " AND (?4 = 0 OR ",
    backlog_sql!(),
    ")",
    " AND (?5 IS NULL OR u.upstream_type = ?5)"
);

/// Bind the five filter parameters in order.
macro_rules! bind_filter {
    ($query:expr, $filter:expr, $status:expr, $backlog:expr) => {
        $query
            .bind($filter.q.as_deref().map(like_pattern))
            .bind($filter.repo.as_deref())
            .bind($status)
            .bind(i64::from($backlog))
            .bind($filter.upstream.as_deref())
    };
}

/// One page of packages matching `filter`.
pub async fn list(
    pool: &SqlitePool,
    filter: &PackageFilter,
    sort: PackageSort,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<PackageRow>> {
    let sql = format!(
        "SELECT {} FROM updates u{FILTER_SQL} ORDER BY {} LIMIT ?6 OFFSET ?7",
        package_columns!(),
        sort.order_by()
    );
    bind_filter!(
        sqlx::query_as::<_, PackageRow>(&sql),
        filter,
        filter.status_db(),
        filter.backlog_only()
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

/// Number of packages matching `filter`.
pub async fn count(pool: &SqlitePool, filter: &PackageFilter) -> sqlx::Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM updates u{FILTER_SQL}");
    bind_filter!(
        sqlx::query_scalar::<_, i64>(&sql),
        filter,
        filter.status_db(),
        filter.backlog_only()
    )
    .fetch_one(pool)
    .await
}

/// Package counts per status (plus backlog and open PRs).
#[derive(Debug, Clone, Default, Serialize)]
pub struct StatusCounts {
    pub total: i64,
    pub pending: i64,
    pub up_to_date: i64,
    pub outdated: i64,
    /// Subset of `outdated`.
    pub backlog: i64,
    pub proposed: i64,
    pub failed: i64,
    pub upstream_error: i64,
    /// Subset of `proposed` with a PR URL recorded. `pr_url` is kept as
    /// history after the proposal is merged, so the status must be checked.
    pub open_prs: i64,
}

/// Status counts for packages matching `filter`, ignoring its status part
/// (so the status chips always show every bucket).
pub async fn status_counts(
    pool: &SqlitePool,
    filter: &PackageFilter,
) -> sqlx::Result<StatusCounts> {
    let sql = format!(
        "SELECT u.status, COUNT(*), COALESCE(SUM(CASE WHEN {} THEN 1 ELSE 0 END), 0), \
         COALESCE(SUM(CASE WHEN u.status = 'proposed' AND u.pr_url IS NOT NULL \
         THEN 1 ELSE 0 END), 0) \
         FROM updates u{FILTER_SQL} GROUP BY u.status",
        backlog_sql!()
    );
    let rows: Vec<(String, i64, i64, i64)> =
        bind_filter!(sqlx::query_as(&sql), filter, None::<&str>, false)
            .fetch_all(pool)
            .await?;

    let mut counts = StatusCounts::default();
    for (status, n, backlog, prs) in rows {
        counts.total += n;
        counts.backlog += backlog;
        counts.open_prs += prs;
        match status.as_str() {
            "pending" => counts.pending += n,
            "up_to_date" => counts.up_to_date += n,
            "outdated" => counts.outdated += n,
            "proposed" => counts.proposed += n,
            "failed" => counts.failed += n,
            "upstream_error" => counts.upstream_error += n,
            _ => {},
        }
    }
    Ok(counts)
}

/// Packages with a recorded PR, most recently attempted first. Includes
/// merged proposals (`pr_url` is kept as history); the status tells them apart.
pub async fn recent_prs(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<PackageRow>> {
    let sql = format!(
        "SELECT {} FROM updates u WHERE u.pr_url IS NOT NULL \
         ORDER BY u.last_attempted IS NULL, u.last_attempted DESC LIMIT ?1",
        package_columns!()
    );
    sqlx::query_as(&sql).bind(limit).fetch_all(pool).await
}

/// Repository names known to the dashboard (tracked packages or watch).
pub async fn repos(pool: &SqlitePool) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT repo_name FROM updates GROUP BY repo_name \
         UNION SELECT name FROM watched_repos ORDER BY 1",
    )
    .fetch_all(pool)
    .await
}

/// Upstream types present in `updates`.
pub async fn upstream_types(pool: &SqlitePool) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT DISTINCT upstream_type FROM updates WHERE upstream_type IS NOT NULL ORDER BY 1",
    )
    .fetch_all(pool)
    .await
}

/// All `updates` rows for an attribute (one per repo).
pub async fn by_attr(pool: &SqlitePool, attr: &str) -> sqlx::Result<Vec<PackageRow>> {
    let sql = format!(
        "SELECT {} FROM updates u WHERE u.attr_path = ?1 ORDER BY u.repo_name",
        package_columns!()
    );
    sqlx::query_as(&sql).bind(attr).fetch_all(pool).await
}

/// Repositories in which an attribute appears anywhere (updates, index,
/// phases or logs), for packages without an `updates` row.
pub async fn repos_mentioning(pool: &SqlitePool, attr: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT repo_name FROM updates WHERE attr_path = ?1 \
         UNION SELECT repo_name FROM upstream_index WHERE attr_path = ?1 \
         UNION SELECT repo_name FROM update_phases WHERE attr_path = ?1 \
         UNION SELECT repo_name FROM update_logs WHERE attr_path = ?1 \
         ORDER BY 1",
    )
    .bind(attr)
    .fetch_all(pool)
    .await
}

/// A row of `upstream_index`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct IndexEntryRow {
    pub repo_name: String,
    pub attr_path: String,
    pub upstream_type: String,
    pub upstream_key: String,
    pub instance: Option<String>,
    pub current_version: Option<String>,
    pub src_url: Option<String>,
    pub indexed_at: String,
    pub pname: Option<String>,
    pub semver_strategy: Option<String>,
    pub version_regex: Option<String>,
    pub include_prereleases: bool,
}

/// Index entries for an attribute in one repo.
pub async fn index_entries(
    pool: &SqlitePool,
    attr: &str,
    repo: &str,
) -> sqlx::Result<Vec<IndexEntryRow>> {
    sqlx::query_as(
        "SELECT repo_name, attr_path, upstream_type, upstream_key, instance, current_version, \
         src_url, indexed_at, pname, semver_strategy, version_regex, include_prereleases \
         FROM upstream_index WHERE attr_path = ?1 AND repo_name = ?2 \
         ORDER BY upstream_type, upstream_key",
    )
    .bind(attr)
    .bind(repo)
    .fetch_all(pool)
    .await
}

/// Poll health for the upstream sources of an attribute in one repo: those
/// named by its index entries and by its `updates` row.
pub async fn sources_for(
    pool: &SqlitePool,
    attr: &str,
    repo: &str,
) -> sqlx::Result<Vec<SourceRow>> {
    let sql = format!(
        "SELECT {} FROM upstream_sources s WHERE EXISTS ( \
             SELECT 1 FROM upstream_index i WHERE i.attr_path = ?1 AND i.repo_name = ?2 \
               AND i.upstream_type = s.upstream_type AND i.upstream_key = s.upstream_key \
               AND COALESCE(i.instance, '') = s.instance) \
         OR EXISTS ( \
             SELECT 1 FROM updates u WHERE u.attr_path = ?1 AND u.repo_name = ?2 \
               AND u.upstream_type = s.upstream_type AND u.upstream_key = s.upstream_key \
               AND COALESCE(u.upstream_instance, '') = s.instance) \
         ORDER BY s.upstream_type, s.upstream_key",
        source_columns!()
    );
    sqlx::query_as(&sql)
        .bind(attr)
        .bind(repo)
        .fetch_all(pool)
        .await
}

/// A phase row joined with its session.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct PackagePhaseRow {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub phase: PhaseRow,
    pub session_mode: Option<String>,
    pub session_status: Option<String>,
    pub session_started_at: Option<String>,
}

/// Most recent phases of an attribute in one repo (newest first).
pub async fn phases_for(
    pool: &SqlitePool,
    attr: &str,
    repo: &str,
    limit: i64,
) -> sqlx::Result<Vec<PackagePhaseRow>> {
    let sql = format!(
        "SELECT {}, s.mode AS session_mode, s.status AS session_status, \
         s.started_at AS session_started_at \
         FROM update_phases p LEFT JOIN update_sessions s ON s.id = p.session_id \
         WHERE p.attr_path = ?1 AND p.repo_name = ?2 ORDER BY p.id DESC LIMIT ?3",
        phase_columns!()
    );
    sqlx::query_as(&sql)
        .bind(attr)
        .bind(repo)
        .bind(limit)
        .fetch_all(pool)
        .await
}

/// Most recent failure logs of an attribute in one repo.
pub async fn logs_for(
    pool: &SqlitePool,
    attr: &str,
    repo: &str,
    limit: i64,
) -> sqlx::Result<Vec<LogRow>> {
    let sql = format!(
        "SELECT {} FROM update_logs l WHERE l.attr_path = ?1 AND l.repo_name = ?2 \
         ORDER BY l.timestamp DESC LIMIT ?3",
        log_columns!()
    );
    sqlx::query_as(&sql)
        .bind(attr)
        .bind(repo)
        .bind(limit)
        .fetch_all(pool)
        .await
}

/// A row of `autofix_queue`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AutofixQueueRow {
    pub id: i64,
    pub repo_name: String,
    pub attr_path: String,
    pub session_id: String,
    pub error_type: String,
    pub failed_phase: String,
    pub status: String,
    pub priority: i64,
    pub attempts: i64,
    pub max_attempts: i64,
    pub created_at: String,
    pub updated_at: String,
    pub fixed_at: Option<String>,
    pub artifacts_path: Option<String>,
}

/// A row of `autofix_attempts` (without the large prompt/response texts).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AutofixAttemptRow {
    pub id: i64,
    pub queue_id: i64,
    pub repo_name: String,
    pub attempt_number: i64,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub changes_applied: bool,
    pub build_success: Option<bool>,
    pub status: String,
    pub error_message: Option<String>,
}

/// Autofix queue entries for an attribute in one repository (newest first).
pub async fn autofix_queue_for(
    pool: &SqlitePool,
    attr: &str,
    repo: &str,
) -> sqlx::Result<Vec<AutofixQueueRow>> {
    sqlx::query_as(
        "SELECT id, repo_name, attr_path, session_id, error_type, failed_phase, status, \
         COALESCE(priority, 0) AS priority, COALESCE(attempts, 0) AS attempts, \
         COALESCE(max_attempts, 0) AS max_attempts, created_at, updated_at, fixed_at, \
         artifacts_path FROM autofix_queue WHERE attr_path = ?1 AND repo_name = ?2 \
         ORDER BY id DESC LIMIT 20",
    )
    .bind(attr)
    .bind(repo)
    .fetch_all(pool)
    .await
}

/// Attempts belonging to the autofix queue entries of an attribute in one
/// repository.
pub async fn autofix_attempts_for(
    pool: &SqlitePool,
    attr: &str,
    repo: &str,
) -> sqlx::Result<Vec<AutofixAttemptRow>> {
    sqlx::query_as(
        "SELECT a.id, a.queue_id, a.repo_name, a.attempt_number, a.started_at, a.completed_at, \
         a.prompt_tokens, a.completion_tokens, COALESCE(a.changes_applied, 0) AS \
         changes_applied, a.build_success, a.status, a.error_message \
         FROM autofix_attempts a JOIN autofix_queue q ON q.id = a.queue_id \
         WHERE q.attr_path = ?1 AND q.repo_name = ?2 \
         ORDER BY a.queue_id DESC, a.attempt_number ASC",
    )
    .bind(attr)
    .bind(repo)
    .fetch_all(pool)
    .await
}

/// Whether `status` is a known filter value (a status or `backlog`).
pub fn is_known_status(status: &str) -> bool {
    status == "backlog" || PACKAGE_STATUSES.contains(&status)
}
