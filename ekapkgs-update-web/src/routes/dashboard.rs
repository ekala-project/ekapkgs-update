//! `/` and its live panels (`/fragments/{stats,active,watch,activity}`).

use std::collections::HashMap;

use axum::extract::State;
use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;

use crate::error::AppResult;
use crate::format::{
    self, Badge, DASH, Ts, Version, href, package_href, pct_text, query_string, repo_label,
    session_href, short_id, thousands,
};
use crate::queries::packages::{self as pkg_q, PackageFilter, StatusCounts};
use crate::queries::sessions::{self as sess_q, ActivityRow, PhaseRow};
use crate::queries::{phase_short, stats, watch as watch_q};
use crate::state::AppState;
use crate::templates::{
    ActiveFragment, ActivityFragment, DashboardTemplate, StatsFragment, WatchPanelFragment,
};
use crate::views::{CycleItem, PackageItem, RepoItem, SegBar, SessionItem, outcome};

/// Window for the headline success rate.
pub const SUCCESS_WINDOW_DAYS: i64 = 30;
/// Phase rows sampled for the activity feed.
const ACTIVITY_SAMPLE: i64 = 300;
/// Outcomes shown in the activity feed.
const ACTIVITY_ITEMS: usize = 20;
/// PRs shown next to the activity feed.
const RECENT_PRS: i64 = 8;

// ── Stats tiles ──────────────────────────────────────────────────────────

/// One headline number.
#[derive(Debug, Clone)]
pub struct Tile {
    pub label: &'static str,
    pub value: String,
    pub href: String,
    /// Status tone for the leading dot (`neutral` for the total).
    pub tone: &'static str,
    /// Secondary line ("12 backlog", "3 open PRs").
    pub note: String,
    pub note_href: Option<String>,
}

/// Headline numbers panel.
#[derive(Debug, Clone)]
pub struct StatsView {
    pub tiles: Vec<Tile>,
    /// Status composition across all tracked packages.
    pub composition: SegBar,
    pub is_empty: bool,
    pub success_rate: String,
    pub success_detail: String,
    /// "+2.1 pts vs previous 30 days"; empty without a comparison window.
    pub success_delta: String,
    pub success_delta_tone: &'static str,
    pub window_days: i64,
    /// Link to the backlog when there is one.
    pub backlog_href: Option<String>,
}

/// Status composition bar for a set of counts (fixed status order).
pub fn composition_bar(c: &StatusCounts) -> SegBar {
    let parts: [(&'static str, i64, &str); 7] = [
        ("seg-good", c.up_to_date, "up to date"),
        ("seg-info", c.proposed, "proposed"),
        ("seg-warn", c.outdated - c.backlog, "outdated"),
        ("seg-warn-light", c.backlog, "backlog"),
        ("seg-critical", c.failed, "failed"),
        ("seg-serious", c.upstream_error, "upstream error"),
        ("seg-neutral", c.pending, "pending"),
    ];
    let mut bar = SegBar::default();
    let mut label = Vec::new();
    for (class, value, name) in parts {
        bar.push(class, value, name);
        if value > 0 {
            label.push(format!("{} {name}", thousands(value)));
        }
    }
    bar.label = label.join(", ");
    bar
}

fn packages_link(status: &str) -> String {
    href("/packages", &query_string(&[("status", status)]))
}

pub async fn build_stats(pool: &SqlitePool, now: DateTime<Utc>) -> sqlx::Result<StatsView> {
    let counts = pkg_q::status_counts(pool, &PackageFilter::default()).await?;
    let window = Duration::days(SUCCESS_WINDOW_DAYS);
    let cutoff = (now - window).to_rfc3339();
    let prev_cutoff = (now - window - window).to_rfc3339();
    let current = stats::outcomes_between(pool, &cutoff, "").await?;
    let previous = stats::outcomes_between(pool, &prev_cutoff, &cutoff).await?;

    let rate = current.success_rate();
    let (success_delta, success_delta_tone) = match (rate, previous.success_rate()) {
        (Some(now_rate), Some(prev_rate)) => {
            let delta = now_rate - prev_rate;
            let tone = if delta >= 0.05 {
                "good"
            } else if delta <= -0.05 {
                "critical"
            } else {
                "neutral"
            };
            (
                format!(
                    "{}{delta:.1} pts vs previous {SUCCESS_WINDOW_DAYS} days",
                    if delta >= 0.0 { "+" } else { "" }
                ),
                tone,
            )
        },
        _ => (String::new(), "neutral"),
    };

    let outdated_note = if counts.backlog > 0 {
        format!("{} backlog", thousands(counts.backlog))
    } else {
        String::new()
    };
    let tiles = vec![
        Tile {
            label: "Tracked",
            value: thousands(counts.total),
            href: "/packages".to_owned(),
            tone: "neutral",
            note: if counts.pending > 0 {
                format!("{} pending", thousands(counts.pending))
            } else {
                String::new()
            },
            note_href: (counts.pending > 0).then(|| packages_link("pending")),
        },
        Tile {
            label: "Up to date",
            value: thousands(counts.up_to_date),
            href: packages_link("up_to_date"),
            tone: "good",
            note: pct_text(format::ratio_pct(counts.up_to_date, counts.total)),
            note_href: None,
        },
        Tile {
            label: "Outdated",
            value: thousands(counts.outdated),
            href: packages_link("outdated"),
            tone: "warn",
            note_href: (counts.backlog > 0).then(|| packages_link("backlog")),
            note: outdated_note,
        },
        Tile {
            label: "Proposed",
            value: thousands(counts.proposed),
            href: packages_link("proposed"),
            tone: "info",
            note: format!("{} with PR", thousands(counts.open_prs)),
            note_href: None,
        },
        Tile {
            label: "Failed",
            value: thousands(counts.failed),
            href: packages_link("failed"),
            tone: "critical",
            note: String::new(),
            note_href: None,
        },
        Tile {
            label: "Upstream errors",
            value: thousands(counts.upstream_error),
            href: packages_link("upstream_error"),
            tone: "serious",
            note: String::new(),
            note_href: None,
        },
    ];

    Ok(StatsView {
        composition: composition_bar(&counts),
        is_empty: counts.total == 0,
        success_rate: pct_text(rate),
        success_detail: if current.sessions == 0 {
            format!("No finished updates in the last {SUCCESS_WINDOW_DAYS} days")
        } else {
            format!(
                "{} succeeded · {} failed · {} skipped across {} sessions",
                thousands(current.succeeded),
                thousands(current.failed),
                thousands(current.skipped),
                thousands(current.sessions)
            )
        },
        success_delta,
        success_delta_tone,
        window_days: SUCCESS_WINDOW_DAYS,
        backlog_href: (counts.backlog > 0).then(|| packages_link("backlog")),
        tiles,
    })
}

// ── Active sessions ──────────────────────────────────────────────────────

/// A package currently inside a phase.
#[derive(Debug, Clone)]
pub struct CurrentWork {
    pub repo: String,
    pub attr_path: String,
    pub href: String,
    pub phase: String,
    pub elapsed: String,
}

impl CurrentWork {
    pub fn new(row: &PhaseRow, now: DateTime<Utc>) -> Self {
        Self {
            repo: repo_label(&row.repo_name).to_owned(),
            attr_path: row.attr_path.clone(),
            href: package_href(&row.repo_name, &row.attr_path),
            phase: phase_short(&row.phase),
            elapsed: format::elapsed_ms(&row.started_at, None, now)
                .map_or_else(String::new, format::duration_ms),
        }
    }
}

/// A running session card.
#[derive(Debug, Clone)]
pub struct ActiveSession {
    pub s: SessionItem,
    pub work: Vec<CurrentWork>,
    pub more_work: usize,
}

/// Running sessions panel.
#[derive(Debug, Clone)]
pub struct ActiveView {
    pub sessions: Vec<ActiveSession>,
    pub any: bool,
}

/// Running phases listed per session card.
const WORK_PER_SESSION: usize = 6;

pub async fn build_active(pool: &SqlitePool, now: DateTime<Utc>) -> sqlx::Result<ActiveView> {
    let running = sess_q::running(pool).await?;
    let phases = sess_q::running_phases(pool, 200).await?;
    let sessions: Vec<ActiveSession> = running
        .iter()
        .map(|row| {
            let mine: Vec<&PhaseRow> = phases.iter().filter(|p| p.session_id == row.id).collect();
            ActiveSession {
                s: SessionItem::new(row, now),
                work: mine
                    .iter()
                    .take(WORK_PER_SESSION)
                    .map(|p| CurrentWork::new(p, now))
                    .collect(),
                more_work: mine.len().saturating_sub(WORK_PER_SESSION),
            }
        })
        .collect();
    Ok(ActiveView {
        any: !sessions.is_empty(),
        sessions,
    })
}

// ── Watch panel ──────────────────────────────────────────────────────────

/// Compact watch status for the dashboard.
#[derive(Debug, Clone)]
pub struct WatchPanelView {
    pub configured: bool,
    pub repos: Vec<RepoItem>,
    pub last_poll: Option<CycleItem>,
    pub sources_total: String,
    pub sources_failing: i64,
    pub sources_failing_text: String,
    pub backlog: i64,
    pub backlog_text: String,
}

pub async fn build_watch_panel(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> sqlx::Result<WatchPanelView> {
    let repos = watch_q::repos(pool).await?;
    let last_poll = watch_q::last_poll(pool).await?;
    let types = watch_q::source_type_counts(pool).await?;
    let sources_total: i64 = types.iter().map(|t| t.total).sum();
    let sources_failing: i64 = types.iter().map(|t| t.failing).sum();
    let backlog: i64 = repos.iter().map(|r| r.backlog_count).sum();
    Ok(WatchPanelView {
        configured: !repos.is_empty() || last_poll.is_some() || sources_total > 0,
        repos: repos.iter().map(|r| RepoItem::new(r, now)).collect(),
        last_poll: last_poll.as_ref().map(|c| CycleItem::new(c, now)),
        sources_total: thousands(sources_total),
        sources_failing,
        sources_failing_text: thousands(sources_failing),
        backlog,
        backlog_text: thousands(backlog),
    })
}

// ── Activity feed ────────────────────────────────────────────────────────

/// The latest outcome of one package within one session.
#[derive(Debug, Clone)]
pub struct ActivityItem {
    pub repo: String,
    pub attr_path: String,
    pub href: String,
    pub badge: Badge,
    pub when: Ts,
    /// Last phase reached ("build", "PR").
    pub phase: String,
    /// Versions or error type.
    pub detail: String,
    pub pr_url: Option<String>,
    pub pr_label: String,
    pub session_href: String,
    pub session_short: String,
    pub mode: String,
}

/// Recent activity panel.
#[derive(Debug, Clone)]
pub struct ActivityView {
    pub items: Vec<ActivityItem>,
    pub prs: Vec<PackageItem>,
}

/// Group recent phase rows into per-(session, package) outcomes, newest first.
pub fn activity_items(rows: &[ActivityRow], now: DateTime<Utc>, limit: usize) -> Vec<ActivityItem> {
    let mut order: Vec<(&str, &str, &str)> = Vec::new();
    let mut groups: HashMap<(&str, &str, &str), Vec<&ActivityRow>> = HashMap::new();
    for row in rows {
        let key = (
            row.phase.session_id.as_str(),
            row.phase.repo_name.as_str(),
            row.phase.attr_path.as_str(),
        );
        groups
            .entry(key)
            .or_insert_with(|| {
                order.push(key);
                Vec::new()
            })
            .push(row);
    }
    order
        .into_iter()
        .take(limit)
        .map(|key| {
            // Rows are newest first; `outcome` wants execution order.
            let group = &groups[&key];
            let latest = group[0];
            let phases: Vec<&PhaseRow> = group.iter().rev().map(|r| &r.phase).collect();
            let running = latest.session_status.as_deref() == Some("running");
            let (kind, badge) = outcome(&phases, running);
            let current = Version::new(latest.current_version.as_deref());
            let detail = match kind {
                "failed" => {
                    let failed = phases.iter().find(|p| p.status == "failed");
                    let error = failed
                        .and_then(|p| p.error_type.clone())
                        .unwrap_or_else(|| "error".to_owned());
                    match latest.failed_version.as_deref() {
                        Some(v) => format!("{error} · target {}", format::short_version(v)),
                        None => error,
                    }
                },
                "proposed" => match latest.proposed_version.as_deref() {
                    Some(v) if current.is_set() => {
                        format!("{} → {}", current.short, format::short_version(v))
                    },
                    Some(v) => format!("→ {}", format::short_version(v)),
                    None => String::new(),
                },
                _ => String::new(),
            };
            ActivityItem {
                repo: repo_label(key.1).to_owned(),
                attr_path: key.2.to_owned(),
                href: package_href(key.1, key.2),
                badge,
                when: Ts::new(
                    latest
                        .phase
                        .completed_at
                        .as_deref()
                        .or(Some(&latest.phase.started_at)),
                    now,
                ),
                phase: phase_short(&latest.phase.phase),
                detail,
                pr_url: if kind == "proposed" {
                    format::safe_url(latest.pr_url.as_deref())
                } else {
                    None
                },
                pr_label: latest
                    .pr_number
                    .map_or_else(|| "PR".to_owned(), |n| format!("#{n}")),
                session_href: session_href(key.0),
                session_short: short_id(key.0),
                mode: latest
                    .session_mode
                    .clone()
                    .unwrap_or_else(|| DASH.to_owned()),
            }
        })
        .collect()
}

pub async fn build_activity(pool: &SqlitePool, now: DateTime<Utc>) -> sqlx::Result<ActivityView> {
    let rows = sess_q::recent_phases(pool, ACTIVITY_SAMPLE).await?;
    let prs = pkg_q::recent_prs(pool, RECENT_PRS).await?;
    Ok(ActivityView {
        items: activity_items(&rows, now, ACTIVITY_ITEMS),
        prs: prs.iter().map(|p| PackageItem::new(p, now)).collect(),
    })
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /`
pub async fn index(State(state): State<AppState>) -> AppResult<DashboardTemplate> {
    let pool = state.db.pool();
    let now = Utc::now();
    Ok(DashboardTemplate {
        stats: build_stats(pool, now).await?,
        active: build_active(pool, now).await?,
        watch: build_watch_panel(pool, now).await?,
        activity: build_activity(pool, now).await?,
    })
}

/// `GET /fragments/stats`
pub async fn stats_fragment(State(state): State<AppState>) -> AppResult<StatsFragment> {
    Ok(StatsFragment {
        stats: build_stats(state.db.pool(), Utc::now()).await?,
    })
}

/// `GET /fragments/active`
pub async fn active_fragment(State(state): State<AppState>) -> AppResult<ActiveFragment> {
    Ok(ActiveFragment {
        active: build_active(state.db.pool(), Utc::now()).await?,
    })
}

/// `GET /fragments/watch`
pub async fn watch_fragment(State(state): State<AppState>) -> AppResult<WatchPanelFragment> {
    Ok(WatchPanelFragment {
        watch: build_watch_panel(state.db.pool(), Utc::now()).await?,
    })
}

/// `GET /fragments/activity`
pub async fn activity_fragment(State(state): State<AppState>) -> AppResult<ActivityFragment> {
    Ok(ActivityFragment {
        activity: build_activity(state.db.pool(), Utc::now()).await?,
    })
}
