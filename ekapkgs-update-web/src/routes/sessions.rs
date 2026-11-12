//! `/sessions`, `/sessions/:id` and their live fragments.

use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use super::{Params, ROLLUP_PER_PAGE, SESSIONS_PER_PAGE, chip, flag, param};
use crate::error::{AppError, AppResult};
use crate::format::{self, Chip, Pager, href, parse_page, pretty_json, query_string, thousands};
use crate::queries::sessions::{self as q, SessionFilter};
use crate::queries::{failures, watch};
use crate::state::AppState;
use crate::templates::{
    SessionDetailTemplate, SessionLiveFragment, SessionsFragment, SessionsTemplate,
};
use crate::views::{CycleItem, LogItem, PhaseError, RollupItem, SessionItem, rollup};

/// Session statuses written by the CLI, in chip order.
const SESSION_STATUSES: [&str; 4] = ["running", "completed", "failed", "cancelled"];
/// Session modes.
const SESSION_MODES: [&str; 2] = ["run", "watch"];

// ── List ─────────────────────────────────────────────────────────────────

/// `/sessions` view (also rendered by `/fragments/sessions`).
#[derive(Debug, Clone)]
pub struct SessionListView {
    pub items: Vec<SessionItem>,
    pub pager: Pager,
    pub status_chips: Vec<Chip>,
    pub mode_chips: Vec<Chip>,
    pub show_empty: bool,
    pub hidden_empty: i64,
    pub hidden_empty_text: String,
    pub toggle_empty_href: String,
    /// `/fragments/sessions?…` with the same filters and page.
    pub fragment_href: String,
    pub has_filters: bool,
    pub db_empty: bool,
}

/// Parsed `/sessions` (and `/api/sessions`) parameters.
pub struct ListParams {
    pub filter: SessionFilter,
    pub page: i64,
}

impl ListParams {
    pub fn parse(params: &Params) -> Self {
        Self {
            filter: SessionFilter {
                status: param(params, "status"),
                mode: param(params, "mode").filter(|m| SESSION_MODES.contains(&m.as_str())),
                show_empty: flag(params, "show_empty"),
            },
            page: parse_page(params.get("page").map(String::as_str)),
        }
    }

    fn query(&self, status: Option<&str>, mode: Option<&str>, show_empty: Option<bool>) -> String {
        let status = status.unwrap_or(self.filter.status.as_deref().unwrap_or(""));
        let mode = mode.unwrap_or(self.filter.mode.as_deref().unwrap_or(""));
        let show_empty = show_empty.unwrap_or(self.filter.show_empty);
        query_string(&[
            ("status", status),
            ("mode", mode),
            ("show_empty", if show_empty { "1" } else { "" }),
        ])
    }
}

async fn build_list(
    pool: &SqlitePool,
    params: &Params,
    now: DateTime<Utc>,
) -> sqlx::Result<SessionListView> {
    let lp = ListParams::parse(params);
    let total = q::count(pool, &lp.filter).await?;
    let base = lp.query(None, None, None);
    let pager = Pager::new("/sessions", &base, lp.page, SESSIONS_PER_PAGE, total);
    let rows = q::list(
        pool,
        &lp.filter,
        SESSIONS_PER_PAGE,
        pager.offset(SESSIONS_PER_PAGE),
    )
    .await?;
    let hidden_empty = if lp.filter.show_empty {
        0
    } else {
        q::hidden_empty_count(pool, &lp.filter).await?
    };
    let facets = q::facet_counts(pool).await?;

    let status = lp.filter.status.clone().unwrap_or_default();
    let mode = lp.filter.mode.clone().unwrap_or_default();
    let count_where = |f: &dyn Fn(&str, &str) -> bool| -> i64 {
        facets
            .iter()
            .filter(|(s, m, _)| f(s, m))
            .map(|(_, _, n)| n)
            .sum()
    };

    let mut statuses: Vec<String> = SESSION_STATUSES.iter().map(|s| (*s).to_owned()).collect();
    for (s, _, _) in &facets {
        if !statuses.contains(s) {
            statuses.push(s.clone());
        }
    }
    let mut status_chips = vec![chip(
        "All",
        Some(count_where(&|_, m| mode.is_empty() || m == mode)),
        href("/sessions", &lp.query(Some(""), None, None)),
        status.is_empty(),
        "neutral",
    )];
    for s in &statuses {
        status_chips.push(chip(
            &format::humanize(s),
            Some(count_where(&|fs, m| {
                fs == s && (mode.is_empty() || m == mode)
            })),
            href("/sessions", &lp.query(Some(s), None, None)),
            status == *s,
            format::tone(s),
        ));
    }
    let mut mode_chips = vec![chip(
        "All modes",
        None,
        href("/sessions", &lp.query(None, Some(""), None)),
        mode.is_empty(),
        "neutral",
    )];
    for m in SESSION_MODES {
        mode_chips.push(chip(
            m,
            Some(count_where(&|s, fm| {
                fm == m && (status.is_empty() || s == status)
            })),
            href("/sessions", &lp.query(None, Some(m), None)),
            mode == m,
            "neutral",
        ));
    }

    let page_query = if pager.page > 1 {
        let page = format!("page={}", pager.page);
        if base.is_empty() {
            page
        } else {
            format!("{base}&{page}")
        }
    } else {
        base.clone()
    };

    Ok(SessionListView {
        items: rows.iter().map(|r| SessionItem::new(r, now)).collect(),
        pager,
        status_chips,
        mode_chips,
        show_empty: lp.filter.show_empty,
        hidden_empty,
        hidden_empty_text: thousands(hidden_empty),
        toggle_empty_href: href(
            "/sessions",
            &lp.query(None, None, Some(!lp.filter.show_empty)),
        ),
        fragment_href: href("/fragments/sessions", &page_query),
        has_filters: lp.filter.status.is_some() || lp.filter.mode.is_some(),
        db_empty: facets.is_empty() && hidden_empty == 0 && total == 0,
    })
}

/// `GET /sessions`
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> AppResult<SessionsTemplate> {
    Ok(SessionsTemplate {
        list: build_list(state.db.pool(), &params, Utc::now()).await?,
    })
}

/// `GET /fragments/sessions`
pub async fn list_fragment(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> AppResult<SessionsFragment> {
    Ok(SessionsFragment {
        list: build_list(state.db.pool(), &params, Utc::now()).await?,
    })
}

// ── Detail ───────────────────────────────────────────────────────────────

/// Package outcomes in chip order.
const OUTCOMES: [&str; 6] = [
    "succeeded",
    "proposed",
    "failed",
    "skipped",
    "running",
    "interrupted",
];

/// `/sessions/:id` view (the live part is also `/fragments/sessions/:id`).
#[derive(Debug, Clone)]
pub struct SessionDetailView {
    pub s: SessionItem,
    pub config: String,
    pub has_config: bool,
    pub rollup: Vec<RollupItem>,
    pub outcome_chips: Vec<Chip>,
    pub outcome: String,
    pub pager: Pager,
    pub packages_text: String,
    pub phases_text: String,
    pub errors: Vec<PhaseError>,
    pub more_errors: usize,
    pub logs: Vec<LogItem>,
    pub cycle: Option<CycleItem>,
    pub live_href: String,
    pub api_href: String,
}

/// Errors shown in full on the session page.
const SESSION_ERRORS: usize = 50;
/// Failure logs shown on the session page.
const SESSION_LOGS: i64 = 20;

async fn build_detail(
    pool: &SqlitePool,
    id: &str,
    params: &Params,
    now: DateTime<Utc>,
) -> AppResult<SessionDetailView> {
    let Some(row) = q::get(pool, id).await? else {
        return Err(AppError::NotFound(format!(
            "There is no session with id “{id}”."
        )));
    };
    let s = SessionItem::new(&row.session, now);
    let phases = q::phases(pool, id).await?;
    let all = rollup(&phases, s.is_running, now);
    let logs = failures::for_session(pool, id, SESSION_LOGS).await?;
    let cycle = watch::cycle_for_session(pool, id).await?;

    let outcome = param(params, "outcome")
        .filter(|o| OUTCOMES.contains(&o.as_str()))
        .unwrap_or_default();
    let session_path = format::session_href(id);
    let mut outcome_chips = vec![chip(
        "All",
        Some(all.len() as i64),
        session_path.clone(),
        outcome.is_empty(),
        "neutral",
    )];
    for o in OUTCOMES {
        let n = all.iter().filter(|r| r.outcome_key == o).count();
        if n > 0 || outcome == o {
            outcome_chips.push(chip(
                o,
                Some(n as i64),
                href(&session_path, &query_string(&[("outcome", o)])),
                outcome == o,
                match o {
                    "succeeded" => "good",
                    "proposed" | "running" => "info",
                    "failed" => "critical",
                    "interrupted" => "serious",
                    _ => "neutral",
                },
            ));
        }
    }

    let filtered: Vec<RollupItem> = all
        .iter()
        .filter(|r| outcome.is_empty() || r.outcome_key == outcome)
        .cloned()
        .collect();
    let base = query_string(&[("outcome", &outcome)]);
    let pager = Pager::new(
        &session_path,
        &base,
        parse_page(params.get("page").map(String::as_str)),
        ROLLUP_PER_PAGE,
        filtered.len() as i64,
    );
    let offset = usize::try_from(pager.offset(ROLLUP_PER_PAGE)).unwrap_or(0);
    let page_items: Vec<RollupItem> = filtered
        .into_iter()
        .skip(offset)
        .take(usize::try_from(ROLLUP_PER_PAGE).unwrap_or(100))
        .collect();

    let failed: Vec<PhaseError> = phases
        .iter()
        .filter(|p| p.status == "failed")
        .map(|p| PhaseError::new(p, now))
        .collect();
    let more_errors = failed.len().saturating_sub(SESSION_ERRORS);

    let mut live_query = base.clone();
    if pager.page > 1 {
        if !live_query.is_empty() {
            live_query.push('&');
        }
        live_query.push_str(&format!("page={}", pager.page));
    }
    let config = row
        .config_json
        .as_deref()
        .map(pretty_json)
        .unwrap_or_default();

    Ok(SessionDetailView {
        has_config: !config.is_empty(),
        config,
        packages_text: thousands(all.len() as i64),
        phases_text: thousands(phases.len() as i64),
        rollup: page_items,
        outcome_chips,
        outcome,
        pager,
        errors: failed.into_iter().take(SESSION_ERRORS).collect(),
        more_errors,
        logs: logs.iter().map(|l| LogItem::new(l, now)).collect(),
        cycle: cycle.as_ref().map(|c| CycleItem::new(c, now)),
        live_href: href(
            &format!("/fragments/sessions/{}", format::url_encode(id)),
            &live_query,
        ),
        api_href: format!("/api/sessions/{}/phases", format::url_encode(id)),
        s,
    })
}

/// `GET /sessions/:id`
pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<Params>,
) -> AppResult<SessionDetailTemplate> {
    Ok(SessionDetailTemplate {
        s: build_detail(state.db.pool(), &id, &params, Utc::now()).await?,
    })
}

/// `GET /fragments/sessions/:id`
pub async fn live_fragment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<Params>,
) -> AppResult<SessionLiveFragment> {
    Ok(SessionLiveFragment {
        s: build_detail(state.db.pool(), &id, &params, Utc::now()).await?,
    })
}
