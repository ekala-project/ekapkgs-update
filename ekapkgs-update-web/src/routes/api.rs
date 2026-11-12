//! JSON API (`/api/*`). Raw rows, no display formatting.

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{Duration, Utc};
use serde_json::{Value, json};

use super::{Params, param};
use crate::error::{ApiResult, AppError};
use crate::format::{parse_page, repo_from_param};
use crate::queries::packages as pkg_q;
use crate::queries::{sessions as sess_q, stats, watch as watch_q};
use crate::state::AppState;

/// Default and maximum page size for list endpoints.
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 500;

fn limit(params: &Params) -> i64 {
    param(params, "limit")
        .and_then(|l| l.parse::<i64>().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT)
}

/// `GET /api/stats`
pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let now = Utc::now();
    let counts = pkg_q::status_counts(pool, &pkg_q::PackageFilter::default()).await?;
    let window = stats::outcomes_between(
        pool,
        &(now - Duration::days(super::dashboard::SUCCESS_WINDOW_DAYS)).to_rfc3339(),
        "",
    )
    .await?;
    let running = sess_q::running(pool).await?;
    let sessions_total = stats::session_total(pool).await?;
    let sources = watch_q::source_type_counts(pool).await?;
    let active_updates: i64 = running.iter().map(sess_q::SessionRow::in_flight).sum();
    Ok(Json(json!({
        "generated_at": now.to_rfc3339(),
        // Kept from the original API:
        "total_packages": counts.total,
        "success_rate": window.success_rate(),
        "active_updates": active_updates,
        "total_sessions": sessions_total,
        // Added:
        "packages": counts,
        "running_sessions": running.len(),
        "outcomes_30d": window,
        "upstream_sources": {
            "total": sources.iter().map(|s| s.total).sum::<i64>(),
            "failing": sources.iter().map(|s| s.failing).sum::<i64>(),
        },
    })))
}

/// `GET /api/sessions?status=&mode=&show_empty=&page=&limit=`
pub async fn sessions(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let lp = super::sessions::ListParams::parse(&params);
    let limit = limit(&params);
    let total = sess_q::count(pool, &lp.filter).await?;
    let rows = sess_q::list(pool, &lp.filter, limit, (lp.page - 1) * limit).await?;
    Ok(Json(json!({
        "sessions": rows,
        "total": total,
        "page": lp.page,
        "per_page": limit,
    })))
}

/// `GET /api/sessions/:id/phases` — phases in execution order.
pub async fn session_phases(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    if sess_q::get(pool, &id).await?.is_none() {
        return Err(AppError::NotFound(format!("There is no session with id “{id}”.")).into());
    }
    let phases = sess_q::phases(pool, &id).await?;
    Ok(Json(json!(phases)))
}

/// `GET /api/packages?q=&repo=&status=&upstream=&sort=&page=&limit=`
pub async fn packages(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let lp = super::packages::ListParams::parse(&params);
    let limit = limit(&params);
    let total = pkg_q::count(pool, &lp.filter).await?;
    let rows = pkg_q::list(pool, &lp.filter, lp.sort, limit, (lp.page - 1) * limit).await?;
    let counts = pkg_q::status_counts(pool, &lp.filter).await?;
    Ok(Json(json!({
        "packages": rows,
        "total": total,
        "page": lp.page,
        "per_page": limit,
        "sort": lp.sort.as_str(),
        "status_counts": counts,
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
        return Err(AppError::NotFound(format!("Unknown attribute path “{attr}”.")).into());
    }
    let repo = match param(&params, "repo") {
        Some(label) => {
            let name = repo_from_param(&label);
            if !repos.contains(&name) {
                return Err(AppError::NotFound(format!(
                    "“{attr}” has not been seen in repository “{label}”."
                ))
                .into());
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
        "phases": pkg_q::phases_for(pool, &attr, &repo, 200).await?,
        "logs": pkg_q::logs_for(pool, &attr, &repo, 20).await?,
        "autofix": {
            "queue": pkg_q::autofix_queue_for(pool, &attr, &repo).await?,
            "attempts": pkg_q::autofix_attempts_for(pool, &attr, &repo).await?,
        },
    })))
}

/// `GET /api/watch` — repositories, recent cycles and source health.
pub async fn watch(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> ApiResult<Json<Value>> {
    let pool = state.db.pool();
    let limit = limit(&params);
    let page = parse_page(params.get("page").map(String::as_str));
    Ok(Json(json!({
        "repos": watch_q::repos(pool).await?,
        "cycles": watch_q::cycles(pool, None, limit, (page - 1) * limit).await?,
        "cycle_total": watch_q::cycle_count(pool, None).await?,
        "source_types": watch_q::source_type_counts(pool).await?,
        "failing_sources": watch_q::failing_sources(pool, limit).await?,
        "recent_releases": watch_q::recent_releases(pool, limit).await?,
    })))
}
