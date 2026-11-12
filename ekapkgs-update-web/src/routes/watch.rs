//! `/watch`: watched repositories, poll/index cycles and source health.

use axum::extract::{Query, State};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use super::{CYCLES_PER_PAGE, Params, chip, param};
use crate::error::AppResult;
use crate::format::{Chip, Pager, href, parse_page, query_string, thousands};
use crate::queries::watch as q;
use crate::state::AppState;
use crate::templates::WatchTemplate;
use crate::views::charts::{Meter, ranked_bars};
use crate::views::{CycleItem, RepoItem, SourceItem};

/// Source health for one upstream type.
#[derive(Debug, Clone)]
pub struct SourceTypeItem {
    pub kind: String,
    pub total: String,
    pub failing: i64,
    pub failing_text: String,
    pub never_polled: String,
    /// Bar length: share of sources (relative to the largest type).
    pub width: String,
    pub tip: String,
    pub failure: Meter,
}

/// `/watch` view.
#[derive(Debug, Clone)]
pub struct WatchPageView {
    pub configured: bool,
    pub repos: Vec<RepoItem>,
    pub cycles: Vec<CycleItem>,
    pub cycle_pager: Pager,
    pub kind_chips: Vec<Chip>,
    pub types: Vec<SourceTypeItem>,
    pub failing: Vec<SourceItem>,
    pub recent: Vec<SourceItem>,
    pub sources_total: String,
    pub failing_total: i64,
    pub failing_total_text: String,
    pub last_poll: Option<CycleItem>,
    /// Some repository has not completed its first (observe-only) poll.
    pub awaiting_bootstrap: bool,
    pub backlog: i64,
    pub backlog_text: String,
}

/// Failing sources listed.
const FAILING_LIMIT: i64 = 25;
/// Recently changed sources listed.
const RECENT_LIMIT: i64 = 25;

async fn build(
    pool: &SqlitePool,
    params: &Params,
    now: DateTime<Utc>,
) -> sqlx::Result<WatchPageView> {
    let kind = param(params, "kind").filter(|k| k == "poll" || k == "index");
    let repos = q::repos(pool).await?;
    let total_cycles = q::cycle_count(pool, kind.as_deref()).await?;
    let base = query_string(&[("kind", kind.as_deref().unwrap_or(""))]);
    let cycle_pager = Pager::new(
        "/watch",
        &base,
        parse_page(params.get("page").map(String::as_str)),
        CYCLES_PER_PAGE,
        total_cycles,
    );
    let cycles = q::cycles(
        pool,
        kind.as_deref(),
        CYCLES_PER_PAGE,
        cycle_pager.offset(CYCLES_PER_PAGE),
    )
    .await?;
    let all_cycles = if kind.is_some() {
        q::cycle_count(pool, None).await?
    } else {
        total_cycles
    };
    let polls = q::cycle_count(pool, Some("poll")).await?;
    let types = q::source_type_counts(pool).await?;
    let failing = q::failing_sources(pool, FAILING_LIMIT).await?;
    let recent = q::recent_releases(pool, RECENT_LIMIT).await?;
    let last_poll = q::last_poll(pool).await?;

    let current = kind.clone().unwrap_or_default();
    let kind_chips = vec![
        chip(
            "All cycles",
            Some(all_cycles),
            "/watch".to_owned(),
            current.is_empty(),
            "neutral",
        ),
        chip(
            "poll",
            Some(polls),
            href("/watch", "kind=poll"),
            current == "poll",
            "neutral",
        ),
        chip(
            "index",
            Some(all_cycles - polls),
            href("/watch", "kind=index"),
            current == "index",
            "neutral",
        ),
    ];

    let bars = ranked_bars(
        &types
            .iter()
            .map(|t| (t.upstream_type.clone(), t.total))
            .collect::<Vec<_>>(),
    );
    let type_items = types
        .iter()
        .zip(bars)
        .map(|(t, bar)| SourceTypeItem {
            kind: t.upstream_type.clone(),
            total: thousands(t.total),
            failing: t.failing,
            failing_text: thousands(t.failing),
            never_polled: thousands(t.never_polled),
            width: bar.width,
            tip: format!(
                "{}: {} sources, {} failing, {} never polled",
                t.upstream_type,
                thousands(t.total),
                thousands(t.failing),
                thousands(t.never_polled)
            ),
            failure: Meter::failure_rate(t.failing, t.total),
        })
        .collect();
    let sources_total: i64 = types.iter().map(|t| t.total).sum();
    let failing_total: i64 = types.iter().map(|t| t.failing).sum();
    let backlog: i64 = repos.iter().map(|r| r.backlog_count).sum();

    Ok(WatchPageView {
        configured: !repos.is_empty() || all_cycles > 0 || sources_total > 0,
        awaiting_bootstrap: repos.iter().any(|r| r.bootstrapped_at.is_none()),
        repos: repos.iter().map(|r| RepoItem::new(r, now)).collect(),
        cycles: cycles.iter().map(|c| CycleItem::new(c, now)).collect(),
        cycle_pager,
        kind_chips,
        types: type_items,
        failing: failing.iter().map(|s| SourceItem::new(s, now)).collect(),
        recent: recent.iter().map(|s| SourceItem::new(s, now)).collect(),
        sources_total: thousands(sources_total),
        failing_total,
        failing_total_text: thousands(failing_total),
        last_poll: last_poll.as_ref().map(|c| CycleItem::new(c, now)),
        backlog,
        backlog_text: thousands(backlog),
    })
}

/// `GET /watch`
pub async fn index(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> AppResult<WatchTemplate> {
    Ok(WatchTemplate {
        w: build(state.db.pool(), &params, Utc::now()).await?,
    })
}
