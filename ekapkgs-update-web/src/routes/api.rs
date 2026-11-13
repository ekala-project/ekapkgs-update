//! JSON API (`/api/*`). Raw rows plus the counts and facets that need SQL;
//! all display formatting, grouping and chart geometry happens in the Elm
//! frontend.
//!
//! Every page of the single-page app is backed by one endpoint here
//! (`/api/dashboard`, `/api/packages`, …), so a page renders from a single
//! request and refreshes by repeating it.

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use super::{Params, flag, param};
use crate::error::{ApiResult, AppError};
use crate::params::{clamp_page, parse_page, repo_from_param};
use crate::queries::failures::{self as fail_q, FailureFilter};
use crate::queries::packages::{self as pkg_q, PackageFilter, PackageSort};
use crate::queries::sessions::{self as sess_q, SessionFilter};
use crate::queries::{analytics as an_q, stats, watch as watch_q};
use crate::state::AppState;

/// Maximum page size for list endpoints (`limit=`).
const MAX_LIMIT: i64 = 500;
/// Default page sizes.
pub const PACKAGES_PER_PAGE: i64 = 50;
pub const SESSIONS_PER_PAGE: i64 = 50;
pub const FAILURES_PER_PAGE: i64 = 25;
pub const CYCLES_PER_PAGE: i64 = 25;

/// Window for the dashboard success rate (and its comparison window).
pub const SUCCESS_WINDOW_DAYS: i64 = 30;
/// Recent phases the dashboard groups into its activity feed.
const ACTIVITY_SAMPLE: i64 = 300;
/// Open PRs listed on the dashboard.
const RECENT_PRS: i64 = 8;
/// Running phases fetched for the "in progress" panel.
const RUNNING_PHASES: i64 = 200;
/// Phase rows fetched for a package's history.
const HISTORY_PHASES: i64 = 400;
/// Failure logs on a package page.
const PACKAGE_LOGS: i64 = 15;
/// Failure logs on a session page.
const SESSION_LOGS: i64 = 20;
/// Sources listed in each watch-page table.
const WATCH_SOURCES: i64 = 25;
/// Rows in the analytics "top" tables.
const TOP_ROWS: i64 = 10;
/// Session statuses and modes the session filters accept.
const SESSION_MODES: [&str; 2] = ["run", "watch"];

/// Selectable analytics ranges: (param, days; 0 = all time).
const RANGES: [(&str, i64); 4] = [("7d", 7), ("30d", 30), ("90d", 90), ("all", 0)];
/// The daily trend never spans more than this many days.
const MAX_TREND_DAYS: i64 = 90;

fn limit(params: &Params, default: i64) -> i64 {
    param(params, "limit")
        .and_then(|l| l.parse::<i64>().ok())
        .unwrap_or(default)
        .clamp(1, MAX_LIMIT)
}

fn page(params: &Params) -> i64 {
    parse_page(params.get("page").map(String::as_str))
}

fn days_ago(now: DateTime<Utc>, days: i64) -> String {
    (now - Duration::days(days)).to_rfc3339()
}

/// Parsed `/api/packages` filters.
pub fn package_filter(params: &Params) -> PackageFilter {
    PackageFilter {
        q: param(params, "q"),
        repo: param(params, "repo").map(|r| repo_from_param(&r)),
        status: param(params, "status").filter(|s| pkg_q::is_known_status(s)),
        upstream: param(params, "upstream"),
    }
}

/// Parsed `/api/sessions` filters.
pub fn session_filter(params: &Params) -> SessionFilter {
    SessionFilter {
        status: param(params, "status"),
        mode: param(params, "mode").filter(|m| SESSION_MODES.contains(&m.as_str())),
        show_empty: flag(params, "show_empty"),
    }
}

/// `GET /api/stats` — headline numbers (kept for external consumers).
pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let now = Utc::now();
    let counts = pkg_q::status_counts(pool, &PackageFilter::default()).await?;
    let window = stats::outcomes_between(pool, &days_ago(now, SUCCESS_WINDOW_DAYS), "").await?;
    let running = sess_q::running(pool).await?;
    let sessions_total = stats::session_total(pool).await?;
    let sources = watch_q::source_type_counts(pool).await?;
    let active_updates: i64 = running.iter().map(sess_q::SessionRow::in_flight).sum();
    Ok(Json(json!({
        "generated_at": now.to_rfc3339(),
        "total_packages": counts.total,
        "success_rate": window.success_rate(),
        "active_updates": active_updates,
        "total_sessions": sessions_total,
        "packages": counts,
        "running_sessions": running.len(),
        "outcomes_30d": window,
        "upstream_sources": {
            "total": sources.iter().map(|s| s.total).sum::<i64>(),
            "failing": sources.iter().map(|s| s.failing).sum::<i64>(),
        },
    })))
}

/// `GET /api/dashboard` — everything the overview page shows.
pub async fn dashboard(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let now = Utc::now();
    let cutoff = days_ago(now, SUCCESS_WINDOW_DAYS);
    let prev_cutoff = days_ago(now, 2 * SUCCESS_WINDOW_DAYS);
    Ok(Json(json!({
        "generated_at": now.to_rfc3339(),
        "window_days": SUCCESS_WINDOW_DAYS,
        "counts": pkg_q::status_counts(pool, &PackageFilter::default()).await?,
        "current": stats::outcomes_between(pool, &cutoff, "").await?,
        "previous": stats::outcomes_between(pool, &prev_cutoff, &cutoff).await?,
        "running": sess_q::running(pool).await?,
        "running_phases": sess_q::running_phases(pool, RUNNING_PHASES).await?,
        "watch": {
            "repos": watch_q::repos(pool).await?,
            "last_poll": watch_q::last_poll(pool).await?,
            "source_types": watch_q::source_type_counts(pool).await?,
        },
        "activity": sess_q::recent_phases(pool, ACTIVITY_SAMPLE).await?,
        "recent_prs": pkg_q::recent_prs(pool, RECENT_PRS).await?,
    })))
}

/// `GET /api/packages?q=&repo=&status=&upstream=&sort=&page=&limit=`
pub async fn packages(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let filter = package_filter(&params);
    let sort = PackageSort::parse(param(&params, "sort").as_deref());
    let limit = limit(&params, PACKAGES_PER_PAGE);
    let total = pkg_q::count(pool, &filter).await?;
    let page = clamp_page(page(&params), total, limit);
    let rows = pkg_q::list(pool, &filter, sort, limit, (page - 1) * limit).await?;
    let counts = pkg_q::status_counts(pool, &filter).await?;
    let all_total = pkg_q::status_counts(pool, &PackageFilter::default())
        .await?
        .total;
    Ok(Json(json!({
        "packages": rows,
        "total": total,
        "all_total": all_total,
        "page": page,
        "per_page": limit,
        "sort": sort.as_str(),
        "sorts": PackageSort::ALL
            .iter()
            .map(|s| json!({ "key": s.as_str(), "label": s.label() }))
            .collect::<Vec<_>>(),
        "status_counts": counts,
        "repos": pkg_q::repos(pool).await?,
        "upstream_types": pkg_q::upstream_types(pool).await?,
    })))
}

/// `GET /api/packages/:attr?repo=` — everything known about one attribute.
pub async fn package(
    State(state): State<AppState>,
    Path(attr): Path<String>,
    Query(params): Query<Params>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let repos = pkg_q::repos_mentioning(pool, &attr).await?;
    if repos.is_empty() {
        return Err(AppError::NotFound(format!(
            "No package with attribute path “{attr}” has been seen by ekapkgs-update."
        )));
    }
    let repo = match param(&params, "repo") {
        Some(label) => {
            let name = repo_from_param(&label);
            if !repos.contains(&name) {
                return Err(AppError::NotFound(format!(
                    "“{attr}” has not been seen in repository “{label}”."
                )));
            }
            name
        },
        None => repos[0].clone(),
    };
    let updates = pkg_q::by_attr(pool, &attr).await?;
    let update = updates.iter().find(|u| u.repo_name == repo);
    Ok(Json(json!({
        "attr_path": attr,
        "repo_name": repo,
        "repos": repos,
        "update": update,
        "index": pkg_q::index_entries(pool, &attr, &repo).await?,
        "sources": pkg_q::sources_for(pool, &attr, &repo).await?,
        "phases": pkg_q::phases_for(pool, &attr, &repo, HISTORY_PHASES).await?,
        "logs": pkg_q::logs_for(pool, &attr, &repo, PACKAGE_LOGS).await?,
        "autofix": {
            "queue": pkg_q::autofix_queue_for(pool, &attr, &repo).await?,
            "attempts": pkg_q::autofix_attempts_for(pool, &attr, &repo).await?,
        },
    })))
}

/// `GET /api/sessions?status=&mode=&show_empty=&page=&limit=`
pub async fn sessions(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let filter = session_filter(&params);
    let limit = limit(&params, SESSIONS_PER_PAGE);
    let total = sess_q::count(pool, &filter).await?;
    let page = clamp_page(page(&params), total, limit);
    let rows = sess_q::list(pool, &filter, limit, (page - 1) * limit).await?;
    let hidden_empty = if filter.show_empty {
        0
    } else {
        sess_q::hidden_empty_count(pool, &filter).await?
    };
    let facets: Vec<Value> = sess_q::facet_counts(pool)
        .await?
        .into_iter()
        .map(|(status, mode, count)| json!({ "status": status, "mode": mode, "count": count }))
        .collect();
    Ok(Json(json!({
        "sessions": rows,
        "total": total,
        "page": page,
        "per_page": limit,
        "hidden_empty": hidden_empty,
        "facets": facets,
    })))
}

/// `GET /api/sessions/:id` — a session, its phases, failure logs and the
/// watch cycle that started it (if any).
pub async fn session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let Some(session) = sess_q::get(pool, &id).await? else {
        return Err(AppError::NotFound(format!(
            "There is no session with id “{id}”."
        )));
    };
    Ok(Json(json!({
        "session": session,
        "phases": sess_q::phases(pool, &id).await?,
        "logs": fail_q::for_session(pool, &id, SESSION_LOGS).await?,
        "cycle": watch_q::cycle_for_session(pool, &id).await?,
    })))
}

/// `GET /api/sessions/:id/phases` — phases in execution order.
pub async fn session_phases(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    if sess_q::get(pool, &id).await?.is_none() {
        return Err(AppError::NotFound(format!(
            "There is no session with id “{id}”."
        )));
    }
    Ok(Json(json!(sess_q::phases(pool, &id).await?)))
}

/// `GET /api/failures?q=&repo=&phase=&error_type=&page=&limit=`
pub async fn failures(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let filter = FailureFilter {
        q: param(&params, "q"),
        repo: param(&params, "repo").map(|r| repo_from_param(&r)),
        phase: param(&params, "phase"),
        error_type: param(&params, "error_type"),
    };
    let limit = limit(&params, FAILURES_PER_PAGE);
    let total = fail_q::count(pool, &filter).await?;
    let page = clamp_page(page(&params), total, limit);
    let rows = fail_q::list(pool, &filter, limit, (page - 1) * limit).await?;
    let (repos, phases, error_types) = fail_q::facets(pool).await?;
    Ok(Json(json!({
        "logs": rows,
        "total": total,
        "page": page,
        "per_page": limit,
        "facets": { "repos": repos, "phases": phases, "error_types": error_types },
    })))
}

/// `GET /api/watch?kind=&page=&limit=` — repositories, cycles and source
/// health.
pub async fn watch(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let kind = param(&params, "kind").filter(|k| k == "poll" || k == "index");
    let limit = limit(&params, CYCLES_PER_PAGE);
    let cycle_total = watch_q::cycle_count(pool, kind.as_deref()).await?;
    let page = clamp_page(page(&params), cycle_total, limit);
    Ok(Json(json!({
        "repos": watch_q::repos(pool).await?,
        "kind": kind,
        "cycles": watch_q::cycles(pool, kind.as_deref(), limit, (page - 1) * limit).await?,
        "cycle_total": cycle_total,
        "cycle_counts": {
            "all": watch_q::cycle_count(pool, None).await?,
            "poll": watch_q::cycle_count(pool, Some("poll")).await?,
            "index": watch_q::cycle_count(pool, Some("index")).await?,
        },
        "page": page,
        "per_page": limit,
        "last_poll": watch_q::last_poll(pool).await?,
        "source_types": watch_q::source_type_counts(pool).await?,
        "failing_sources": watch_q::failing_sources(pool, WATCH_SOURCES).await?,
        "recent_releases": watch_q::recent_releases(pool, WATCH_SOURCES).await?,
    })))
}

/// `GET /api/analytics?range=7d|30d|90d|all` — aggregates for the
/// analytics page. `range` defaults to `30d`; the daily trend covers the
/// range (at most [`MAX_TREND_DAYS`]).
pub async fn analytics(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let now = Utc::now();
    let wanted = param(&params, "range").unwrap_or_default();
    let (range, days) = RANGES
        .iter()
        .find(|(k, _)| *k == wanted)
        .copied()
        .unwrap_or(RANGES[1]);
    // An empty cutoff means all time.
    let cutoff = if days > 0 {
        days_ago(now, days)
    } else {
        String::new()
    };
    let trend_days = if days > 0 { days } else { MAX_TREND_DAYS };

    let percentiles: Vec<Value> = an_q::phase_percentiles(pool, &cutoff)
        .await?
        .into_iter()
        .map(|(phase, p50, p90)| json!({ "phase": phase, "p50_ms": p50, "p90_ms": p90 }))
        .collect();
    let by_repo: Vec<Value> = an_q::status_by_repo(pool)
        .await?
        .into_iter()
        .map(|(repo, status, count, backlog)| {
            json!({ "repo_name": repo, "status": status, "count": count, "backlog": backlog })
        })
        .collect();

    Ok(Json(json!({
        "generated_at": now.to_rfc3339(),
        "range": range,
        "days": days,
        "trend_days": trend_days,
        "outcomes": stats::outcomes_between(pool, &cutoff, "").await?,
        "daily": an_q::daily_outcomes(pool, &days_ago(now, trend_days)).await?,
        "error_types": an_q::error_types(pool, &cutoff).await?,
        "phase_stats": an_q::phase_stats(pool, &cutoff).await?,
        "phase_percentiles": percentiles,
        "status_by_repo": by_repo,
        "upstream_types": an_q::upstream_types(pool).await?,
        // Histogram buckets in axis order (the counts come in SQL order).
        "rebuild_bucket_order": an_q::REBUILD_BUCKETS
            .iter()
            .chain(std::iter::once(&"unknown"))
            .collect::<Vec<_>>(),
        "rebuild_buckets": an_q::rebuild_buckets(pool).await?,
        "top_rebuilds": an_q::top_rebuilds(pool, TOP_ROWS).await?,
        "autofix_queue": an_q::autofix_queue(pool, &cutoff).await?,
        "autofix_attempts": an_q::autofix_attempts(pool, &cutoff).await?,
        "top_failing": an_q::top_failing(pool, &cutoff, TOP_ROWS).await?,
    })))
}

/// `/api/*` paths that match no endpoint.
pub async fn not_found() -> AppError {
    AppError::NotFound("There is no API endpoint at this address.".to_owned())
}
