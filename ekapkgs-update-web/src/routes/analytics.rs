//! `/analytics`: aggregates over sessions, phases, packages and autofix.

use axum::extract::{Query, State};
use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;

use super::dashboard::composition_bar;
use super::{Params, chip, param};
use crate::error::AppResult;
use crate::format::{
    Badge, Chip, DASH, Ts, duration_ms, href, package_href, pct_text, query_string, ratio_pct,
    repo_label, thousands,
};
use crate::queries::analytics::{self as q, REBUILD_BUCKETS};
use crate::queries::packages::StatusCounts;
use crate::queries::{PHASES, phase_rank, phase_short, stats};
use crate::state::AppState;
use crate::templates::AnalyticsTemplate;
use crate::views::charts::{Bar, ColumnChart, Meter, daily_columns, ranked_bars};
use crate::views::{PackageItem, SegBar};

/// Selectable time ranges: (param, label, days; 0 = all time).
const RANGES: [(&str, &str, i64); 4] = [
    ("7d", "7 days", 7),
    ("30d", "30 days", 30),
    ("90d", "90 days", 90),
    ("all", "All time", 0),
];
/// The daily trend never spans more than this many days.
const MAX_TREND_DAYS: i64 = 90;
/// Error types shown before folding the rest into "other".
const ERROR_TYPES_SHOWN: usize = 12;
/// Rows in the "top" tables.
const TOP_ROWS: i64 = 10;

/// Per-phase statistics row.
#[derive(Debug, Clone)]
pub struct PhaseStatItem {
    pub phase: String,
    pub label: String,
    pub total: String,
    pub succeeded: String,
    pub failed: String,
    pub skipped: String,
    pub running: String,
    pub failure: Meter,
    pub avg: String,
    pub p50: String,
    pub p90: String,
    pub max: String,
    pub failures_href: String,
}

/// Status composition for one repository.
#[derive(Debug, Clone)]
pub struct RepoStatusItem {
    pub repo: String,
    pub href: String,
    pub total: String,
    pub bar: SegBar,
    pub up_to_date: String,
    pub outdated: String,
    pub backlog: String,
    pub proposed: String,
    pub failed: String,
    pub upstream_error: String,
    pub pending: String,
}

/// Autofix attempts per outcome.
#[derive(Debug, Clone)]
pub struct AttemptOutcomeItem {
    pub badge: Badge,
    pub count: String,
    pub prompt_tokens: String,
    pub completion_tokens: String,
}

/// A package with many recorded failures.
#[derive(Debug, Clone)]
pub struct FailingItem {
    pub repo: String,
    pub attr_path: String,
    pub href: String,
    pub failures: String,
    pub last: Ts,
    pub consecutive: String,
    pub error_type: String,
}

/// `/analytics` view.
#[derive(Debug, Clone)]
pub struct AnalyticsView {
    pub range_chips: Vec<Chip>,
    pub range_label: String,
    pub sessions: String,
    pub attempted: String,
    pub succeeded: String,
    pub failed: String,
    pub success_rate: String,
    pub trend: ColumnChart,
    pub trend_note: String,
    pub error_types: Vec<Bar>,
    pub phases: Vec<PhaseStatItem>,
    pub repo_status: Vec<RepoStatusItem>,
    pub upstream_types: Vec<Bar>,
    pub rebuild_buckets: Vec<Bar>,
    pub top_rebuilds: Vec<PackageItem>,
    pub autofix_queue: Vec<Bar>,
    pub autofix_attempts: Vec<AttemptOutcomeItem>,
    pub top_failing: Vec<FailingItem>,
    pub has_packages: bool,
}

fn opt_ms(ms: Option<i64>) -> String {
    ms.map_or_else(|| DASH.to_owned(), duration_ms)
}

/// Fold rows beyond `shown` into an "other" bucket.
fn fold_other(rows: Vec<(String, i64)>, shown: usize) -> Vec<(String, i64)> {
    if rows.len() <= shown {
        return rows;
    }
    let rest: i64 = rows[shown..].iter().map(|(_, n)| n).sum();
    let mut out: Vec<(String, i64)> = rows.into_iter().take(shown).collect();
    out.push(("other".to_owned(), rest));
    out
}

async fn build(
    pool: &SqlitePool,
    params: &Params,
    now: DateTime<Utc>,
) -> sqlx::Result<AnalyticsView> {
    let range_param = param(params, "range").unwrap_or_default();
    let (range_key, range_label, days) = RANGES
        .iter()
        .find(|(k, _, _)| *k == range_param)
        .copied()
        .unwrap_or(RANGES[1]);
    let cutoff = if days > 0 {
        (now - Duration::days(days)).to_rfc3339()
    } else {
        String::new()
    };
    let trend_days = if days > 0 { days } else { MAX_TREND_DAYS };
    let trend_cutoff = (now - Duration::days(trend_days)).to_rfc3339();

    let range_chips = RANGES
        .iter()
        .map(|(key, label, _)| {
            chip(
                label,
                None,
                href(
                    "/analytics",
                    &query_string(&[("range", if *key == "30d" { "" } else { key })]),
                ),
                *key == range_key,
                "neutral",
            )
        })
        .collect();

    let outcomes = stats::outcomes_between(pool, &cutoff, "").await?;
    let daily = q::daily_outcomes(pool, &trend_cutoff).await?;
    let trend = daily_columns(
        &daily
            .iter()
            .map(|d| (d.day.clone(), d.succeeded, d.failed))
            .collect::<Vec<_>>(),
        now.date_naive(),
        trend_days,
    );

    // Error types (single hue, ranked)
    let errors = q::error_types(pool, &cutoff).await?;
    let error_rows = fold_other(
        errors.into_iter().map(|e| (e.label, e.count)).collect(),
        ERROR_TYPES_SHOWN,
    );
    let mut error_types = ranked_bars(&error_rows);
    for bar in &mut error_types {
        if bar.label != "other" {
            bar.href = Some(href(
                "/failures",
                &query_string(&[("error_type", &bar.label)]),
            ));
        }
    }

    // Phase statistics in workflow order
    let mut phase_rows = q::phase_stats(pool, &cutoff).await?;
    phase_rows.sort_by_key(|p| (phase_rank(&p.phase), p.phase.clone()));
    let percentiles = q::phase_percentiles(pool, &cutoff).await?;
    let phases = phase_rows
        .iter()
        .map(|p| {
            let (p50, p90) = percentiles
                .iter()
                .find(|(name, _, _)| *name == p.phase)
                .map_or((None, None), |(_, a, b)| (*a, *b));
            let finished = p.succeeded + p.failed;
            PhaseStatItem {
                label: phase_short(&p.phase),
                total: thousands(p.total),
                succeeded: thousands(p.succeeded),
                failed: thousands(p.failed),
                skipped: thousands(p.skipped),
                running: thousands(p.running),
                failure: Meter::failure_rate(p.failed, finished),
                avg: p
                    .avg_ms
                    .map_or_else(|| DASH.to_owned(), |ms| duration_ms(ms.round() as i64)),
                p50: opt_ms(p50),
                p90: opt_ms(p90),
                max: opt_ms(p.max_ms),
                failures_href: if PHASES.contains(&p.phase.as_str()) {
                    href("/failures", &query_string(&[("phase", &p.phase)]))
                } else {
                    String::new()
                },
                phase: p.phase.clone(),
            }
        })
        .collect();

    // Status per repository
    let mut repo_counts: Vec<(String, StatusCounts)> = Vec::new();
    for (repo, status, n, backlog) in q::status_by_repo(pool).await? {
        let idx = match repo_counts.iter().position(|(r, _)| *r == repo) {
            Some(i) => i,
            None => {
                repo_counts.push((repo, StatusCounts::default()));
                repo_counts.len() - 1
            },
        };
        let c = &mut repo_counts[idx].1;
        c.total += n;
        c.backlog += backlog;
        match status.as_str() {
            "pending" => c.pending += n,
            "up_to_date" => c.up_to_date += n,
            "outdated" => c.outdated += n,
            "proposed" => c.proposed += n,
            "failed" => c.failed += n,
            "upstream_error" => c.upstream_error += n,
            _ => {},
        }
    }
    let has_packages = !repo_counts.is_empty();
    let repo_status = repo_counts
        .iter()
        .map(|(repo, c)| {
            let label = repo_label(repo).to_owned();
            RepoStatusItem {
                href: href("/packages", &query_string(&[("repo", &label)])),
                repo: label,
                total: thousands(c.total),
                bar: composition_bar(c),
                up_to_date: thousands(c.up_to_date),
                outdated: thousands(c.outdated),
                backlog: thousands(c.backlog),
                proposed: thousands(c.proposed),
                failed: thousands(c.failed),
                upstream_error: thousands(c.upstream_error),
                pending: thousands(c.pending),
            }
        })
        .collect();

    // Upstream types
    let upstream_types = ranked_bars(
        &q::upstream_types(pool)
            .await?
            .into_iter()
            .map(|r| (r.label, r.count))
            .collect::<Vec<_>>(),
    );

    // Rebuild distribution in bucket order (a histogram, not a ranking)
    let buckets = q::rebuild_buckets(pool).await?;
    let ordered: Vec<(String, i64)> = REBUILD_BUCKETS
        .iter()
        .chain(std::iter::once(&"unknown"))
        .map(|b| {
            let n = buckets
                .iter()
                .find(|r| r.label == *b)
                .map_or(0, |r| r.count);
            ((*b).to_owned(), n)
        })
        .collect();
    let rebuild_buckets = if ordered.iter().all(|(_, n)| *n == 0) {
        Vec::new()
    } else {
        ranked_bars(&ordered)
    };
    let top_rebuilds = q::top_rebuilds(pool, TOP_ROWS)
        .await?
        .iter()
        .map(|p| PackageItem::new(p, now))
        .collect();

    // Autofix
    let mut autofix_queue = ranked_bars(
        &q::autofix_queue(pool, &cutoff)
            .await?
            .into_iter()
            .map(|r| (r.label, r.count))
            .collect::<Vec<_>>(),
    );
    for bar in &mut autofix_queue {
        bar.badge = Some(Badge::status(&bar.label));
    }
    let autofix_attempts = q::autofix_attempts(pool, &cutoff)
        .await?
        .iter()
        .map(|a| AttemptOutcomeItem {
            badge: Badge::status(&a.status),
            count: thousands(a.count),
            prompt_tokens: thousands(a.prompt_tokens),
            completion_tokens: thousands(a.completion_tokens),
        })
        .collect();

    // Most failing packages
    let top_failing = q::top_failing(pool, &cutoff, TOP_ROWS)
        .await?
        .iter()
        .map(|f| FailingItem {
            repo: repo_label(&f.repo_name).to_owned(),
            href: package_href(&f.repo_name, &f.attr_path),
            attr_path: f.attr_path.clone(),
            failures: thousands(f.failures),
            last: Ts::new(Some(&f.last_failure), now),
            consecutive: f.consecutive.map_or_else(|| DASH.to_owned(), thousands),
            error_type: f.last_error_type.clone().unwrap_or_default(),
        })
        .collect();

    Ok(AnalyticsView {
        range_chips,
        range_label: range_label.to_owned(),
        sessions: thousands(outcomes.sessions),
        attempted: thousands(outcomes.attempted),
        succeeded: thousands(outcomes.succeeded),
        failed: thousands(outcomes.failed),
        success_rate: pct_text(ratio_pct(
            outcomes.succeeded,
            outcomes.succeeded + outcomes.failed,
        )),
        trend_note: if days == 0 {
            format!("Last {MAX_TREND_DAYS} days (the trend is capped at {MAX_TREND_DAYS} days)")
        } else {
            format!("Last {trend_days} days")
        },
        trend,
        error_types,
        phases,
        repo_status,
        upstream_types,
        rebuild_buckets,
        top_rebuilds,
        autofix_queue,
        autofix_attempts,
        top_failing,
        has_packages,
    })
}

/// `GET /analytics?range=7d|30d|90d|all`
pub async fn index(
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> AppResult<AnalyticsTemplate> {
    Ok(AnalyticsTemplate {
        a: build(state.db.pool(), &params, Utc::now()).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::fold_other;

    #[test]
    fn fold_other_sums_the_tail() {
        let rows = vec![
            ("a".to_owned(), 5),
            ("b".to_owned(), 3),
            ("c".to_owned(), 2),
        ];
        assert_eq!(fold_other(rows.clone(), 5), rows);
        let folded = fold_other(rows, 1);
        assert_eq!(folded, vec![("a".to_owned(), 5), ("other".to_owned(), 5)]);
    }
}
