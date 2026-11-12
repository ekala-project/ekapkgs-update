//! `/failures`: recorded update failures (`update_logs`).

use axum::extract::{Query, State};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use super::{FAILURES_PER_PAGE, Params, param};
use crate::error::AppResult;
use crate::format::{
    Pager, SelectOption, humanize, parse_page, query_string, repo_from_param, repo_label,
};
use crate::queries::failures::{self as q, FailureFilter};
use crate::queries::phase_short;
use crate::state::AppState;
use crate::templates::FailuresTemplate;
use crate::views::LogItem;

/// `/failures` view.
#[derive(Debug, Clone)]
pub struct FailureListView {
    pub items: Vec<LogItem>,
    pub pager: Pager,
    pub q: String,
    pub repo_options: Vec<SelectOption>,
    pub phase_options: Vec<SelectOption>,
    pub error_type_options: Vec<SelectOption>,
    pub has_filters: bool,
    pub db_empty: bool,
}

async fn build_list(
    pool: &SqlitePool,
    params: &Params,
    now: DateTime<Utc>,
) -> sqlx::Result<FailureListView> {
    let repo_label_param = param(params, "repo").unwrap_or_default();
    let filter = FailureFilter {
        q: param(params, "q"),
        repo: (!repo_label_param.is_empty()).then(|| repo_from_param(&repo_label_param)),
        phase: param(params, "phase"),
        error_type: param(params, "error_type"),
    };
    let phase = filter.phase.clone().unwrap_or_default();
    let error_type = filter.error_type.clone().unwrap_or_default();
    let base = query_string(&[
        ("q", filter.q.as_deref().unwrap_or("")),
        ("repo", &repo_label_param),
        ("phase", &phase),
        ("error_type", &error_type),
    ]);
    let total = q::count(pool, &filter).await?;
    let pager = Pager::new(
        "/failures",
        &base,
        parse_page(params.get("page").map(String::as_str)),
        FAILURES_PER_PAGE,
        total,
    );
    let rows = q::list(
        pool,
        &filter,
        FAILURES_PER_PAGE,
        pager.offset(FAILURES_PER_PAGE),
    )
    .await?;
    let (repos, phases, error_types) = q::facets(pool).await?;
    let has_filters = filter.q.is_some()
        || filter.repo.is_some()
        || filter.phase.is_some()
        || filter.error_type.is_some();
    let db_empty = !has_filters && total == 0;

    let mut repo_options = vec![SelectOption::new("", "All repos", &repo_label_param)];
    repo_options.extend(repos.iter().map(|r| {
        let label = repo_label(r);
        SelectOption::new(label, label, &repo_label_param)
    }));
    let mut phase_options = vec![SelectOption::new("", "Any phase", &phase)];
    phase_options.extend(phases.iter().map(|p| {
        let label = format!("{} ({})", phase_short(p), humanize(p));
        SelectOption::new(p, &label, &phase)
    }));
    let mut error_type_options = vec![SelectOption::new("", "Any error type", &error_type)];
    error_type_options.extend(
        error_types
            .iter()
            .map(|e| SelectOption::new(e, e, &error_type)),
    );

    Ok(FailureListView {
        items: rows.iter().map(|r| LogItem::new(r, now)).collect(),
        pager,
        q: filter.q.clone().unwrap_or_default(),
        repo_options,
        phase_options,
        error_type_options,
        has_filters,
        db_empty,
    })
}

/// `GET /failures`
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> AppResult<FailuresTemplate> {
    Ok(FailuresTemplate {
        list: build_list(state.db.pool(), &params, Utc::now()).await?,
    })
}
