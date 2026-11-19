//! Main event loop for the watch command.
//!
//! 1. On startup every repo is synced (fast-forward) and indexed, unless its
//!    last successful index is younger than `index_refresh_interval` and was
//!    built from the same binary and configuration (a restart reuses it).
//! 2. Poll cycles run immediately, then every `poll_interval`. A cycle polls
//!    only the sources that are due according to `upstream_sources`
//!    (`last_polled_at`), so a restart does not re-poll everything; sources
//!    that were not found are rechecked daily. Each poll is recorded in
//!    `watch_cycles`; each source's health in `upstream_sources`; each
//!    package's observation in `updates`.
//! 3. The first observation of a package records its baseline and dispatches
//!    nothing (observe-only bootstrap). Later releases newer than the
//!    baseline are dispatched to the updater service.
//! 4. Updates are dispatched as soon as their source has been polled, so they
//!    build while the rest of the sweep is still polling. An update session
//!    (and its updater service) is created on a cycle's first dispatch. Each
//!    managed checkout is fast-forwarded before its repo's first update of
//!    the cycle, so update worktrees (created from `HEAD`) start from the
//!    current upstream branch. The cycle ends when every update has finished.
//! 5. Every `head_check_interval`, a background task re-indexes each repo
//!    whose upstream branch moved past the commit its index was built from
//!    (e.g. a merged PR), see [`refresh`]. Poll cycles reload the sources
//!    from the database, so they pick up re-indexed packages.
//! 6. Repos are re-synced and re-indexed every `index_refresh_interval`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};

use super::feeds::{
    PolledSource, failure_health, poll_source, update_version_for_entry, upstream_source_from_index,
};
use super::refresh::{self, index_config, index_repo_locked};
use super::{ResolvedRepo, sync};
use crate::commands::run::{UpdateRequest, UpdaterServiceConfig, UpdaterStats};
use crate::database::{
    Database, RepoIndexState, SessionMode, SessionStatus, SourceHealth, SourcePoll,
    SourcePollState, UpdateRecord, UpstreamIndexEntry, WatchCycleKind, WatchCycleStats,
};
use crate::vcs_sources::{Release, compare_version_components};

/// Concurrent feed / API fetches per poll cycle.
const POLL_CONCURRENCY: usize = 8;
/// Uneventful poll cycles are kept this long.
const CYCLE_RETENTION_DAYS: i64 = 7;
/// Shortest allowed poll / index interval (`interval` panics on zero).
const MIN_INTERVAL: Duration = Duration::from_secs(60);
/// How often a source that answered 404/410 is checked again.
const NOT_FOUND_RECHECK: Duration = Duration::from_secs(24 * 60 * 60);

/// `(repo, attr_path, version)` already reported by a dry-run dispatch.
type DryRunReported = HashSet<(String, String, String)>;

/// Run the watch event loop until Ctrl-C.
pub async fn run_watch_loop(
    repos: Arc<Vec<ResolvedRepo>>,
    db: Database,
    updater_config: UpdaterServiceConfig,
    poll_interval: Duration,
    index_refresh_interval: Duration,
    head_check_interval: Option<Duration>,
    session_config: serde_json::Value,
) -> anyhow::Result<()> {
    let poll_interval = poll_interval.max(MIN_INTERVAL);
    let index_refresh_interval = index_refresh_interval.max(MIN_INTERVAL);

    let next_index = refresh_repos(&repos, &db, Some(index_refresh_interval)).await;
    let mut sources = load_sources(&db, None).await;

    let head_watcher = head_check_interval.map(|interval| {
        let interval = interval.max(MIN_INTERVAL);
        info!(
            "Re-indexing repos whose upstream branch moved (checked every {})",
            humantime(interval)
        );
        tokio::spawn(refresh::watch_heads(
            Arc::clone(&repos),
            db.clone(),
            interval,
        ))
    });

    // The first poll tick fires immediately (polling only the sources that
    // are due); on a fresh database that poll records baselines.
    let mut poll_ticker = tokio::time::interval(poll_interval);
    poll_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut index_ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + next_index.max(MIN_INTERVAL),
        index_refresh_interval,
    );
    index_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    // Dry runs leave no trace in the database, so without this every poll
    // would report (and open a session for) the same releases again.
    let mut dry_run_reported = DryRunReported::new();

    loop {
        tokio::select! {
            _ = poll_ticker.tick() => {
                // The head watcher may have re-indexed since the last cycle.
                sources = load_sources(&db, Some(sources)).await;
                run_poll_cycle(
                    &sources,
                    poll_interval,
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
                refresh_repos(&repos, &db, None).await;
            },
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down watch service...");
                break;
            },
        }
    }

    if let Some(watcher) = head_watcher {
        watcher.abort();
    }
    Ok(())
}

/// Load the source list, keeping `previous` if the database read fails.
/// The count is logged when it changes.
async fn load_sources(
    db: &Database,
    previous: Option<Vec<UpstreamIndexEntry>>,
) -> Vec<UpstreamIndexEntry> {
    match db.get_all_upstream_sources().await {
        Ok(sources) => {
            if previous.as_ref().is_some_and(|p| p.len() == sources.len()) {
                debug!("Watching {} unique upstream sources", sources.len());
            } else if sources.is_empty() {
                warn!(
                    "Watching 0 upstream sources: no package could be indexed. Check the index \
                     log lines above (entry point, select expression, iets eval-jobs errors)."
                );
            } else {
                info!("Watching {} unique upstream sources", sources.len());
            }
            sources
        },
        Err(e) => {
            warn!("Failed to load upstream sources: {:#}", e);
            previous.unwrap_or_default()
        },
    }
}

/// Sync and re-index every repo (sequentially: each index build already
/// runs iets eval-jobs with many workers).
///
/// With `reuse_within`, a repo whose last index is still reusable
/// ([`index_reuse_remaining`]) is synced but not re-indexed. Returns how long
/// until the oldest reused index expires (`reuse_within` when every repo
/// was indexed, or the full refresh interval is unknown).
async fn refresh_repos(
    repos: &[ResolvedRepo],
    db: &Database,
    reuse_within: Option<Duration>,
) -> Duration {
    let mut next = reuse_within.unwrap_or(Duration::MAX);
    for repo in repos {
        let lock = crate::git::checkout_lock(&repo.repo_dir);
        let _checkout = lock.lock().await;
        sync::sync_repo_locked(repo, db).await;
        let head = sync::local_head(&repo.repo_dir).await;
        let config = index_config(repo);
        if let Some(interval) = reuse_within {
            let state = db
                .repo_index_state(&repo.name)
                .await
                .map_err(|e| warn!("[{}] Failed to read index state: {:#}", repo.label(), e))
                .ok()
                .flatten();
            if let Some(remaining) = index_reuse_remaining(
                state.as_ref(),
                &config,
                head.as_deref(),
                Utc::now(),
                interval,
            ) {
                info!(
                    "[{}] Reusing the upstream index from {} ago (re-index in {})",
                    repo.label(),
                    humantime(interval.saturating_sub(remaining)),
                    humantime(remaining)
                );
                next = next.min(remaining);
                continue;
            }
        }
        index_repo_locked(repo, db, config, head).await;
    }
    next
}

/// How much longer a repo's last index can be reused, or `None` when the
/// repo must be indexed now: the last build failed, was interrupted, indexed
/// nothing, was built from another binary, configuration or commit than
/// `head`, or is older than `interval`.
fn index_reuse_remaining(
    state: Option<&RepoIndexState>,
    config: &str,
    head: Option<&str>,
    now: DateTime<Utc>,
    interval: Duration,
) -> Option<Duration> {
    let state = state?;
    if state.status.as_deref() != Some("ok")
        || state.indexed_count <= 0
        || state.config.as_deref() != Some(config)
        || refresh::head_moved(head, state.indexed_commit.as_deref())
    {
        return None;
    }
    let age = (now - state.last_indexed_at?).to_std().ok()?;
    interval.checked_sub(age).filter(|r| !r.is_zero())
}

/// Whether a source should be polled in this cycle. A source is due half a
/// poll interval before its next scheduled poll, so a source polled late in
/// the previous cycle is not skipped until the one after; sources that were
/// not found are rechecked every [`NOT_FOUND_RECHECK`].
fn source_due(
    state: Option<&SourcePollState>,
    now: DateTime<Utc>,
    poll_interval: Duration,
) -> bool {
    let Some(last) = state.and_then(|s| s.last_polled_at) else {
        return true;
    };
    let interval = if state.is_some_and(|s| s.health == SourceHealth::NotFound) {
        NOT_FOUND_RECHECK.max(poll_interval)
    } else {
        poll_interval
    };
    // A last poll in the future (clock change) counts as just polled.
    let elapsed = (now - last).to_std().unwrap_or_default();
    elapsed + poll_interval / 2 >= interval
}

fn humantime(d: Duration) -> String {
    let mins = d.as_secs() / 60;
    if mins >= 60 {
        format!("{}h{:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
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
    update_version_for_entry(entry, releases).unwrap_or_else(|| current.to_owned())
}

/// Result of polling one source.
#[derive(Debug, Default)]
struct SourceOutcome {
    failed: bool,
    new_release: bool,
    observations: Vec<Observation>,
}

/// Why packages whose source polled fine but listed nothing are not observed.
const NO_RELEASES: &str = "upstream lists no releases or tags";

/// Record every configured package tracking a source as `upstream_error`
/// when the source could not be observed (poll failed, no releases, or not
/// pollable). Without this a newly indexed package stays `pending` forever.
async fn record_unobserved(
    db: &Database,
    source_row: &UpstreamIndexEntry,
    configured: &HashSet<String>,
    error: &str,
) {
    let (kind, key) = (
        source_row.upstream_type.as_str(),
        source_row.upstream_key.as_str(),
    );
    let entries = match db
        .get_attr_paths_for_source(kind, key, source_row.instance.as_deref())
        .await
    {
        Ok(entries) => entries,
        Err(e) => {
            warn!("{}/{}: failed to load packages: {:#}", kind, key, e);
            return;
        },
    };
    for entry in entries {
        if !configured.contains(&entry.repo_name) {
            continue;
        }
        let Some(current) = entry.current_version.as_deref().filter(|v| !v.is_empty()) else {
            continue;
        };
        if let Err(e) = db
            .record_upstream_error(&entry.repo_name, &entry.attr_path, current, error)
            .await
        {
            warn!(
                "{}: failed to record upstream error: {:#}",
                entry.attr_path, e
            );
        }
    }
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
        record_unobserved(
            db,
            source_row,
            configured,
            "cannot poll this upstream source",
        )
        .await;
        return outcome;
    };

    let PolledSource { releases, moved_to } = match poll_source(&source).await {
        Ok(polled) => polled,
        Err(e) => {
            let message = format!("{e:#}");
            let health = failure_health(&e);
            debug!("{}/{}: poll failed: {}", kind, key, message);
            outcome.failed = true;
            if let Err(e) = db
                .record_source_poll(
                    kind,
                    key,
                    instance,
                    &SourcePoll::Failure {
                        error: &message,
                        health,
                    },
                )
                .await
            {
                warn!("{}/{}: failed to record poll failure: {:#}", kind, key, e);
            }
            record_unobserved(db, source_row, configured, &message).await;
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
                moved_to: moved_to.as_deref(),
            },
        )
        .await
    {
        Ok(changed) => outcome.new_release = changed,
        Err(e) => warn!("{}/{}: failed to record poll: {:#}", kind, key, e),
    }
    if releases.is_empty() {
        record_unobserved(db, source_row, configured, NO_RELEASES).await;
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

/// The sources due for polling now ([`source_due`]); every source when the
/// poll state cannot be read.
async fn due_sources(
    db: &Database,
    sources: &[UpstreamIndexEntry],
    poll_interval: Duration,
) -> Vec<UpstreamIndexEntry> {
    let states = db.source_poll_states().await.unwrap_or_else(|e| {
        warn!(
            "Failed to read source poll state, polling every source: {:#}",
            e
        );
        HashMap::new()
    });
    let now = Utc::now();
    sources
        .iter()
        .filter(|s| {
            let key = (
                s.upstream_type.clone(),
                s.upstream_key.clone(),
                s.instance.clone().unwrap_or_default(),
            );
            source_due(states.get(&key), now, poll_interval)
        })
        .cloned()
        .collect()
}

/// Run one poll cycle over the sources that are due, recording it in
/// `watch_cycles` (no cycle when nothing is due).
async fn run_poll_cycle(
    sources: &[UpstreamIndexEntry],
    poll_interval: Duration,
    repos: &[ResolvedRepo],
    db: &Database,
    updater_config: &UpdaterServiceConfig,
    session_config: &serde_json::Value,
    dry_run_reported: &mut DryRunReported,
) {
    let due = due_sources(db, sources, poll_interval).await;
    if due.is_empty() {
        debug!(
            "No upstream source is due ({} polled recently)",
            sources.len()
        );
        return;
    }
    let cycle_id = db
        .start_watch_cycle(WatchCycleKind::Poll, None)
        .await
        .map_err(|e| warn!("Failed to record poll cycle: {:#}", e))
        .ok();
    let started = Instant::now();
    let mut stats = WatchCycleStats::default();
    let result = poll_cycle(
        &due,
        sources.len(),
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
    due: &[UpstreamIndexEntry],
    total: usize,
    repos: &[ResolvedRepo],
    db: &Database,
    updater_config: &UpdaterServiceConfig,
    session_config: &serde_json::Value,
    dry_run_reported: &mut DryRunReported,
    cycle_id: Option<i64>,
    stats: &mut WatchCycleStats,
) -> anyhow::Result<()> {
    info!(
        "Polling {} of {} upstream sources for new releases ({} polled recently)...",
        due.len(),
        total,
        total - due.len()
    );
    let now = Utc::now();
    let configured: Arc<HashSet<String>> = Arc::new(repos.iter().map(|r| r.name.clone()).collect());
    let semaphore = Arc::new(Semaphore::new(POLL_CONCURRENCY));

    let mut tasks = JoinSet::new();
    for source_row in due {
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

    // Updates start as soon as a polled source yields one, so building
    // overlaps with the rest of the sweep instead of waiting for it.
    let mut dispatcher = Dispatcher::new(
        repos,
        db,
        updater_config,
        session_config,
        cycle_id,
        dry_run_reported,
    );
    let mut packages_observed = 0;
    let mut backoff = 0;
    let mut observed_repos: HashSet<String> = HashSet::new();
    while let Some(joined) = tasks.join_next().await {
        stats.sources_polled += 1;
        let outcome = match joined {
            Ok(outcome) => outcome,
            Err(e) => {
                warn!("Source poll task panicked: {}", e);
                stats.sources_failed += 1;
                continue;
            },
        };
        stats.sources_failed += usize::from(outcome.failed);
        stats.new_releases += usize::from(outcome.new_release);
        packages_observed += outcome.observations.len();
        for o in &outcome.observations {
            match o.decision {
                Decision::Baseline => stats.bootstrap = true,
                Decision::Backoff => backoff += 1,
                Decision::Dispatch => dispatcher.dispatch(o).await,
                _ => {},
            }
            observed_repos.insert(o.entry.repo_name.clone());
        }
    }

    stats.packages_observed = packages_observed;
    for repo in &observed_repos {
        if let Err(e) = db.mark_repo_bootstrapped(repo).await {
            debug!("[{}] Failed to mark bootstrapped: {:#}", repo, e);
        }
    }

    dispatcher.finish(stats, packages_observed, backoff).await
}

/// Feeds a poll cycle's dispatched updates to an updater service that is
/// started (with its session) on the first dispatch.
struct Dispatcher<'a> {
    repos: &'a [ResolvedRepo],
    db: &'a Database,
    updater_config: &'a UpdaterServiceConfig,
    session_config: &'a serde_json::Value,
    cycle_id: Option<i64>,
    dry_run_reported: &'a mut DryRunReported,
    /// Repos whose checkout was synced before their first update this cycle.
    synced: HashSet<String>,
    service: Option<RunningService>,
    dispatched: usize,
    /// Set when the session could not be created; nothing more is dispatched.
    error: Option<anyhow::Error>,
}

struct RunningService {
    session_id: String,
    tx: mpsc::UnboundedSender<UpdateRequest>,
    handle: tokio::task::JoinHandle<UpdaterStats>,
}

impl<'a> Dispatcher<'a> {
    fn new(
        repos: &'a [ResolvedRepo],
        db: &'a Database,
        updater_config: &'a UpdaterServiceConfig,
        session_config: &'a serde_json::Value,
        cycle_id: Option<i64>,
        dry_run_reported: &'a mut DryRunReported,
    ) -> Self {
        Self {
            repos,
            db,
            updater_config,
            session_config,
            cycle_id,
            dry_run_reported,
            synced: HashSet::new(),
            service: None,
            dispatched: 0,
            error: None,
        }
    }

    async fn dispatch(&mut self, o: &Observation) {
        if self.error.is_some() {
            return;
        }
        let Some(repo) = self.repos.iter().find(|r| r.name == o.entry.repo_name) else {
            return;
        };
        let dry_run = self.updater_config.dry_run;
        if dry_run
            && !self.dry_run_reported.insert((
                repo.name.clone(),
                o.entry.attr_path.clone(),
                o.latest.clone(),
            ))
        {
            return;
        }
        info!(
            "[{}] {}: new release {} -> {}",
            repo.label(),
            o.entry.attr_path,
            o.current,
            o.latest
        );

        // Base the repo's update worktrees (created from `HEAD`) on the
        // current upstream branch. Its first update of the cycle has not been
        // sent yet, so none of its worktrees exist; other repos' updates may
        // be running, but they use separate checkouts. (Fast-forward only;
        // unmanaged checkouts are never touched.)
        if !dry_run && self.synced.insert(repo.name.clone()) {
            sync::sync_repo(repo, self.db).await;
        }

        let request = UpdateRequest {
            attr_path: o.entry.attr_path.clone(),
            drv_path: o.entry.drv_path.clone(),
            current_version: o.current.clone(),
            new_version: o.latest.clone(),
            repo_name: Some(repo.name.clone()),
            eval_entry_point: Some(repo.eval_entry_point.clone()),
            pr_config: repo.pr_config.clone(),
            fork: Some(repo.fork.clone()),
        };
        let service = match self.service.as_mut() {
            Some(service) => service,
            None => match self.start().await {
                Ok(service) => self.service.insert(service),
                Err(e) => {
                    warn!("Failed to start update session, not dispatching: {:#}", e);
                    self.error = Some(e);
                    return;
                },
            },
        };
        if service.tx.send(request).is_ok() {
            self.dispatched += 1;
        } else {
            self.error = Some(anyhow::anyhow!("updater channel closed"));
        }
    }

    /// Lazy session: only cycles that dispatch work get one.
    async fn start(&self) -> anyhow::Result<RunningService> {
        let mut config = self.session_config.clone();
        if let Some(obj) = config.as_object_mut() {
            obj.insert("cycle_id".to_owned(), serde_json::json!(self.cycle_id));
        }
        let session_id = self
            .db
            .create_session(SessionMode::Watch, Some(config.to_string()))
            .await?;
        let mut cycle_config = self.updater_config.clone();
        cycle_config.session_id = session_id.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        let db = self.db.clone();
        let handle = tokio::spawn(async move { cycle_config.run_service(rx, db).await });
        Ok(RunningService {
            session_id,
            tx,
            handle,
        })
    }

    /// Close the channel, wait for the dispatched updates and finalize the
    /// session.
    async fn finish(
        self,
        stats: &mut WatchCycleStats,
        packages_observed: usize,
        backoff: usize,
    ) -> anyhow::Result<()> {
        stats.dispatched = self.dispatched;
        if let Some(RunningService {
            session_id,
            tx,
            handle,
        }) = self.service
        {
            stats.session_id = Some(session_id.clone());
            drop(tx);
            let outcome = handle.await.unwrap_or_else(|e| {
                warn!("Updater service panicked: {}", e);
                UpdaterStats {
                    failed: self.dispatched,
                    ..UpdaterStats::default()
                }
            });
            stats.updated = outcome.updated + outcome.dry_run;
            stats.failed = outcome.failed;
            let status = if outcome.failed > 0 {
                SessionStatus::Failed
            } else {
                SessionStatus::Completed
            };
            self.db
                .finalize_session(&session_id, status, packages_observed, backoff)
                .await?;
        }
        self.error.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::UpdateStatus;

    const MIN: Duration = Duration::from_secs(60);

    fn ago(now: DateTime<Utc>, minutes: i64) -> DateTime<Utc> {
        now - chrono::Duration::minutes(minutes)
    }

    #[test]
    fn source_due_cases() {
        let now = Utc::now();
        let hour = 60 * MIN;
        let polled = |minutes, health| SourcePollState {
            last_polled_at: Some(ago(now, minutes)),
            health,
        };
        for (state, due) in [
            (None, true),
            (
                Some(SourcePollState {
                    last_polled_at: None,
                    health: SourceHealth::Ok,
                }),
                true,
            ),
            // Restart shortly after a poll: not due.
            (Some(polled(5, SourceHealth::Ok)), false),
            (Some(polled(29, SourceHealth::Ok)), false),
            // Polled late in the previous cycle: still due on this tick.
            (Some(polled(30, SourceHealth::Ok)), true),
            (Some(polled(61, SourceHealth::Error)), true),
            (Some(polled(61, SourceHealth::Moved)), true),
            // Not found: daily.
            (Some(polled(61, SourceHealth::NotFound)), false),
            (Some(polled(23 * 60 + 30, SourceHealth::NotFound)), true),
            // Last poll in the future (clock change).
            (Some(polled(-30, SourceHealth::Ok)), false),
        ] {
            assert_eq!(source_due(state.as_ref(), now, hour), due, "{state:?}");
        }
    }

    #[test]
    fn index_reuse_cases() {
        let now = Utc::now();
        let day = 24 * 60 * MIN;
        let ok = |minutes, config: &str| RepoIndexState {
            status: Some("ok".to_owned()),
            last_indexed_at: Some(ago(now, minutes)),
            indexed_count: 1305,
            config: Some(config.to_owned()),
            indexed_commit: Some("abc".to_owned()),
        };
        let remaining = |state: Option<RepoIndexState>| {
            index_reuse_remaining(state.as_ref(), "cfg", Some("abc"), now, day)
                .map(|d| d.as_secs() / 60)
        };
        assert_eq!(remaining(Some(ok(60, "cfg"))), Some(23 * 60));
        assert_eq!(
            remaining(Some(RepoIndexState {
                indexed_commit: Some("old".to_owned()),
                ..ok(60, "cfg")
            })),
            None,
            "checkout moved since (e.g. a PR was merged)"
        );
        assert_eq!(
            remaining(Some(RepoIndexState {
                indexed_commit: None,
                ..ok(60, "cfg")
            })),
            None,
            "built before the commit was recorded"
        );
        // An unreadable HEAD does not force a rebuild.
        assert_eq!(
            index_reuse_remaining(Some(&ok(60, "cfg")), "cfg", None, now, day)
                .map(|d| d.as_secs() / 60),
            Some(23 * 60)
        );
        assert_eq!(remaining(None), None);
        assert_eq!(remaining(Some(ok(24 * 60, "cfg"))), None, "expired");
        assert_eq!(remaining(Some(ok(60, "other"))), None, "config changed");
        assert_eq!(
            remaining(Some(RepoIndexState {
                config: None,
                ..ok(60, "cfg")
            })),
            None,
            "built before the fingerprint was recorded"
        );
        assert_eq!(
            remaining(Some(RepoIndexState {
                indexed_count: 0,
                ..ok(60, "cfg")
            })),
            None,
            "indexed nothing"
        );
        for status in [None, Some("error"), Some("indexing")] {
            assert_eq!(
                remaining(Some(RepoIndexState {
                    status: status.map(str::to_owned),
                    ..ok(60, "cfg")
                })),
                None,
                "{status:?}"
            );
        }
        assert_eq!(
            remaining(Some(RepoIndexState {
                last_indexed_at: None,
                ..ok(60, "cfg")
            })),
            None
        );
    }

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
                drv_path: None,
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
