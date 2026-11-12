//! View models: database rows turned into ready-to-render values.
//!
//! Templates only print fields and loop; every bit of formatting happens
//! here so it is testable and askama templates stay simple.

pub mod charts;

use chrono::{DateTime, Duration, Utc};

use crate::format::{
    self, Badge, DASH, Ts, Version, duration_ms, duration_opt, package_href, repo_label, safe_url,
    session_href, short_id, thousands, truncate,
};
use crate::queries::failures::LogRow;
use crate::queries::packages::{AutofixAttemptRow, AutofixQueueRow, IndexEntryRow, PackageRow};
use crate::queries::sessions::{PhaseRow, SessionRow};
use crate::queries::watch::{CycleRow, RepoRow, SourceRow};
use crate::queries::{phase_rank, phase_short};

/// A running session whose heartbeat is older than this is flagged stale.
pub const STALE_AFTER_HOURS: i64 = 6;

/// One segment of a thin segmented bar.
#[derive(Debug, Clone)]
pub struct Segment {
    /// `seg-good`, `seg-critical`, …
    pub class: &'static str,
    /// Relative size (`flex-grow`).
    pub value: i64,
    pub title: String,
}

/// A thin segmented bar with its accessible label.
#[derive(Debug, Clone, Default)]
pub struct SegBar {
    pub segments: Vec<Segment>,
    pub label: String,
}

impl SegBar {
    pub fn push(&mut self, class: &'static str, value: i64, name: &str) {
        if value > 0 {
            self.segments.push(Segment {
                class,
                value,
                title: format!("{} {name}", thousands(value)),
            });
        }
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

/// A package row for tables.
#[derive(Debug, Clone)]
pub struct PackageItem {
    pub repo: String,
    pub attr_path: String,
    pub href: String,
    pub pname: String,
    pub badge: Badge,
    pub current: Version,
    pub latest: Version,
    pub proposed: Version,
    /// The latest upstream version differs from the current one.
    pub has_newer: bool,
    pub pr_url: Option<String>,
    pub pr_label: String,
    pub checked: Ts,
    pub failures: i64,
    pub failures_text: String,
    pub rebuilds: String,
    pub last_error: String,
    pub upstream: String,
}

impl PackageItem {
    pub fn new(row: &PackageRow, now: DateTime<Utc>) -> Self {
        let current = Version::new(row.current_version.as_deref());
        let latest = Version::new(row.latest_upstream_version.as_deref());
        let has_newer = latest.is_set() && latest.full != current.full;
        Self {
            repo: repo_label(&row.repo_name).to_owned(),
            attr_path: row.attr_path.clone(),
            href: package_href(&row.repo_name, &row.attr_path),
            pname: row.pname.clone().unwrap_or_default(),
            badge: Badge::package(&row.status, row.is_backlog),
            current,
            latest,
            proposed: Version::new(row.proposed_version.as_deref()),
            has_newer,
            pr_url: safe_url(row.pr_url.as_deref()),
            pr_label: row
                .pr_number
                .map_or_else(|| "PR".to_owned(), |n| format!("#{n}")),
            checked: Ts::new(
                row.last_checked
                    .as_deref()
                    .or(row.last_attempted.as_deref()),
                now,
            ),
            failures: row.failure_count,
            failures_text: thousands(row.failure_count),
            rebuilds: row.rebuild_count.map_or_else(|| DASH.to_owned(), thousands),
            last_error: row
                .last_error
                .as_deref()
                .map(|e| truncate(e, 160))
                .unwrap_or_default(),
            upstream: row.upstream_type.clone().unwrap_or_default(),
        }
    }
}

/// Everything about one `updates` row, for the package detail page.
#[derive(Debug, Clone)]
pub struct PackageDetail {
    pub item: PackageItem,
    pub description: String,
    pub homepage: Option<String>,
    pub baseline: Version,
    pub failed_version: Version,
    pub last_error: String,
    pub next_attempt: Ts,
    pub in_backoff: bool,
    pub last_attempted: Ts,
    pub cve_resolved: i64,
    pub cve_introduced: i64,
    pub semver: String,
    pub upstream_label: String,
    pub upstream_url: Option<String>,
    pub last_session_href: Option<String>,
    pub last_session_short: String,
    pub is_backlog: bool,
}

impl PackageDetail {
    pub fn new(row: &PackageRow, now: DateTime<Utc>) -> Self {
        let next = row.next_attempt.as_deref().and_then(format::parse_ts);
        let upstream_label = match (&row.upstream_type, &row.upstream_key) {
            (Some(t), Some(k)) => match row.upstream_instance.as_deref() {
                Some(i) if !i.is_empty() => format!("{t} · {k} @ {i}"),
                _ => format!("{t} · {k}"),
            },
            (Some(t), None) => t.clone(),
            _ => String::new(),
        };
        let upstream_url = match (&row.upstream_type, &row.upstream_key) {
            (Some(t), Some(k)) => format::upstream_url(t, k, row.upstream_instance.as_deref()),
            _ => None,
        };
        Self {
            item: PackageItem::new(row, now),
            description: row.description.clone().unwrap_or_default(),
            homepage: safe_url(row.homepage.as_deref()),
            baseline: Version::new(row.baseline_version.as_deref()),
            failed_version: Version::new(row.failed_version.as_deref()),
            last_error: row.last_error.clone().unwrap_or_default(),
            next_attempt: Ts::new(row.next_attempt.as_deref(), now),
            in_backoff: next.is_some_and(|n| n > now),
            last_attempted: Ts::new(row.last_attempted.as_deref(), now),
            cve_resolved: row.cve_resolved_count,
            cve_introduced: row.cve_introduced_count,
            semver: row.semver_strategy.clone().unwrap_or_default(),
            upstream_label,
            upstream_url,
            last_session_href: row.last_session_id.as_deref().map(session_href),
            last_session_short: row
                .last_session_id
                .as_deref()
                .map(short_id)
                .unwrap_or_default(),
            is_backlog: row.is_backlog,
        }
    }
}

/// A session row for tables and cards.
#[derive(Debug, Clone)]
pub struct SessionItem {
    pub id: String,
    pub short_id: String,
    pub href: String,
    pub mode: String,
    pub badge: Badge,
    pub is_running: bool,
    pub stale: bool,
    pub started: Ts,
    pub completed: Ts,
    pub heartbeat: Ts,
    pub duration: String,
    pub attempted: String,
    pub succeeded: String,
    pub failed: String,
    pub skipped: String,
    pub evaluated: String,
    pub backoff: String,
    pub in_flight: String,
    pub has_attempts: bool,
    pub progress: SegBar,
    pub progress_text: String,
    pub success_rate: String,
}

impl SessionItem {
    pub fn new(row: &SessionRow, now: DateTime<Utc>) -> Self {
        let is_running = row.status == "running";
        let heartbeat_raw = row.updated_at.as_deref().unwrap_or(&row.started_at);
        let stale = is_running
            && format::parse_ts(heartbeat_raw)
                .is_some_and(|hb| now - hb > Duration::hours(STALE_AFTER_HOURS));
        let end = if is_running {
            None
        } else {
            row.completed_at
                .as_deref()
                .or(row.updated_at.as_deref())
                .or(Some(row.started_at.as_str()))
        };
        let duration = format::elapsed_ms(&row.started_at, end, now)
            .map_or_else(|| DASH.to_owned(), duration_ms);

        let mut progress = SegBar::default();
        progress.push("seg-good", row.packages_succeeded, "succeeded");
        progress.push("seg-neutral", row.packages_skipped, "skipped");
        progress.push("seg-critical", row.packages_failed, "failed");
        progress.push("seg-info", row.in_flight(), "in flight");
        progress.label = format!(
            "{} succeeded, {} skipped, {} failed, {} in flight",
            row.packages_succeeded,
            row.packages_skipped,
            row.packages_failed,
            row.in_flight()
        );
        let progress_text = format!(
            "{} of {} finished",
            thousands(row.finished()),
            thousands(row.packages_attempted)
        );

        let badge = if stale {
            Badge::new("running · stale?", "serious")
        } else {
            Badge::status(&row.status)
        };

        Self {
            id: row.id.clone(),
            short_id: short_id(&row.id),
            href: session_href(&row.id),
            mode: row.mode.clone(),
            badge,
            is_running,
            stale,
            started: Ts::new(Some(&row.started_at), now),
            completed: Ts::new(row.completed_at.as_deref(), now),
            heartbeat: Ts::new(Some(heartbeat_raw), now),
            duration,
            attempted: thousands(row.packages_attempted),
            succeeded: thousands(row.packages_succeeded),
            failed: thousands(row.packages_failed),
            skipped: thousands(row.packages_skipped),
            evaluated: thousands(row.packages_evaluated),
            backoff: thousands(row.packages_backoff),
            in_flight: thousands(row.in_flight()),
            has_attempts: row.packages_attempted > 0,
            progress,
            progress_text,
            success_rate: format::pct_text(format::ratio_pct(
                row.packages_succeeded,
                row.packages_succeeded + row.packages_failed,
            )),
        }
    }
}

/// One phase as a small chip.
#[derive(Debug, Clone)]
pub struct PhaseChip {
    pub label: String,
    pub tone: &'static str,
    pub duration: String,
    pub title: String,
}

impl PhaseChip {
    pub fn new(row: &PhaseRow, now: DateTime<Utc>) -> Self {
        let duration = match row.duration_ms {
            Some(ms) => duration_ms(ms),
            None if row.status == "running" => {
                format::elapsed_ms(&row.started_at, None, now).map_or_else(String::new, duration_ms)
            },
            None => String::new(),
        };
        let mut title = format!("{} · {}", row.phase, format::humanize(&row.status));
        if !duration.is_empty() {
            title.push_str(" · ");
            title.push_str(&duration);
        }
        if let Some(e) = &row.error_type {
            title.push_str(" · ");
            title.push_str(e);
        }
        Self {
            label: phase_short(&row.phase),
            tone: format::tone(&row.status),
            duration,
            title,
        }
    }
}

/// A failed phase's error, for collapsible details.
#[derive(Debug, Clone)]
pub struct PhaseError {
    pub repo: String,
    pub attr_path: String,
    pub href: String,
    pub phase: String,
    pub error_type: String,
    pub details: String,
    pub has_details: bool,
    pub artifacts_path: String,
    pub when: Ts,
}

impl PhaseError {
    pub fn new(row: &PhaseRow, now: DateTime<Utc>) -> Self {
        let details = row
            .error_details
            .as_deref()
            .map(format::pretty_json)
            .unwrap_or_default();
        Self {
            repo: repo_label(&row.repo_name).to_owned(),
            attr_path: row.attr_path.clone(),
            href: package_href(&row.repo_name, &row.attr_path),
            phase: row.phase.clone(),
            error_type: row.error_type.clone().unwrap_or_else(|| "error".to_owned()),
            has_details: !details.is_empty(),
            details,
            artifacts_path: row.artifacts_path.clone().unwrap_or_default(),
            when: Ts::new(row.completed_at.as_deref().or(Some(&row.started_at)), now),
        }
    }
}

/// The overall outcome of one package's phases within a session.
pub fn outcome(phases: &[&PhaseRow], session_running: bool) -> (&'static str, Badge) {
    if let Some(failed) = phases.iter().find(|p| p.status == "failed") {
        return (
            "failed",
            Badge::new(
                format!("failed · {}", phase_short(&failed.phase)),
                "critical",
            ),
        );
    }
    if phases.iter().any(|p| p.status == "running") {
        return if session_running {
            ("running", Badge::new("running", "info"))
        } else {
            ("interrupted", Badge::new("interrupted", "serious"))
        };
    }
    if phases
        .iter()
        .any(|p| p.phase == "pr_creation" && p.status == "success")
    {
        return ("proposed", Badge::new("proposed", "info"));
    }
    if !phases.is_empty() && phases.iter().all(|p| p.status == "skipped") {
        return ("skipped", Badge::new("skipped", "neutral"));
    }
    if phases.iter().any(|p| p.status == "skipped")
        && !phases
            .iter()
            .any(|p| phase_rank(&p.phase) >= phase_rank("build"))
    {
        // Stopped early (e.g. no compatible release) without building.
        return ("skipped", Badge::new("skipped", "neutral"));
    }
    ("succeeded", Badge::new("succeeded", "good"))
}

/// One package's phases within a session.
#[derive(Debug, Clone)]
pub struct RollupItem {
    pub repo: String,
    pub attr_path: String,
    pub href: String,
    pub chips: Vec<PhaseChip>,
    pub outcome_key: &'static str,
    pub outcome: Badge,
    pub duration: String,
}

/// Group a session's phases by package, in first-seen order.
pub fn rollup(phases: &[PhaseRow], session_running: bool, now: DateTime<Utc>) -> Vec<RollupItem> {
    let mut order: Vec<(&str, &str)> = Vec::new();
    let mut groups: std::collections::HashMap<(&str, &str), Vec<&PhaseRow>> =
        std::collections::HashMap::new();
    for p in phases {
        let key = (p.repo_name.as_str(), p.attr_path.as_str());
        groups
            .entry(key)
            .or_insert_with(|| {
                order.push(key);
                Vec::new()
            })
            .push(p);
    }
    order
        .into_iter()
        .map(|key| {
            let rows = &groups[&key];
            let (outcome_key, badge) = outcome(rows, session_running);
            let total: i64 = rows.iter().filter_map(|p| p.duration_ms).sum();
            RollupItem {
                repo: repo_label(key.0).to_owned(),
                attr_path: key.1.to_owned(),
                href: package_href(key.0, key.1),
                chips: rows.iter().map(|p| PhaseChip::new(p, now)).collect(),
                outcome_key,
                outcome: badge,
                duration: if total > 0 {
                    duration_ms(total)
                } else {
                    String::new()
                },
            }
        })
        .collect()
}

/// A failure log entry.
#[derive(Debug, Clone)]
pub struct LogItem {
    pub drv_path: String,
    pub repo: String,
    pub attr_path: String,
    pub href: String,
    pub when: Ts,
    pub old: Version,
    pub new: Version,
    pub phase: String,
    pub error_type: String,
    pub session_href: Option<String>,
    pub session_short: String,
    pub summary: String,
    pub log: String,
    pub truncated: bool,
    pub size: String,
}

impl LogItem {
    pub fn new(row: &LogRow, now: DateTime<Utc>) -> Self {
        let summary = row
            .log_tail
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| truncate(l, 180))
            .unwrap_or_default();
        Self {
            drv_path: row.drv_path.clone(),
            repo: repo_label(&row.repo_name).to_owned(),
            attr_path: row.attr_path.clone(),
            href: package_href(&row.repo_name, &row.attr_path),
            when: Ts::new(Some(&row.timestamp), now),
            old: Version::new(row.old_version.as_deref()),
            new: Version::new(row.new_version.as_deref()),
            phase: row.phase.clone().unwrap_or_default(),
            error_type: row.error_type.clone().unwrap_or_default(),
            session_href: row.session_id.as_deref().map(session_href),
            session_short: row.session_id.as_deref().map(short_id).unwrap_or_default(),
            summary,
            log: row.log_tail.clone(),
            truncated: row.log_len > crate::queries::LOG_TAIL_CHARS,
            size: format::bytes(row.log_len),
        }
    }
}

/// A watch cycle row.
#[derive(Debug, Clone)]
pub struct CycleItem {
    pub id: i64,
    pub kind: String,
    pub repo: String,
    pub badge: Badge,
    pub started: Ts,
    pub duration: String,
    pub bootstrap: bool,
    pub sources_polled: String,
    pub sources_failed: i64,
    pub observed: String,
    pub new_releases: String,
    pub dispatched: String,
    pub updated: String,
    pub failed: i64,
    pub session_href: Option<String>,
    pub session_short: String,
    pub error: String,
}

impl CycleItem {
    pub fn new(row: &CycleRow, now: DateTime<Utc>) -> Self {
        let duration = match row.duration_ms {
            Some(ms) => duration_ms(ms),
            None if row.status == "running" => format::elapsed_ms(&row.started_at, None, now)
                .map_or_else(
                    || DASH.to_owned(),
                    |ms| format!("{} so far", duration_ms(ms)),
                ),
            None => DASH.to_owned(),
        };
        Self {
            id: row.id,
            kind: row.kind.clone(),
            repo: match row.repo_name.as_deref() {
                Some(r) => repo_label(r).to_owned(),
                None => "all repos".to_owned(),
            },
            badge: Badge::status(&row.status),
            started: Ts::new(Some(&row.started_at), now),
            duration,
            bootstrap: row.bootstrap,
            sources_polled: thousands(row.sources_polled),
            sources_failed: row.sources_failed,
            observed: thousands(row.packages_observed),
            new_releases: thousands(row.new_releases),
            dispatched: thousands(row.dispatched),
            updated: thousands(row.updated),
            failed: row.failed,
            session_href: row.session_id.as_deref().map(session_href),
            session_short: row.session_id.as_deref().map(short_id).unwrap_or_default(),
            error: row.error.clone().unwrap_or_default(),
        }
    }
}

/// A watched repository.
#[derive(Debug, Clone)]
pub struct RepoItem {
    pub name: String,
    pub upstream: String,
    pub upstream_url: Option<String>,
    pub path: String,
    pub entrypoint: String,
    pub branch: String,
    pub fork_remote: String,
    pub head_sha: String,
    pub head_full: String,
    pub head_subject: String,
    pub sync_badge: Badge,
    pub sync_error: String,
    pub synced: Ts,
    pub index_badge: Badge,
    pub index_error: String,
    pub indexed: Ts,
    pub index_duration: String,
    pub evaluated: String,
    pub indexed_count: String,
    pub index_skipped: String,
    pub index_errors: i64,
    pub index_rows: String,
    pub bootstrapped: Ts,
    pub is_bootstrapped: bool,
    pub backlog: i64,
    pub backlog_text: String,
    pub packages_href: String,
    pub backlog_href: String,
}

impl RepoItem {
    pub fn new(row: &RepoRow, now: DateTime<Utc>) -> Self {
        let label = repo_label(&row.name).to_owned();
        let opt_count = |v: Option<i64>| v.map_or_else(|| DASH.to_owned(), thousands);
        Self {
            upstream: row.upstream.clone().unwrap_or_default(),
            upstream_url: safe_url(row.upstream.as_deref()),
            path: row.path.clone(),
            entrypoint: row.entrypoint.clone(),
            branch: row.base_branch.clone().unwrap_or_default(),
            fork_remote: row.fork_remote.clone().unwrap_or_default(),
            head_sha: row
                .head_commit
                .as_deref()
                .map(format::short_sha)
                .unwrap_or_default(),
            head_full: row.head_commit.clone().unwrap_or_default(),
            head_subject: row
                .head_subject
                .as_deref()
                .map(|s| truncate(s, 80))
                .unwrap_or_default(),
            sync_badge: Badge::status(row.sync_status.as_deref().unwrap_or("unknown")),
            sync_error: row.sync_error.clone().unwrap_or_default(),
            synced: Ts::new(row.last_synced_at.as_deref(), now),
            index_badge: Badge::status(row.index_status.as_deref().unwrap_or("not indexed")),
            index_error: row.index_error.clone().unwrap_or_default(),
            indexed: Ts::new(row.last_indexed_at.as_deref(), now),
            index_duration: duration_opt(row.index_duration_ms),
            evaluated: opt_count(row.evaluated_count),
            indexed_count: opt_count(row.indexed_count),
            index_skipped: opt_count(row.index_skipped),
            index_errors: row.index_errors.unwrap_or(0),
            index_rows: thousands(row.index_rows),
            bootstrapped: Ts::new(row.bootstrapped_at.as_deref(), now),
            is_bootstrapped: row.bootstrapped_at.is_some(),
            backlog: row.backlog_count,
            backlog_text: thousands(row.backlog_count),
            packages_href: format::href("/packages", &format::query_string(&[("repo", &label)])),
            backlog_href: format::href(
                "/packages",
                &format::query_string(&[("repo", &label), ("status", "backlog")]),
            ),
            name: label,
        }
    }
}

/// An upstream source with poll health.
#[derive(Debug, Clone)]
pub struct SourceItem {
    pub kind: String,
    pub key: String,
    pub instance: String,
    pub url: Option<String>,
    pub latest: Version,
    pub tag: String,
    pub releases: String,
    pub changed: Ts,
    pub polled: Ts,
    pub success: Ts,
    pub error: String,
    pub badge: Badge,
    pub packages: String,
}

impl SourceItem {
    pub fn new(row: &SourceRow, now: DateTime<Utc>) -> Self {
        let badge = if row.consecutive_failures > 0 {
            Badge::new(
                format!("failing ×{}", row.consecutive_failures),
                if row.consecutive_failures >= 3 {
                    "critical"
                } else {
                    "serious"
                },
            )
        } else if row.last_polled_at.is_none() {
            Badge::new("not polled", "neutral")
        } else {
            Badge::new("ok", "good")
        };
        Self {
            kind: row.upstream_type.clone(),
            key: row.upstream_key.clone(),
            instance: row.instance.clone(),
            url: format::upstream_url(&row.upstream_type, &row.upstream_key, Some(&row.instance)),
            latest: Version::new(row.latest_version.as_deref()),
            tag: row.latest_tag.clone().unwrap_or_default(),
            releases: row.release_count.map_or_else(|| DASH.to_owned(), thousands),
            changed: Ts::new(row.latest_changed_at.as_deref(), now),
            polled: Ts::new(row.last_polled_at.as_deref(), now),
            success: Ts::new(row.last_success_at.as_deref(), now),
            error: row.last_error.clone().unwrap_or_default(),
            badge,
            packages: thousands(row.package_count),
        }
    }
}

/// An `upstream_index` entry.
#[derive(Debug, Clone)]
pub struct IndexItem {
    pub kind: String,
    pub key: String,
    pub instance: String,
    pub url: Option<String>,
    pub current: Version,
    pub src_url: String,
    pub indexed: Ts,
    pub semver: String,
    pub version_regex: String,
    pub prereleases: bool,
}

impl IndexItem {
    pub fn new(row: &IndexEntryRow, now: DateTime<Utc>) -> Self {
        Self {
            kind: row.upstream_type.clone(),
            key: row.upstream_key.clone(),
            instance: row.instance.clone().unwrap_or_default(),
            url: format::upstream_url(
                &row.upstream_type,
                &row.upstream_key,
                row.instance.as_deref(),
            ),
            current: Version::new(row.current_version.as_deref()),
            src_url: row.src_url.clone().unwrap_or_default(),
            indexed: Ts::new(Some(&row.indexed_at), now),
            semver: row.semver_strategy.clone().unwrap_or_default(),
            version_regex: row.version_regex.clone().unwrap_or_default(),
            prereleases: row.include_prereleases,
        }
    }
}

/// An autofix attempt.
#[derive(Debug, Clone)]
pub struct AttemptItem {
    pub number: i64,
    pub badge: Badge,
    pub started: Ts,
    pub duration: String,
    pub tokens: String,
    pub changes_applied: bool,
    pub build: String,
    pub error: String,
}

/// An autofix queue entry with its attempts.
#[derive(Debug, Clone)]
pub struct AutofixItem {
    pub badge: Badge,
    pub error_type: String,
    pub failed_phase: String,
    pub attempts_text: String,
    pub created: Ts,
    pub updated: Ts,
    pub fixed: Ts,
    pub session_href: String,
    pub session_short: String,
    pub artifacts_path: String,
    pub attempts: Vec<AttemptItem>,
}

/// Combine queue entries with their attempts.
pub fn autofix_items(
    queue: &[AutofixQueueRow],
    attempts: &[AutofixAttemptRow],
    now: DateTime<Utc>,
) -> Vec<AutofixItem> {
    queue
        .iter()
        .map(|q| AutofixItem {
            badge: Badge::status(&q.status),
            error_type: q.error_type.clone(),
            failed_phase: q.failed_phase.clone(),
            attempts_text: format!("{} / {}", q.attempts, q.max_attempts),
            created: Ts::new(Some(&q.created_at), now),
            updated: Ts::new(Some(&q.updated_at), now),
            fixed: Ts::new(q.fixed_at.as_deref(), now),
            session_href: session_href(&q.session_id),
            session_short: short_id(&q.session_id),
            artifacts_path: q.artifacts_path.clone().unwrap_or_default(),
            attempts: attempts
                .iter()
                .filter(|a| a.queue_id == q.id)
                .map(|a| AttemptItem {
                    number: a.attempt_number,
                    badge: Badge::status(&a.status),
                    started: Ts::new(Some(&a.started_at), now),
                    duration: format::elapsed_ms(&a.started_at, a.completed_at.as_deref(), now)
                        .filter(|_| a.completed_at.is_some())
                        .map_or_else(|| DASH.to_owned(), duration_ms),
                    tokens: match (a.prompt_tokens, a.completion_tokens) {
                        (None, None) => DASH.to_owned(),
                        (p, c) => format!(
                            "{} in · {} out",
                            thousands(p.unwrap_or(0)),
                            thousands(c.unwrap_or(0))
                        ),
                    },
                    changes_applied: a.changes_applied,
                    build: match a.build_success {
                        Some(true) => "passed".to_owned(),
                        Some(false) => "failed".to_owned(),
                        None => DASH.to_owned(),
                    },
                    error: a.error_message.clone().unwrap_or_default(),
                })
                .collect(),
        })
        .collect()
}
