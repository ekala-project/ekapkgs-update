//! `/packages` and `/packages/:attr`.

use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use super::{PACKAGES_PER_PAGE, Params, chip, param};
use crate::error::{AppError, AppResult};
use crate::format::{
    self, Badge, Chip, Pager, SelectOption, Ts, href, humanize, parse_page, query_string,
    repo_from_param, repo_label, session_href, short_id,
};
use crate::queries::failures::LogRow;
use crate::queries::packages::{self as q, PackageFilter, PackagePhaseRow, PackageSort};
use crate::state::AppState;
use crate::templates::{PackageDetailTemplate, PackagesTemplate};
use crate::views::{
    AutofixItem, IndexItem, LogItem, PackageDetail, PackageItem, PhaseChip, PhaseError, SourceItem,
    autofix_items, outcome,
};

// ── List ─────────────────────────────────────────────────────────────────

/// A sortable column header.
#[derive(Debug, Clone)]
pub struct SortLink {
    pub href: String,
    pub active: bool,
}

/// Header links for each sort order.
#[derive(Debug, Clone)]
pub struct SortLinks {
    pub attr: SortLink,
    pub status: SortLink,
    pub checked: SortLink,
    pub rebuilds: SortLink,
    pub failures: SortLink,
}

/// `/packages` view.
#[derive(Debug, Clone)]
pub struct PackageListView {
    pub items: Vec<PackageItem>,
    pub pager: Pager,
    pub q: String,
    pub status: String,
    pub repo_options: Vec<SelectOption>,
    pub upstream_options: Vec<SelectOption>,
    pub sort_options: Vec<SelectOption>,
    pub status_chips: Vec<Chip>,
    pub sorts: SortLinks,
    pub has_filters: bool,
    /// Nothing tracked at all (as opposed to no matches).
    pub db_empty: bool,
    /// More than one repository is known, so show the repo column.
    pub show_repo: bool,
}

/// Parsed `/packages` (and `/api/packages`) parameters.
pub struct ListParams {
    pub filter: PackageFilter,
    pub sort: PackageSort,
    pub page: i64,
    /// `repo` as given (the display label), for links.
    pub repo_label: String,
}

impl ListParams {
    pub fn parse(params: &Params) -> Self {
        let repo_label = param(params, "repo").unwrap_or_default();
        let filter = PackageFilter {
            q: param(params, "q"),
            repo: (!repo_label.is_empty()).then(|| repo_from_param(&repo_label)),
            status: param(params, "status").filter(|s| q::is_known_status(s)),
            upstream: param(params, "upstream"),
        };
        Self {
            sort: PackageSort::parse(param(params, "sort").as_deref()),
            page: parse_page(params.get("page").map(String::as_str)),
            filter,
            repo_label,
        }
    }

    /// Query string for the current filters with overrides.
    fn query(&self, status: Option<&str>, sort: Option<PackageSort>) -> String {
        let status = status.unwrap_or(self.filter.status.as_deref().unwrap_or(""));
        let sort = sort.unwrap_or(self.sort);
        query_string(&[
            ("q", self.filter.q.as_deref().unwrap_or("")),
            ("repo", &self.repo_label),
            ("status", status),
            ("upstream", self.filter.upstream.as_deref().unwrap_or("")),
            (
                "sort",
                if sort == PackageSort::default() {
                    ""
                } else {
                    sort.as_str()
                },
            ),
        ])
    }
}

/// Status chips in display order: all, then the statuses with backlog
/// right after outdated.
const STATUS_CHIPS: [&str; 7] = [
    "up_to_date",
    "outdated",
    "backlog",
    "proposed",
    "failed",
    "upstream_error",
    "pending",
];

async fn build_list(
    pool: &SqlitePool,
    params: &Params,
    now: DateTime<Utc>,
) -> sqlx::Result<PackageListView> {
    let lp = ListParams::parse(params);
    let total = q::count(pool, &lp.filter).await?;
    let pager = Pager::new(
        "/packages",
        &lp.query(None, None),
        lp.page,
        PACKAGES_PER_PAGE,
        total,
    );
    let rows = q::list(
        pool,
        &lp.filter,
        lp.sort,
        PACKAGES_PER_PAGE,
        pager.offset(PACKAGES_PER_PAGE),
    )
    .await?;
    let counts = q::status_counts(pool, &lp.filter).await?;
    let all_counts =
        if lp.filter.q.is_none() && lp.filter.repo.is_none() && lp.filter.upstream.is_none() {
            counts.total
        } else {
            q::status_counts(pool, &PackageFilter::default())
                .await?
                .total
        };
    let repos = q::repos(pool).await?;
    let upstreams = q::upstream_types(pool).await?;

    let current_status = lp.filter.status.clone().unwrap_or_default();
    let mut status_chips = vec![chip(
        "All",
        Some(counts.total),
        href("/packages", &lp.query(Some(""), None)),
        current_status.is_empty(),
        "neutral",
    )];
    for status in STATUS_CHIPS {
        let label = if status == "backlog" {
            "backlog".to_owned()
        } else {
            humanize(status)
        };
        status_chips.push(chip(
            &label,
            Some(counts.get(status)),
            href("/packages", &lp.query(Some(status), None)),
            current_status == status,
            if status == "backlog" {
                "warn"
            } else {
                format::tone(status)
            },
        ));
    }

    let mut repo_options = vec![SelectOption::new("", "All repos", &lp.repo_label)];
    repo_options.extend(repos.iter().map(|r| {
        let label = repo_label(r);
        SelectOption::new(label, label, &lp.repo_label)
    }));
    let current_upstream = lp.filter.upstream.clone().unwrap_or_default();
    let mut upstream_options = vec![SelectOption::new("", "Any upstream", &current_upstream)];
    upstream_options.extend(
        upstreams
            .iter()
            .map(|u| SelectOption::new(u, u, &current_upstream)),
    );
    let sort_options = PackageSort::ALL
        .iter()
        .map(|s| SelectOption::new(s.as_str(), s.label(), lp.sort.as_str()))
        .collect();
    let link = |s: PackageSort| SortLink {
        href: href("/packages", &lp.query(None, Some(s))),
        active: lp.sort == s,
    };

    Ok(PackageListView {
        items: rows.iter().map(|r| PackageItem::new(r, now)).collect(),
        pager,
        q: lp.filter.q.clone().unwrap_or_default(),
        status: current_status,
        repo_options,
        upstream_options,
        sort_options,
        status_chips,
        sorts: SortLinks {
            attr: link(PackageSort::Attr),
            status: link(PackageSort::Status),
            checked: link(PackageSort::Checked),
            rebuilds: link(PackageSort::Rebuilds),
            failures: link(PackageSort::Failures),
        },
        has_filters: lp.filter.q.is_some()
            || lp.filter.repo.is_some()
            || lp.filter.status.is_some()
            || lp.filter.upstream.is_some(),
        db_empty: all_counts == 0,
        show_repo: repos.len() > 1,
    })
}

/// `GET /packages`
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> AppResult<PackagesTemplate> {
    Ok(PackagesTemplate {
        list: build_list(state.db.pool(), &params, Utc::now()).await?,
    })
}

// ── Detail ───────────────────────────────────────────────────────────────

/// One session's phases for this package.
#[derive(Debug, Clone)]
pub struct PhaseGroup {
    pub session_href: String,
    pub session_short: String,
    pub mode: String,
    pub started: Ts,
    pub outcome: Badge,
    pub chips: Vec<PhaseChip>,
    pub errors: Vec<PhaseError>,
    pub duration: String,
}

/// Sessions shown in the phase history.
const HISTORY_SESSIONS: usize = 25;

/// Group phase rows (newest first) by session, keeping session order.
pub fn phase_groups(rows: &[PackagePhaseRow], now: DateTime<Utc>) -> Vec<PhaseGroup> {
    let mut groups: Vec<(&str, Vec<&PackagePhaseRow>)> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|(id, _)| *id == row.phase.session_id.as_str())
        {
            Some((_, list)) => list.push(row),
            None => groups.push((row.phase.session_id.as_str(), vec![row])),
        }
    }
    groups
        .into_iter()
        .take(HISTORY_SESSIONS)
        .map(|(id, mut list)| {
            list.reverse();
            let phases: Vec<_> = list.iter().map(|r| &r.phase).collect();
            let first = list[0];
            let running = first.session_status.as_deref() == Some("running");
            let (_, badge) = outcome(&phases, running);
            let total: i64 = phases.iter().filter_map(|p| p.duration_ms).sum();
            PhaseGroup {
                session_href: session_href(id),
                session_short: short_id(id),
                mode: first.session_mode.clone().unwrap_or_default(),
                started: Ts::new(
                    first
                        .session_started_at
                        .as_deref()
                        .or(Some(&first.phase.started_at)),
                    now,
                ),
                outcome: badge,
                chips: phases.iter().map(|p| PhaseChip::new(p, now)).collect(),
                errors: phases
                    .iter()
                    .filter(|p| p.status == "failed")
                    .map(|p| PhaseError::new(p, now))
                    .collect(),
                duration: if total > 0 {
                    format::duration_ms(total)
                } else {
                    String::new()
                },
            }
        })
        .collect()
}

/// `/packages/:attr` view.
#[derive(Debug, Clone)]
pub struct PackageDetailView {
    pub attr_path: String,
    pub repo: String,
    /// One chip per repository the attribute appears in (shown when > 1).
    pub repo_chips: Vec<Chip>,
    pub detail: Option<PackageDetail>,
    pub index: Vec<IndexItem>,
    pub sources: Vec<SourceItem>,
    pub history: Vec<PhaseGroup>,
    pub logs: Vec<LogItem>,
    pub autofix: Vec<AutofixItem>,
    pub failures_href: String,
    pub api_href: String,
}

/// Phase rows fetched for the history.
const HISTORY_PHASES: i64 = 400;
/// Failure logs shown on the detail page.
const DETAIL_LOGS: i64 = 15;

async fn build_detail(
    pool: &SqlitePool,
    attr: &str,
    params: &Params,
    now: DateTime<Utc>,
) -> AppResult<PackageDetailView> {
    let repos = q::repos_mentioning(pool, attr).await?;
    if repos.is_empty() {
        return Err(AppError::NotFound(format!(
            "No package with attribute path “{attr}” has been seen by ekapkgs-update."
        )));
    }
    let repo = match param(params, "repo") {
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

    let rows = q::by_attr(pool, attr).await?;
    let detail = rows
        .iter()
        .find(|r| r.repo_name == repo)
        .map(|r| PackageDetail::new(r, now));
    let index = q::index_entries(pool, attr, &repo).await?;
    let sources = q::sources_for(pool, attr, &repo).await?;
    let phases = q::phases_for(pool, attr, &repo, HISTORY_PHASES).await?;
    let logs: Vec<LogRow> = q::logs_for(pool, attr, &repo, DETAIL_LOGS).await?;
    let queue = q::autofix_queue_for(pool, attr, &repo).await?;
    let attempts = q::autofix_attempts_for(pool, attr, &repo).await?;

    let repo_chips = repos
        .iter()
        .map(|r| {
            chip(
                repo_label(r),
                None,
                format::package_href(r, attr),
                *r == repo,
                "neutral",
            )
        })
        .collect();
    let label = repo_label(&repo).to_owned();

    Ok(PackageDetailView {
        attr_path: attr.to_owned(),
        failures_href: href("/failures", &query_string(&[("q", attr), ("repo", &label)])),
        api_href: href(
            &format!("/api/packages/{}", format::url_encode(attr)),
            &query_string(&[("repo", &label)]),
        ),
        repo: label,
        repo_chips,
        detail,
        index: index.iter().map(|i| IndexItem::new(i, now)).collect(),
        sources: sources.iter().map(|s| SourceItem::new(s, now)).collect(),
        history: phase_groups(&phases, now),
        logs: logs.iter().map(|l| LogItem::new(l, now)).collect(),
        autofix: autofix_items(&queue, &attempts, now),
    })
}

/// `GET /packages/:attr?repo=…`
pub async fn detail(
    State(state): State<AppState>,
    Path(attr): Path<String>,
    Query(params): Query<Params>,
) -> AppResult<PackageDetailTemplate> {
    Ok(PackageDetailTemplate {
        d: build_detail(state.db.pool(), &attr, &params, Utc::now()).await?,
    })
}
