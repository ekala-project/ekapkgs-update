//! Read-only SQL against the shared ekapkgs-update database.
//!
//! Every query here is a `SELECT`; the web portal never writes. Row structs
//! are defined locally (not imported from the CLI crate) so the dashboard
//! only depends on the table schema, not on the CLI's Rust types.

/// SQL predicate selecting backlog packages on `updates` aliased as `u`:
/// outdated, but only because of a release watch saw during its
/// observe-only bootstrap (left for `run`).
macro_rules! backlog_sql {
    () => {
        "(u.status = 'outdated' AND u.baseline_version IS NOT NULL \
         AND u.latest_upstream_version = u.baseline_version)"
    };
}

/// Columns of `updates u` decoded by [`packages::PackageRow`].
macro_rules! package_columns {
    () => {
        concat!(
            "u.repo_name, u.attr_path, u.status, u.current_version, \
             u.latest_upstream_version, u.proposed_version, u.baseline_version, \
             u.failed_version, COALESCE(u.failure_count, 0) AS failure_count, u.last_error, \
             u.last_session_id, u.last_checked, u.last_attempted, u.next_attempt, u.pr_url, \
             u.pr_number, u.rebuild_count, COALESCE(u.cve_resolved_count, 0) AS \
             cve_resolved_count, COALESCE(u.cve_introduced_count, 0) AS cve_introduced_count, \
             u.pname, u.description, u.homepage, u.upstream_type, u.upstream_key, \
             u.upstream_instance, u.semver_strategy, CASE WHEN ",
            backlog_sql!(),
            " THEN 1 ELSE 0 END AS is_backlog"
        )
    };
}

/// Columns of `update_phases p` decoded by [`sessions::PhaseRow`].
macro_rules! phase_columns {
    () => {
        "p.id, p.session_id, p.repo_name, p.attr_path, p.phase, p.started_at, p.completed_at, \
         p.duration_ms, p.status, p.error_type, p.error_details, p.artifacts_path"
    };
}

/// Columns of `update_sessions s` decoded by [`sessions::SessionRow`].
macro_rules! session_columns {
    () => {
        "s.id, s.mode, s.status, s.started_at, s.completed_at, s.updated_at, \
         COALESCE(s.packages_attempted, 0) AS packages_attempted, \
         COALESCE(s.packages_succeeded, 0) AS packages_succeeded, \
         COALESCE(s.packages_failed, 0) AS packages_failed, \
         COALESCE(s.packages_skipped, 0) AS packages_skipped, \
         COALESCE(s.packages_evaluated, 0) AS packages_evaluated, \
         COALESCE(s.packages_backoff, 0) AS packages_backoff"
    };
}

/// Columns of `update_logs l` decoded by [`failures::LogRow`]. Only the tail
/// of `error_log` is fetched (see [`LOG_TAIL_CHARS`]).
macro_rules! log_columns {
    () => {
        "l.drv_path, l.repo_name, l.attr_path, l.timestamp, l.status, l.old_version, \
         l.new_version, l.session_id, l.phase, l.error_type, \
         CASE WHEN length(l.error_log) > 6000 THEN substr(l.error_log, -6000) \
         ELSE l.error_log END AS log_tail, length(l.error_log) AS log_len"
    };
}

/// Columns of `upstream_sources s` decoded by [`watch::SourceRow`].
macro_rules! source_columns {
    () => {
        "s.upstream_type, s.upstream_key, s.instance, s.latest_version, s.latest_tag, \
         s.release_count, s.latest_changed_at, s.first_seen_at, s.last_polled_at, \
         s.last_success_at, s.last_error, s.consecutive_failures, \
         (SELECT COUNT(DISTINCT i.attr_path) FROM upstream_index i \
          WHERE i.upstream_type = s.upstream_type AND i.upstream_key = s.upstream_key \
            AND COALESCE(i.instance, '') = s.instance) AS package_count"
    };
}

/// Columns of `watch_cycles c` decoded by [`watch::CycleRow`].
macro_rules! cycle_columns {
    () => {
        "c.id, c.kind, c.repo_name, c.started_at, c.completed_at, c.duration_ms, c.status, \
         c.bootstrap, c.sources_polled, c.sources_failed, c.packages_observed, c.new_releases, \
         c.dispatched, c.updated, c.failed, c.session_id, c.error"
    };
}

pub mod analytics;
pub mod failures;
pub mod live;
pub mod packages;
pub mod sessions;
pub mod stats;
pub mod watch;

/// Characters of `update_logs.error_log` fetched for display (the tail).
pub const LOG_TAIL_CHARS: i64 = 6000;

/// Every `updates.status` value, in display order.
pub const PACKAGE_STATUSES: [&str; 6] = [
    "pending",
    "up_to_date",
    "outdated",
    "proposed",
    "failed",
    "upstream_error",
];

/// Every `UpdatePhase`, in workflow order.
pub const PHASES: [&str; 10] = [
    "metadata_extraction",
    "source_discovery",
    "version_selection",
    "source_hash_update",
    "dependency_hash_update",
    "build",
    "patch_recovery",
    "testing",
    "agent_fix",
    "pr_creation",
];

/// Workflow position of a phase (unknown phases sort last).
pub fn phase_rank(phase: &str) -> usize {
    PHASES
        .iter()
        .position(|p| *p == phase)
        .unwrap_or(PHASES.len())
}

/// Short label for a phase chip: `dependency_hash_update` → `dep hash`.
pub fn phase_short(phase: &str) -> String {
    match phase {
        "metadata_extraction" => "metadata".to_owned(),
        "source_discovery" => "discovery".to_owned(),
        "version_selection" => "version".to_owned(),
        "source_hash_update" => "src hash".to_owned(),
        "dependency_hash_update" => "dep hash".to_owned(),
        "patch_recovery" => "patches".to_owned(),
        "testing" => "tests".to_owned(),
        "agent_fix" => "agent fix".to_owned(),
        "pr_creation" => "PR".to_owned(),
        other => crate::format::humanize(other),
    }
}

/// Build a `LIKE` pattern matching `q` anywhere, escaping wildcards.
/// Use with `ESCAPE '\'`.
pub fn like_pattern(q: &str) -> String {
    let mut escaped = String::with_capacity(q.len() + 2);
    escaped.push('%');
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped.push('%');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(like_pattern("foo"), "%foo%");
        assert_eq!(like_pattern("50%_off\\"), "%50\\%\\_off\\\\%");
    }

    #[test]
    fn phases_have_ranks_and_labels() {
        assert_eq!(phase_rank("metadata_extraction"), 0);
        assert_eq!(phase_rank("pr_creation"), 9);
        assert_eq!(phase_rank("unknown"), PHASES.len());
        assert_eq!(phase_short("dependency_hash_update"), "dep hash");
        assert_eq!(phase_short("build"), "build");
    }
}
