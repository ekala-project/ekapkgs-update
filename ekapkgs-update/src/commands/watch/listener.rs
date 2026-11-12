//! Main event loop for the watch command.
//!
//! 1. On startup every repo is synced (fast-forward) and indexed.
//! 2. All indexed upstream sources are polled immediately, then every
//!    `poll_interval`. Each poll is recorded in `watch_cycles`; each source's
//!    health in `upstream_sources`; each package's observation in `updates`.
//! 3. The first observation of a package records its baseline and dispatches
//!    nothing (observe-only bootstrap). Later releases newer than the
//!    baseline are dispatched to the updater service.
//! 4. An update session is created only for poll cycles that dispatch work.
//! 5. Repos are re-synced and re-indexed every `index_refresh_interval`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};

use super::feeds::{poll_source, upstream_source_from_index};
use super::{ResolvedRepo, index, sync};
use crate::commands::run::{UpdateRequest, UpdaterServiceConfig};
use crate::database::{
    Database, IndexOutcome, SessionMode, SessionStatus, SourcePoll, UpdateRecord,
    UpstreamIndexEntry, WatchCycleKind, WatchCycleStats,
};
use crate::vcs_sources::{Release, compare_version_components, find_best_release};

/// Concurrent feed / API fetches per poll cycle.
const POLL_CONCURRENCY: usize = 8;
/// Uneventful poll cycles are kept this long.
const CYCLE_RETENTION_DAYS: i64 = 7;
/// Shortest allowed poll / index interval (`interval` panics on zero).
const MIN_INTERVAL: Duration = Duration::from_secs(60);

/// `(repo, attr_path, version)` already reported by a dry-run dispatch.
type DryRunReported = HashSet<(String, String, String)>;

/// Run the watch event loop until Ctrl-C.
pub async fn run_watch_loop(
    repos: Arc<Vec<ResolvedRepo>>,
    db: Database,
    updater_config: UpdaterServiceConfig,
    poll_interval: Duration,
    index_refresh_interval: Duration,
    session_config: serde_json::Value,
) -> anyhow::Result<()> {
    refresh_repos(&repos, &db).await;
    let mut sources = load_sources(&db, None).await;

    let poll_interval = poll_interval.max(MIN_INTERVAL);
    let index_refresh_interval = index_refresh_interval.max(MIN_INTERVAL);

    // The first poll tick fires immediately: that poll records baselines.
    let mut poll_ticker = tokio::time::interval(poll_interval);
    poll_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut index_ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + index_refresh_interval,
        index_refresh_interval,
    );
    index_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    // Dry runs leave no trace in the database, so without this every poll
    // would report (and open a session for) the same releases again.
    let mut dry_run_reported = DryRunReported::new();

    loop {
        tokio::select! {
            _ = poll_ticker.tick() => {
                run_poll_cycle(
                    &sources,
                    &repos,
                    &db,
                    &updater_config,
                    &session_config,
                    &mut dry_run_reported,
                )
                .await;
            },
            _ = index_ticker.tick() => {
                info!("Refreshing repositories and upstream indexes...");
                refresh_repos(&repos, &db).await;
                sources = load_sources(&db, Some(sources)).await;
            },
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down watch service...");
                break;
            },
        }
    }

    Ok(())
}

/// Load the source list, keeping `previous` if the database read fails.
async fn load_sources(
    db: &Database,
    previous: Option<Vec<UpstreamIndexEntry>>,
) -> Vec<UpstreamIndexEntry> {
    match db.get_all_upstream_sources().await {
        Ok(sources) => {
            info!("Watching {} unique upstream sources", sources.len());
            sources
        },
        Err(e) => {
            warn!("Failed to load upstream sources: {:#}", e);
            previous.unwrap_or_default()
        },
    }
}

/// Sync and re-index every repo (sequentially: each index build already
/// runs nix-eval-jobs with many workers).
async fn refresh_repos(repos: &[ResolvedRepo], db: &Database) {
    for repo in repos {
        sync::sync_repo(repo, db).await;
        index_repo(repo, db).await;
    }
}

/// Build one repo's index, recording it as an `index` watch cycle and in
/// `watched_repos`.
async fn index_repo(repo: &ResolvedRepo, db: &Database) {
    let cycle = db
        .start_watch_cycle(WatchCycleKind::Index, Some(&repo.name))
        .await
        .map_err(|e| warn!("[{}] Failed to record index cycle: {:#}", repo.label(), e))
        .ok();
    if let Err(e) = db.record_repo_index_start(&repo.name).await {
        warn!("[{}] Failed to record index start: {:#}", repo.label(), e);
    }

    let started = Instant::now();
    let stats = index::build_upstream_index(&repo.name, &repo.eval_entry_point, db, None).await;
    let outcome = IndexOutcome {
        error: stats.stream_error.clone(),
        duration_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
        evaluated: stats.evaluated,
        indexed: stats.indexed,
        skipped: stats.skipped,
        errors: stats.errors,
    };

    if let Err(e) = db.record_repo_index_result(&repo.name, &outcome).await {
        warn!("[{}] Failed to record index result: {:#}", repo.label(), e);
    }
    if let Some(id) = cycle {
        let cycle_stats = WatchCycleStats {
            packages_observed: stats.indexed,
            ..WatchCycleStats::default()
        };
        if let Err(e) = db
            .finish_watch_cycle(id, &cycle_stats, outcome.error.as_deref())
            .await
        {
            warn!("[{}] Failed to finish index cycle: {:#}", repo.label(), e);
        }
    }
}

// ── Poll cycle ────────────────────────────────────────────────────────

/// What a poll does with one package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// First observation: the baseline was recorded, nothing dispatched.
    Baseline,
    /// No newer compatible release.
    UpToDate,
    /// Newer than the package, but not newer than the baseline: part of the
    /// pre-existing backlog (left to `ekapkgs-update run`).
    Backlog,
    /// This version was already proposed.
    AlreadyProposed,
    /// This version failed recently; waiting for the backoff to expire.
    Backoff,
    /// Another attr path with the same source, current and target version
    /// (typically an alias) is handled instead.
    Duplicate,
    Dispatch,
}

/// Decide whether to dispatch an update for one package. `prev` is the
/// package's `updates` record *before* this observation.
fn dispatch_decision(
    current: &str,
    latest: &str,
    prev: Option<&UpdateRecord>,
    now: DateTime<Utc>,
) -> Decision {
    let Some(prev) = prev else {
        return Decision::Baseline;
    };
    let Some(baseline) = prev.baseline_version.as_deref() else {
        return Decision::Baseline;
    };
    if current == latest || !is_newer(latest, current) {
        return Decision::UpToDate;
    }
    if !is_newer(latest, baseline) {
        return Decision::Backlog;
    }
    if prev.proposed_version.as_deref() == Some(latest) {
        return Decision::AlreadyProposed;
    }
    if prev.failed_version.as_deref() == Some(latest) && prev.in_backoff(now) {
        return Decision::Backoff;
    }
    Decision::Dispatch
}

fn is_newer(candidate: &str, than: &str) -> bool {
    compare_version_components(candidate, than) == std::cmp::Ordering::Greater
}

/// One package's observation in a poll cycle.
#[derive(Debug)]
struct Observation {
    entry: UpstreamIndexEntry,
    current: String,
    latest: String,
    decision: Decision,
}

/// Collapse aliases: among packages in the same repo with the same current
/// and target version (one source), only the shortest attr path is
/// dispatched, and none if any of them was already proposed or is backing
/// off for that version.
fn resolve_siblings(observations: &mut [Observation]) {
    let mut groups: HashMap<(String, String, String), Vec<usize>> = HashMap::new();
    for (i, o) in observations.iter().enumerate() {
        if matches!(
            o.decision,
            Decision::Dispatch | Decision::AlreadyProposed | Decision::Backoff
        ) {
            groups
                .entry((
                    o.entry.repo_name.clone(),
                    o.current.clone(),
                    o.latest.clone(),
                ))
                .or_default()
                .push(i);
        }
    }
    for members in groups.values() {
        let handled = members.iter().any(|&i| {
            matches!(
                observations[i].decision,
                Decision::AlreadyProposed | Decision::Backoff
            )
        });
        let chosen = if handled {
            None
        } else {
            members
                .iter()
                .copied()
                .filter(|&i| observations[i].decision == Decision::Dispatch)
                .min_by(|&a, &b| {
                    let (a, b) = (
                        &observations[a].entry.attr_path,
                        &observations[b].entry.attr_path,
                    );
                    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
                })
        };
        for &i in members {
            if observations[i].decision == Decision::Dispatch && Some(i) != chosen {
                observations[i].decision = Decision::Duplicate;
            }
        }
    }
}

/// Newest stable release of a source, ignoring per-package strategies.
fn newest_stable(releases: &[Release]) -> Option<&Release> {
    releases
        .iter()
        .filter(|r| !r.is_prerelease)
        .max_by(|a, b| compare_version_components(a.version(), b.version()))
}

/// Best release for one package (its strategy / regex / prerelease
/// settings). `current` when nothing newer is compatible.
fn best_for_package(releases: &[Release], entry: &UpstreamIndexEntry, current: &str) -> String {
    let regex = entry.version_regex.as_deref();
    find_best_release(
        releases.to_vec(),
        current,
        entry.strategy(),
        None,
        None,
        regex,
        entry.include_prereleases,
    )
    .map_or_else(
        |_| current.to_owned(),
        |r| {
            r.resolved_version(regex)
                .unwrap_or_else(|_| r.version().to_owned())
        },
    )
}

/// Result of polling one source.
#[derive(Debug, Default)]
struct SourceOutcome {
    failed: bool,
    new_release: bool,
    observations: Vec<Observation>,
}

/// Poll one source and record observations for every package tracking it.
async fn poll_one_source(
    db: &Database,
    source_row: &UpstreamIndexEntry,
    configured: &HashSet<String>,
    now: DateTime<Utc>,
) -> SourceOutcome {
    let (kind, key, instance) = (
        source_row.upstream_type.as_str(),
        source_row.upstream_key.as_str(),
        source_row.instance.as_deref(),
    );
    let mut outcome = SourceOutcome::default();

    let Some(source) = upstream_source_from_index(kind, key, instance) else {
        debug!("{}/{}: cannot reconstruct upstream source", kind, key);
        return outcome;
    };

    let releases = match poll_source(&source).await {
        Ok(releases) => releases,
        Err(e) => {
            let message = format!("{e:#}");
            debug!("{}/{}: poll failed: {}", kind, key, message);
            outcome.failed = true;
            if let Err(e) = db
                .record_source_poll(
                    kind,
                    key,
                    instance,
                    &SourcePoll::Failure { error: &message },
                )
                .await
            {
                warn!("{}/{}: failed to record poll failure: {:#}", kind, key, e);
            }
            return outcome;
        },
    };

    let newest = newest_stable(&releases);
    let newest_version = newest.map(Release::version);
    match db
        .record_source_poll(
            kind,
            key,
            instance,
            &SourcePoll::Success {
                latest_version: newest_version,
                latest_tag: newest.map(|r| r.tag_name.as_str()),
                release_count: releases.len(),
            },
        )
        .await
    {
        Ok(changed) => outcome.new_release = changed,
        Err(e) => warn!("{}/{}: failed to record poll: {:#}", kind, key, e),
    }
    if releases.is_empty() {
        return outcome;
    }

    let entries = match db.get_attr_paths_for_source(kind, key, instance).await {
        Ok(entries) => entries,
        Err(e) => {
            warn!("{}/{}: failed to load packages: {:#}", kind, key, e);
            return outcome;
        },
    };

    for entry in entries {
        if !configured.contains(&entry.repo_name) {
            continue;
        }
        let Some(current) = entry.current_version.clone().filter(|v| !v.is_empty()) else {
            continue;
        };
        let latest = best_for_package(&releases, &entry, &current);
        let prev = match db
            .record_observation(&entry.repo_name, &entry.attr_path, &current, &latest)
            .await
        {
            Ok(prev) => prev,
            Err(e) => {
                warn!("{}: failed to record observation: {:#}", entry.attr_path, e);
                continue;
            },
        };
        let decision = dispatch_decision(&current, &latest, prev.as_ref(), now);
        outcome.observations.push(Observation {
            entry,
            current,
            latest,
            decision,
        });
    }
    resolve_siblings(&mut outcome.observations);
    outcome
}

/// Run one poll cycle, recording it in `watch_cycles`.
async fn run_poll_cycle(
    sources: &[UpstreamIndexEntry],
    repos: &[ResolvedRepo],
    db: &Database,
    updater_config: &UpdaterServiceConfig,
    session_config: &serde_json::Value,
    dry_run_reported: &mut DryRunReported,
) {
    let cycle_id = db
        .start_watch_cycle(WatchCycleKind::Poll, None)
        .await
        .map_err(|e| warn!("Failed to record poll cycle: {:#}", e))
        .ok();
    let started = Instant::now();
    let mut stats = WatchCycleStats::default();
    let result = poll_cycle(
        sources,
        repos,
        db,
        updater_config,
        session_config,
        dry_run_reported,
        cycle_id,
        &mut stats,
    )
    .await;
    let error = result.err().map(|e| format!("{e:#}"));

    if let Some(e) = &error {
        warn!("Poll cycle failed: {}", e);
    } else if stats.dispatched > 0 || stats.bootstrap {
        info!(
            "Poll cycle complete in {:.0?}: {} sources ({} failed), {} packages observed, \
             {} new releases, {} dispatched ({} updated, {} failed){}",
            started.elapsed(),
            stats.sources_polled,
            stats.sources_failed,
            stats.packages_observed,
            stats.new_releases,
            stats.dispatched,
            stats.updated,
            stats.failed,
            if stats.bootstrap {
                "; baselines recorded"
            } else {
                ""
            }
        );
    } else {
        debug!(
            "Poll cycle complete: {} sources, nothing to dispatch",
            stats.sources_polled
        );
    }

    if let Some(id) = cycle_id {
        if let Err(e) = db.finish_watch_cycle(id, &stats, error.as_deref()).await {
            warn!("Failed to finish poll cycle: {:#}", e);
        }
    }
    if let Err(e) = db.prune_watch_cycles(CYCLE_RETENTION_DAYS).await {
        debug!("Failed to prune watch cycles: {:#}", e);
    }
}

async fn poll_cycle(
    sources: &[UpstreamIndexEntry],
    repos: &[ResolvedRepo],
    db: &Database,
    updater_config: &UpdaterServiceConfig,
    session_config: &serde_json::Value,
    dry_run_reported: &mut DryRunReported,
    cycle_id: Option<i64>,
    stats: &mut WatchCycleStats,
) -> anyhow::Result<()> {
    info!(
        "Polling {} upstream sources for new releases...",
        sources.len()
    );
    let configured: Arc<HashSet<String>> = Arc::new(repos.iter().map(|r| r.name.clone()).collect());
    let semaphore = Arc::new(Semaphore::new(POLL_CONCURRENCY));
    let now = Utc::now();

    let mut tasks = JoinSet::new();
    for source_row in sources {
        let source_row = source_row.clone();
        let db = db.clone();
        let configured = Arc::clone(&configured);
        let semaphore = Arc::clone(&semaphore);
        tasks.spawn(async move {
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return SourceOutcome::default();
            };
            poll_one_source(&db, &source_row, &configured, now).await
        });
    }

    let mut observations = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        stats.sources_polled += 1;
        match joined {
            Ok(outcome) => {
                stats.sources_failed += usize::from(outcome.failed);
                stats.new_releases += usize::from(outcome.new_release);
                observations.extend(outcome.observations);
            },
            Err(e) => {
                warn!("Source poll task panicked: {}", e);
                stats.sources_failed += 1;
            },
        }
    }

    stats.packages_observed = observations.len();
    stats.bootstrap = observations
        .iter()
        .any(|o| o.decision == Decision::Baseline);
    let backoff = observations
        .iter()
        .filter(|o| o.decision == Decision::Backoff)
        .count();

    let mut observed_repos: Vec<&str> = observations
        .iter()
        .map(|o| o.entry.repo_name.as_str())
        .collect();
    observed_repos.sort_unstable();
    observed_repos.dedup();
    for repo in &observed_repos {
        if let Err(e) = db.mark_repo_bootstrapped(repo).await {
            debug!("[{}] Failed to mark bootstrapped: {:#}", repo, e);
        }
    }

    let mut requests: Vec<UpdateRequest> = observations
        .iter()
        .filter(|o| o.decision == Decision::Dispatch)
        .filter_map(|o| {
            let repo = repos.iter().find(|r| r.name == o.entry.repo_name)?;
            info!(
                "[{}] {}: new release {} -> {}",
                repo.label(),
                o.entry.attr_path,
                o.current,
                o.latest
            );
            Some(UpdateRequest {
                attr_path: o.entry.attr_path.clone(),
                drv_path: None,
                current_version: o.current.clone(),
                new_version: o.latest.clone(),
                repo_name: Some(repo.name.clone()),
                eval_entry_point: Some(repo.eval_entry_point.clone()),
                pr_config: repo.pr_config.clone(),
                fork: Some(repo.fork.clone()),
            })
        })
        .collect();
    if updater_config.dry_run {
        requests.retain(|r| {
            dry_run_reported.insert((
                r.repo().to_owned(),
                r.attr_path.clone(),
                r.new_version.clone(),
            ))
        });
    }
    if requests.is_empty() {
        return Ok(());
    }
    requests.sort_by(|a, b| (a.repo(), &a.attr_path).cmp(&(b.repo(), &b.attr_path)));
    stats.dispatched = requests.len();

    // Lazy session: only cycles that dispatch work get one.
    let mut config = session_config.clone();
    if let Some(obj) = config.as_object_mut() {
        obj.insert("cycle_id".to_owned(), serde_json::json!(cycle_id));
        obj.insert("dispatched".to_owned(), serde_json::json!(requests.len()));
    }
    let session_id = db
        .create_session(SessionMode::Watch, Some(config.to_string()))
        .await?;
    stats.session_id = Some(session_id.clone());

    let mut cycle_config = updater_config.clone();
    cycle_config.session_id = session_id.clone();
    let (tx, rx) = mpsc::unbounded_channel();
    for req in requests {
        tx.send(req)
            .map_err(|_| anyhow::anyhow!("updater channel closed"))?;
    }
    drop(tx);

    let outcome = cycle_config.run_service(rx, db.clone()).await;
    stats.updated = outcome.updated + outcome.dry_run;
    stats.failed = outcome.failed;

    let status = if outcome.failed > 0 {
        SessionStatus::Failed
    } else {
        SessionStatus::Completed
    };
    db.finalize_session(&session_id, status, stats.packages_observed, backoff)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::UpdateStatus;

    fn record(baseline: Option<&str>) -> UpdateRecord {
        UpdateRecord {
            status: UpdateStatus::Pending,
            current_version: Some("1.0".into()),
            latest_upstream_version: None,
            proposed_version: None,
            baseline_version: baseline.map(str::to_owned),
            failed_version: None,
            failure_count: 0,
            last_error: None,
            last_checked: None,
            last_attempted: None,
            next_attempt: None,
        }
    }

    #[test]
    fn decisions() {
        let now = Utc::now();
        let future = now + chrono::Duration::days(1);
        let past = now - chrono::Duration::days(1);

        let proposed = UpdateRecord {
            proposed_version: Some("1.3".into()),
            ..record(Some("1.1"))
        };
        let failed_backoff = UpdateRecord {
            failed_version: Some("1.3".into()),
            next_attempt: Some(future),
            ..record(Some("1.1"))
        };
        let failed_expired = UpdateRecord {
            failed_version: Some("1.3".into()),
            next_attempt: Some(past),
            ..record(Some("1.1"))
        };
        let failed_other = UpdateRecord {
            failed_version: Some("1.2".into()),
            next_attempt: Some(future),
            ..record(Some("1.1"))
        };

        let cases: Vec<(&str, &str, Option<UpdateRecord>, Decision)> = vec![
            ("1.0", "1.3", None, Decision::Baseline),
            // Registered by the indexer but never observed.
            ("1.0", "1.3", Some(record(None)), Decision::Baseline),
            ("1.0", "1.0", Some(record(Some("1.0"))), Decision::UpToDate),
            ("1.2", "1.1", Some(record(Some("1.1"))), Decision::UpToDate),
            // Backlog: newer than current but already known at bootstrap.
            ("1.0", "1.1", Some(record(Some("1.1"))), Decision::Backlog),
            ("1.0", "1.3", Some(record(Some("1.1"))), Decision::Dispatch),
            ("1.0", "1.3", Some(proposed), Decision::AlreadyProposed),
            ("1.0", "1.3", Some(failed_backoff), Decision::Backoff),
            ("1.0", "1.3", Some(failed_expired), Decision::Dispatch),
            // A failure of an older target doesn't block a new release.
            ("1.0", "1.3", Some(failed_other), Decision::Dispatch),
        ];
        for (current, latest, prev, expected) in cases {
            assert_eq!(
                dispatch_decision(current, latest, prev.as_ref(), now),
                expected,
                "{current} -> {latest} with {prev:?}"
            );
        }
    }

    fn obs(repo: &str, attr: &str, current: &str, latest: &str, decision: Decision) -> Observation {
        Observation {
            entry: UpstreamIndexEntry {
                repo_name: repo.into(),
                attr_path: attr.into(),
                upstream_type: "github".into(),
                upstream_key: "o/r".into(),
                instance: None,
                current_version: Some(current.into()),
                src_url: None,
                pname: None,
                semver_strategy: None,
                version_regex: None,
                include_prereleases: false,
            },
            current: current.into(),
            latest: latest.into(),
            decision,
        }
    }

    #[test]
    fn siblings_collapse_to_shortest_attr() {
        use Decision::{AlreadyProposed, Dispatch, Duplicate, UpToDate};
        let mut observations = vec![
            obs("", "python312Packages.foo", "1.0", "1.1", Dispatch),
            obs("", "foo", "1.0", "1.1", Dispatch),
            obs("", "python3Packages.foo", "1.0", "1.1", Dispatch),
            // Other repo: independent.
            obs("other", "foo", "1.0", "1.1", Dispatch),
            // Different current version (another variant): independent.
            obs("", "foo_0_9", "0.9", "0.9.5", Dispatch),
            // Already proposed via a sibling: nothing new is dispatched.
            obs("", "bar", "2.0", "2.1", AlreadyProposed),
            obs("", "bar-alias", "2.0", "2.1", Dispatch),
            obs("", "baz", "1.0", "1.0", UpToDate),
        ];
        resolve_siblings(&mut observations);
        let decisions: Vec<(&str, &str, Decision)> = observations
            .iter()
            .map(|o| {
                (
                    o.entry.repo_name.as_str(),
                    o.entry.attr_path.as_str(),
                    o.decision,
                )
            })
            .collect();
        assert_eq!(
            decisions,
            vec![
                ("", "python312Packages.foo", Duplicate),
                ("", "foo", Dispatch),
                ("", "python3Packages.foo", Duplicate),
                ("other", "foo", Dispatch),
                ("", "foo_0_9", Dispatch),
                ("", "bar", AlreadyProposed),
                ("", "bar-alias", Duplicate),
                ("", "baz", UpToDate),
            ]
        );
    }

    #[test]
    fn package_strategy_is_respected() {
        let releases: Vec<Release> = ["v2.0.0", "v1.5.0", "v1.4.2", "v1.4.1-rc1"]
            .iter()
            .map(|t| Release {
                tag_name: (*t).to_owned(),
                is_prerelease: false,
            })
            .collect();
        let mut entry = obs("", "foo", "1.4.0", "", Decision::Dispatch).entry;

        assert_eq!(
            newest_stable(&releases).map(Release::version),
            Some("2.0.0")
        );
        assert_eq!(best_for_package(&releases, &entry, "1.4.0"), "2.0.0");
        entry.semver_strategy = Some("minor".into());
        assert_eq!(best_for_package(&releases, &entry, "1.4.0"), "1.5.0");
        entry.semver_strategy = Some("patch".into());
        assert_eq!(best_for_package(&releases, &entry, "1.4.0"), "1.4.2");
        // Nothing newer compatible: stays at current.
        assert_eq!(best_for_package(&releases, &entry, "1.4.2"), "1.4.2");
    }
}
