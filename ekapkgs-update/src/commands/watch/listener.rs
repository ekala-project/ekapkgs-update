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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};

use super::feeds::{
    ApiGate, ApiSkipped, PolledSource, failure_health, poll_source, release_flagged_prerelease,
    releases_usable, update_version_for_entry, upstream_source_from_index,
};
use super::refresh::{self, index_config, index_repo_locked};
use super::webhook::{self, WebhookState};
use super::{ResolvedRepo, sync};
use crate::commands::run::{
    UpdateJob, UpdateRequest, UpdaterServiceConfig, UpdaterStats, group_jobs,
};
use crate::database::{
    Database, RepoIndexState, SessionMode, SessionStatus, SourceHealth, SourcePoll,
    SourcePollState, UpdateRecord, UpstreamIndexEntry, WatchCycleKind, WatchCycleStats,
    effective_current_version,
};
use crate::vcs_sources::{Release, compare_version_components, find_explicit_release};

/// Concurrent feed / API fetches per poll cycle.
const POLL_CONCURRENCY: usize = 8;
/// Uneventful poll cycles are kept this long.
const CYCLE_RETENTION_DAYS: i64 = 7;
/// Shortest allowed poll / index interval (`interval` panics on zero).
const MIN_INTERVAL: Duration = Duration::from_secs(60);
/// How often a source that answered 404/410 is checked again.
const NOT_FOUND_RECHECK: Duration = Duration::from_secs(24 * 60 * 60);
/// Longest wait between polls of a source that keeps failing.
const MAX_FAILURE_BACKOFF: Duration = Duration::from_secs(24 * 60 * 60);
/// Repos due for re-indexing within this much of each other are re-indexed
/// together.
const INDEX_SLACK: Duration = Duration::from_secs(5 * 60);
/// How long a signal waits for aborted updates to stop.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// `(repo, attr_path, version)` already reported by a dry-run dispatch.
type DryRunReported = HashSet<(String, String, String)>;

/// Set once SIGINT or SIGTERM arrived.
type Stop = watch::Receiver<bool>;

/// Resolves once a shutdown signal arrived (never, if the signal task is gone
/// without one).
async fn stopped(stop: &mut Stop) {
    if stop.wait_for(|stopped| *stopped).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Polls one upstream source and records its observations. The watch loop
/// uses [`source_poller`]; tests inject their own.
type Poller = Arc<dyn Fn(UpstreamIndexEntry) -> PollFuture + Send + Sync>;
type PollFuture = Pin<Box<dyn Future<Output = SourceOutcome> + Send>>;

/// The poller of one cycle: [`poll_one_source`], sharing the cycle's API
/// gate (rate limit state) and observation time.
fn source_poller(db: &Database, repos: &[ResolvedRepo]) -> Poller {
    let db = db.clone();
    let configured: Arc<HashSet<String>> = Arc::new(repos.iter().map(|r| r.name.clone()).collect());
    let gate = Arc::new(ApiGate::default());
    let now = Utc::now();
    Arc::new(move |source_row| {
        let db = db.clone();
        let configured = Arc::clone(&configured);
        let gate = Arc::clone(&gate);
        Box::pin(async move { poll_one_source(&db, &source_row, &configured, &gate, now).await })
    })
}

/// Run the watch event loop until `shutdown` resolves (the caller passes
/// [`crate::commands::run::shutdown_signal`]: SIGINT or SIGTERM).
///
/// A signal during a poll cycle stops dispatching, aborts the cycle's updater
/// service (and the updates in flight) and closes its session as
/// `cancelled`; `recover_interrupted_watch_state` remains the fallback for a
/// killed process.
pub async fn run_watch_loop(
    repos: Arc<Vec<ResolvedRepo>>,
    db: Database,
    updater_config: UpdaterServiceConfig,
    poll_interval: Duration,
    index_refresh_interval: Duration,
    head_check_interval: Option<Duration>,
    webhook: Option<super::WebhookListen>,
    session_config: serde_json::Value,
    shutdown: impl Future<Output = &'static str> + Send + 'static,
) -> anyhow::Result<()> {
    let poll_interval = poll_interval.max(MIN_INTERVAL);
    let index_refresh_interval = index_refresh_interval.max(MIN_INTERVAL);

    let (stop_tx, mut stop) = watch::channel(false);
    tokio::spawn(async move {
        let signal = shutdown.await;
        info!("Received {}, shutting down watch service...", signal);
        let _ = stop_tx.send(true);
    });

    let all: Vec<&ResolvedRepo> = repos.iter().collect();
    let mut next_index: HashMap<String, tokio::time::Instant> = tokio::select! {
        remaining = refresh_repos(&all, &db, Some(index_refresh_interval), index_refresh_interval) => {
            index_deadlines(remaining).collect()
        },
        () = stopped(&mut stop) => return Ok(()),
    };
    let mut sources = load_sources(&db, None).await;

    let head_check_interval = head_check_interval.map(|i| i.max(MIN_INTERVAL));
    if let Some(interval) = head_check_interval {
        info!(
            "Re-indexing repos whose upstream branch moved (checked every {})",
            humantime(interval)
        );
    }
    // Webhook deliveries ask the head watcher to check a repo right away.
    let (webhook_server, triggers) = match webhook {
        Some(webhook) => {
            let (tx, rx) = mpsc::unbounded_channel();
            let state = WebhookState::new(webhook.secret, db.clone(), Arc::clone(&repos), tx);
            (Some(webhook::serve(webhook.listen, state).await?), Some(rx))
        },
        None => (None, None),
    };
    let head_watcher = (head_check_interval.is_some() || triggers.is_some()).then(|| {
        tokio::spawn(refresh::watch_heads(
            Arc::clone(&repos),
            db.clone(),
            head_check_interval,
            triggers,
        ))
    });

    // The first poll tick fires immediately (polling only the sources that
    // are due); on a fresh database that poll records baselines.
    let mut poll_ticker = tokio::time::interval(poll_interval);
    poll_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    // Dry runs leave no trace in the database, so without this every poll
    // would report (and open a session for) the same releases again.
    let mut dry_run_reported = DryRunReported::new();
    let scan_cves = !updater_config.pr_enhancements.skip_cve_check;

    loop {
        // Each repo is re-indexed when its own index expires (a restart may
        // have reused some indexes and rebuilt others).
        let wake = next_index
            .values()
            .min()
            .copied()
            .unwrap_or_else(|| tokio::time::Instant::now() + index_refresh_interval);
        tokio::select! {
            _ = poll_ticker.tick() => {
                // The head watcher may have re-indexed since the last cycle.
                sources = load_sources(&db, Some(sources)).await;
                run_poll_cycle(
                    &sources,
                    poll_interval,
                    source_poller(&db, &repos),
                    &repos,
                    &db,
                    &updater_config,
                    &session_config,
                    &mut dry_run_reported,
                    &stop,
                )
                .await;
                // Known vulnerabilities of packages whose version changed, or
                // whose last scan is older than a day (cheap when none is due).
                if scan_cves && !*stop.borrow() {
                    tokio::select! {
                        () = scan_vulnerabilities(&db) => {},
                        () = stopped(&mut stop) => {},
                    }
                }
            },
            () = tokio::time::sleep_until(wake) => {
                let due: Vec<&ResolvedRepo> = repos
                    .iter()
                    .filter(|r| next_index.get(&r.name).is_none_or(|&at| at <= wake + INDEX_SLACK))
                    .collect();
                info!(
                    "Refreshing {} of {} repositories and their upstream indexes...",
                    due.len(),
                    repos.len()
                );
                tokio::select! {
                    remaining = refresh_repos(&due, &db, None, index_refresh_interval) => {
                        next_index.extend(index_deadlines(remaining));
                    },
                    () = stopped(&mut stop) => {},
                }
            },
            () = stopped(&mut stop) => {},
        }
        if *stop.borrow() {
            break;
        }
    }

    for task in [head_watcher, webhook_server].into_iter().flatten() {
        task.abort();
    }
    info!("Watch service stopped");
    Ok(())
}

/// Refresh known vulnerabilities after a poll cycle (`--skip-cve` disables it).
async fn scan_vulnerabilities(db: &Database) {
    let options = crate::cve::scan::ScanOptions::default();
    if let Err(e) = crate::cve::scan::scan_due(db.pool(), &options).await {
        warn!("CVE scan failed: {:#}", e);
    }
}

/// When each repo's index is due next.
fn index_deadlines(
    remaining: Vec<(String, Duration)>,
) -> impl Iterator<Item = (String, tokio::time::Instant)> {
    let now = tokio::time::Instant::now();
    remaining
        .into_iter()
        .map(move |(name, d)| (name, now + d.max(MIN_INTERVAL)))
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

/// Sync and re-index `repos` (sequentially: each index build already runs
/// iets eval-jobs with many workers).
///
/// With `reuse_within`, a repo whose last index is still reusable
/// ([`index_reuse_remaining`]) is synced but not re-indexed. Returns how long
/// until each repo is due again: the reused index's remaining time, or
/// `interval` for a repo indexed now.
async fn refresh_repos(
    repos: &[&ResolvedRepo],
    db: &Database,
    reuse_within: Option<Duration>,
    interval: Duration,
) -> Vec<(String, Duration)> {
    let mut next = Vec::with_capacity(repos.len());
    for &repo in repos {
        let lock = crate::git::checkout_lock(&repo.repo_dir);
        let _checkout = lock.lock().await;
        sync::sync_repo_locked(repo, db).await;
        let head = sync::local_head(&repo.repo_dir).await;
        let config = index_config(repo);
        if let Some(reuse) = reuse_within {
            let state = db
                .repo_index_state(&repo.name)
                .await
                .map_err(|e| warn!("[{}] Failed to read index state: {:#}", repo.label(), e))
                .ok()
                .flatten();
            if let Some(remaining) =
                index_reuse_remaining(state.as_ref(), &config, head.as_deref(), Utc::now(), reuse)
            {
                info!(
                    "[{}] Reusing the upstream index from {} ago (re-index in {})",
                    repo.label(),
                    humantime(reuse.saturating_sub(remaining)),
                    humantime(remaining)
                );
                next.push((repo.name.clone(), remaining));
                continue;
            }
        }
        index_repo_locked(repo, db, config, head).await;
        next.push((repo.name.clone(), interval));
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
/// the previous cycle is not skipped until the one after.
///
/// Sources that were not found are rechecked every [`NOT_FOUND_RECHECK`].
/// Sources whose polls keep failing back off: the wait doubles with each
/// consecutive failure after the first, up to [`MAX_FAILURE_BACKOFF`].
fn source_due(
    state: Option<&SourcePollState>,
    now: DateTime<Utc>,
    poll_interval: Duration,
) -> bool {
    let Some(last) = state.and_then(|s| s.last_polled_at) else {
        return true;
    };
    let interval = match state {
        Some(s) if s.health == SourceHealth::NotFound => NOT_FOUND_RECHECK.max(poll_interval),
        Some(s) if s.health == SourceHealth::Error => {
            failure_backoff(poll_interval, s.consecutive_failures)
        },
        _ => poll_interval,
    };
    // A last poll in the future (clock change) counts as just polled.
    let elapsed = (now - last).to_std().unwrap_or_default();
    elapsed + poll_interval / 2 >= interval
}

/// `poll_interval * 2^(failures - 1)`, capped at [`MAX_FAILURE_BACKOFF`]
/// (never below `poll_interval`).
fn failure_backoff(poll_interval: Duration, consecutive_failures: i64) -> Duration {
    let doublings = u32::try_from(consecutive_failures.saturating_sub(1).clamp(0, 16)).unwrap_or(0);
    poll_interval
        .saturating_mul(1 << doublings)
        .min(MAX_FAILURE_BACKOFF)
        .max(poll_interval)
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

/// Collapse aliases: among packages in the same repo with the same `pname`,
/// current and target version (one source), only the shortest attr path is
/// dispatched, and none if any of them was already proposed or is backing
/// off for that version. Separate packages built from one source (a
/// monorepo's `foo` and `foo-cli`, both at its version) have different
/// pnames and are updated separately, like `run`'s (pname, version) dedup.
fn resolve_siblings(observations: &mut [Observation]) {
    type Key = (String, Option<String>, String, String);
    let mut groups: HashMap<Key, Vec<usize>> = HashMap::new();
    for (i, o) in observations.iter().enumerate() {
        if matches!(
            o.decision,
            Decision::Dispatch | Decision::AlreadyProposed | Decision::Backoff
        ) {
            groups
                .entry((
                    o.entry.repo_name.clone(),
                    o.entry.pname.clone(),
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
    /// Not polled: it needed the API, which is rate limited this cycle.
    skipped: bool,
    new_release: bool,
    observations: Vec<Observation>,
}

/// Why packages whose source polled fine but listed nothing are not observed.
const NO_RELEASES: &str = "upstream lists no releases or tags";
/// Why packages of a source that cannot be reconstructed are not observed.
const NOT_POLLABLE: &str = "cannot poll this upstream source";

/// Record every configured package tracking a source as `upstream_error`
/// when the source could not be observed (poll failed, no releases, or not
/// pollable). Without this a newly indexed package stays `pending` forever.
async fn record_unobserved(db: &Database, entries: &[UpstreamIndexEntry], error: &str) {
    for entry in entries {
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

/// Record a failed poll of a source (its health and failure count, which
/// drive [`source_due`]'s backoff) and the packages tracking it.
async fn record_poll_failure(
    db: &Database,
    source_row: &UpstreamIndexEntry,
    entries: &[UpstreamIndexEntry],
    message: &str,
    health: SourceHealth,
) {
    let (kind, key, instance) = (
        source_row.upstream_type.as_str(),
        source_row.upstream_key.as_str(),
        source_row.instance.as_deref(),
    );
    if let Err(e) = db
        .record_source_poll(
            kind,
            key,
            instance,
            &SourcePoll::Failure {
                error: message,
                health,
            },
        )
        .await
    {
        warn!("{}/{}: failed to record poll failure: {:#}", kind, key, e);
    }
    record_unobserved(db, entries, message).await;
}

/// Poll one source and record observations for every package tracking it.
async fn poll_one_source(
    db: &Database,
    source_row: &UpstreamIndexEntry,
    configured: &HashSet<String>,
    gate: &ApiGate,
    now: DateTime<Utc>,
) -> SourceOutcome {
    let (kind, key, instance) = (
        source_row.upstream_type.as_str(),
        source_row.upstream_key.as_str(),
        source_row.instance.as_deref(),
    );
    let mut outcome = SourceOutcome::default();

    let entries: Vec<UpstreamIndexEntry> =
        match db.get_attr_paths_for_source(kind, key, instance).await {
            Ok(entries) => entries
                .into_iter()
                .filter(|e| configured.contains(&e.repo_name))
                .collect(),
            Err(e) => {
                warn!("{}/{}: failed to load packages: {:#}", kind, key, e);
                return outcome;
            },
        };

    let Some(source) = upstream_source_from_index(kind, key, instance) else {
        debug!("{}/{}: cannot reconstruct upstream source", kind, key);
        // Recorded as a failed poll so the source backs off like any other
        // (instead of being due, and opening a cycle, on every tick).
        outcome.failed = true;
        record_poll_failure(db, source_row, &entries, NOT_POLLABLE, SourceHealth::Error).await;
        return outcome;
    };

    let polled = poll_source(&source, |r| releases_usable(&entries, r), gate).await;
    let PolledSource {
        mut releases,
        moved_to,
        from_feed,
    } = match polled {
        Ok(polled) => polled,
        Err(e) if e.downcast_ref::<ApiSkipped>().is_some() => {
            debug!("{}/{}: {}", kind, key, e);
            outcome.skipped = true;
            return outcome;
        },
        Err(e) => {
            let message = format!("{e:#}");
            debug!("{}/{}: poll failed: {}", kind, key, message);
            outcome.failed = true;
            record_poll_failure(db, source_row, &entries, &message, failure_health(&e)).await;
            return outcome;
        },
    };

    if !releases.is_empty() {
        // Releases a feed listed that the platform flags as prereleases
        // (checked per dispatch candidate only), by tag.
        let mut checked: HashMap<String, bool> = HashMap::new();
        for entry in &entries {
            if let Some(observation) = observe(
                db,
                &source,
                entry,
                &mut releases,
                from_feed,
                &mut checked,
                gate,
                now,
            )
            .await
            {
                outcome.observations.push(observation);
            }
        }
        resolve_siblings(&mut outcome.observations);
    }

    // After the prerelease checks, so a flagged release is not recorded as
    // the source's latest.
    let newest = newest_stable(&releases);
    match db
        .record_source_poll(
            kind,
            key,
            instance,
            &SourcePoll::Success {
                latest_version: newest.map(Release::version),
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
        record_unobserved(db, &entries, NO_RELEASES).await;
    }
    outcome
}

/// Observe one package: pick its target, decide, record the observation.
///
/// The current version is the newer of the indexed one and the one
/// `updates` records ([`effective_current_version`]): a merged update is not
/// dispatched again before the next re-index.
///
/// Feeds carry no prerelease flags. When the decision is to dispatch a
/// release from a feed and the package excludes prereleases, the platform is
/// asked once whether that release is a prerelease (GitHub) or yanked (PyPI);
/// if so it is marked as one in `releases` and the target is picked again.
/// Only dispatch candidates are checked, so polling stays one feed request
/// per source; when the check fails, the release is dispatched.
async fn observe(
    db: &Database,
    source: &crate::vcs_sources::UpstreamSource,
    entry: &UpstreamIndexEntry,
    releases: &mut [Release],
    from_feed: bool,
    checked: &mut HashMap<String, bool>,
    gate: &ApiGate,
    now: DateTime<Utc>,
) -> Option<Observation> {
    let indexed = entry.current_version.as_deref().filter(|v| !v.is_empty())?;
    let prev = match db
        .get_update_record(&entry.repo_name, &entry.attr_path)
        .await
    {
        Ok(prev) => prev,
        Err(e) => {
            warn!("{}: failed to read update record: {:#}", entry.attr_path, e);
            return None;
        },
    };
    let current = effective_current_version(
        indexed,
        prev.as_ref().and_then(|r| r.current_version.as_deref()),
    )
    .to_owned();
    let mut entry = entry.clone();
    entry.current_version = Some(current.clone());

    let (latest, decision) = loop {
        let latest = best_for_package(releases, &entry, &current);
        let decision = dispatch_decision(&current, &latest, prev.as_ref(), now);
        if decision != Decision::Dispatch || !from_feed || entry.include_prereleases {
            break (latest, decision);
        }
        let Some(tag) =
            find_explicit_release(releases.to_vec(), &latest, entry.version_regex.as_deref())
                .map(|r| r.tag_name)
        else {
            break (latest, decision);
        };
        let flagged = match checked.get(&tag) {
            Some(&flagged) => flagged,
            None if gate.is_rate_limited() => false,
            None => {
                let release = Release {
                    tag_name: tag.clone(),
                    is_prerelease: false,
                };
                let flagged = match release_flagged_prerelease(source, &release).await {
                    Ok(flagged) => flagged,
                    Err(e) => {
                        gate.note_error(&e);
                        debug!(
                            "{}: prerelease check of {} failed: {:#}",
                            entry.attr_path, tag, e
                        );
                        false
                    },
                };
                checked.insert(tag.clone(), flagged);
                flagged
            },
        };
        if !flagged {
            break (latest, decision);
        }
        info!(
            "{}: {} is flagged as a prerelease upstream; not dispatching it",
            entry.attr_path, tag
        );
        for r in releases.iter_mut().filter(|r| r.tag_name == tag) {
            r.is_prerelease = true;
        }
    };

    if let Err(e) = db
        .record_observation(&entry.repo_name, &entry.attr_path, &current, &latest)
        .await
    {
        warn!("{}: failed to record observation: {:#}", entry.attr_path, e);
        return None;
    }
    Some(Observation {
        entry,
        current,
        latest,
        decision,
    })
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
    poller: Poller,
    repos: &[ResolvedRepo],
    db: &Database,
    updater_config: &UpdaterServiceConfig,
    session_config: &serde_json::Value,
    dry_run_reported: &mut DryRunReported,
    stop: &Stop,
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
        poller,
        repos,
        db,
        updater_config,
        session_config,
        dry_run_reported,
        cycle_id,
        &mut stats,
        stop.clone(),
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
    poller: Poller,
    repos: &[ResolvedRepo],
    db: &Database,
    updater_config: &UpdaterServiceConfig,
    session_config: &serde_json::Value,
    dry_run_reported: &mut DryRunReported,
    cycle_id: Option<i64>,
    stats: &mut WatchCycleStats,
    mut stop: Stop,
) -> anyhow::Result<()> {
    info!(
        "Polling {} of {} upstream sources for new releases ({} polled recently)...",
        due.len(),
        total,
        total - due.len()
    );
    let semaphore = Arc::new(Semaphore::new(POLL_CONCURRENCY));

    let mut tasks = JoinSet::new();
    for source_row in due {
        let poll = poller(source_row.clone());
        let semaphore = Arc::clone(&semaphore);
        tasks.spawn(async move {
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return SourceOutcome::default();
            };
            poll.await
        });
    }

    // Updates start as soon as a polled source yields one, so building
    // overlaps with the rest of the sweep instead of waiting for it. Members
    // of update groups are held until the sweep is done and then dispatched
    // per group (with the group's other outdated members).
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
    let mut skipped = 0;
    loop {
        // A signal wins over polls that finished at the same time: nothing
        // is dispatched after it.
        let joined = tokio::select! {
            biased;
            () = stopped(&mut stop) => {
                tasks.abort_all();
                dispatcher.cancel(stats).await;
                anyhow::bail!("interrupted by a shutdown signal");
            },
            joined = tasks.join_next() => match joined {
                Some(joined) => joined,
                None => break,
            },
        };
        let outcome = match joined {
            Ok(outcome) => outcome,
            Err(e) => {
                warn!("Source poll task panicked: {}", e);
                stats.sources_polled += 1;
                stats.sources_failed += 1;
                continue;
            },
        };
        if outcome.skipped {
            skipped += 1;
            continue;
        }
        stats.sources_polled += 1;
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

    if skipped > 0 {
        info!(
            "{} sources not polled (API rate limited); they stay due",
            skipped
        );
    }
    stats.packages_observed = packages_observed;
    for repo in &observed_repos {
        if let Err(e) = db.mark_repo_bootstrapped(repo).await {
            debug!("[{}] Failed to mark bootstrapped: {:#}", repo, e);
        }
    }

    dispatcher
        .finish(stats, packages_observed, backoff, &mut stop)
        .await
}

/// Feeds a poll cycle's dispatched updates to an updater service that is
/// started (with its session) on the first dispatch. Requests for members of
/// update groups are held and sent as group jobs by [`Dispatcher::finish`].
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
    /// Group roots (attr → root) per repo, loaded on the repo's first dispatch.
    roots: HashMap<String, BTreeMap<String, String>>,
    /// Requests for group members, per repo, dispatched after the sweep.
    held: BTreeMap<String, Vec<UpdateRequest>>,
    /// Set when the session could not be created; nothing more is dispatched.
    error: Option<anyhow::Error>,
}

struct RunningService {
    session_id: String,
    tx: mpsc::UnboundedSender<UpdateJob>,
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
            roots: HashMap::new(),
            held: BTreeMap::new(),
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

        if !self.roots.contains_key(&repo.name) {
            let roots = self.db.group_roots(&repo.name).await.unwrap_or_else(|e| {
                warn!("[{}] Failed to load update groups: {:#}", repo.label(), e);
                BTreeMap::new()
            });
            self.roots.insert(repo.name.clone(), roots);
        }
        if self.roots[&repo.name].contains_key(&request.attr_path) {
            self.held
                .entry(repo.name.clone())
                .or_default()
                .push(request);
            return;
        }
        self.send(UpdateJob::Single(request)).await;
    }

    /// Send a job, starting the updater service (and session) first if needed.
    async fn send(&mut self, job: UpdateJob) {
        if self.error.is_some() {
            return;
        }
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
        let attempts = job.attempts();
        if service.tx.send(job).is_ok() {
            self.dispatched += attempts;
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
        mut self,
        stats: &mut WatchCycleStats,
        packages_observed: usize,
        backoff: usize,
        stop: &mut Stop,
    ) -> anyhow::Result<()> {
        for (repo, requests) in std::mem::take(&mut self.held) {
            let roots = self.roots.remove(&repo).unwrap_or_default();
            for job in group_jobs(self.db, &repo, requests, &roots).await {
                self.send(job).await;
            }
        }
        stats.dispatched = self.dispatched;
        if let Some(RunningService {
            session_id,
            tx,
            mut handle,
        }) = self.service.take()
        {
            stats.session_id = Some(session_id.clone());
            drop(tx);
            let joined = tokio::select! {
                biased;
                () = stopped(stop) => {
                    stop_service(self.db, &session_id, handle).await;
                    anyhow::bail!("interrupted by a shutdown signal (session {session_id} cancelled)");
                },
                joined = &mut handle => joined,
            };
            let outcome = joined.unwrap_or_else(|e| {
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

    /// Stop on a shutdown signal: nothing more is dispatched, the updater
    /// service (and its updates in flight) is aborted and its session closed
    /// as `cancelled`. The cycle keeps what was dispatched and the session.
    async fn cancel(self, stats: &mut WatchCycleStats) {
        stats.dispatched = self.dispatched;
        if let Some(RunningService {
            session_id,
            tx,
            handle,
        }) = self.service
        {
            stats.session_id = Some(session_id.clone());
            drop(tx);
            stop_service(self.db, &session_id, handle).await;
        }
    }
}

/// Abort an updater service, wait (briefly) for its updates to stop and close
/// its session as `cancelled`. Aborting drops the service's `JoinSet`, which
/// aborts every update task.
async fn stop_service(
    db: &Database,
    session_id: &str,
    handle: tokio::task::JoinHandle<UpdaterStats>,
) {
    handle.abort();
    if tokio::time::timeout(SHUTDOWN_GRACE, handle).await.is_err() {
        warn!("Some update tasks did not stop within {:?}", SHUTDOWN_GRACE);
    }
    if let Err(e) = db.close_session(session_id, SessionStatus::Cancelled).await {
        warn!(
            "Failed to close interrupted session {}: {:#}",
            session_id, e
        );
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
            consecutive_failures: i64::from(health == SourceHealth::Error),
        };
        let failing = |minutes, failures| SourcePollState {
            consecutive_failures: failures,
            ..polled(minutes, SourceHealth::Error)
        };
        for (state, due) in [
            (None, true),
            (
                Some(SourcePollState {
                    last_polled_at: None,
                    health: SourceHealth::Ok,
                    consecutive_failures: 0,
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
            // Failing sources back off: 1h, 2h, 4h, ... up to a day.
            (Some(failing(61, 2)), false),
            (Some(failing(2 * 60, 2)), true),
            (Some(failing(3 * 60, 3)), false),
            (Some(failing(4 * 60, 3)), true),
            (Some(failing(23 * 60, 40)), false),
            (Some(failing(24 * 60, 40)), true),
        ] {
            assert_eq!(source_due(state.as_ref(), now, hour), due, "{state:?}");
        }
    }

    #[test]
    fn failure_backoff_cases() {
        let hour = 60 * MIN;
        for (failures, poll, expected) in [
            (0, hour, hour),
            (1, hour, hour),
            (2, hour, 2 * hour),
            (5, hour, 16 * hour),
            (6, hour, 24 * hour),
            (i64::MAX, hour, 24 * hour),
            // Never shorter than the poll interval.
            (3, 48 * hour, 48 * hour),
        ] {
            assert_eq!(failure_backoff(poll, failures), expected, "{failures}");
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
        let pname = attr.rsplit('.').next().map(|p| {
            p.trim_end_matches("-alias")
                .trim_end_matches("_0_9")
                .to_owned()
        });
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
                pname,
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
            // One source, same versions, but another package (a monorepo's
            // second crate): independent.
            obs("", "foo-cli", "1.0", "1.1", Dispatch),
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
                ("", "foo-cli", Dispatch),
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

    // ── Shutdown during a poll cycle ──────────────────────────────

    async fn setup_db() -> Database {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        Database::from(pool)
    }

    fn test_repo() -> ResolvedRepo {
        ResolvedRepo {
            name: "r".into(),
            enumerate_entry_point: "/nonexistent/default.nix".into(),
            select: None,
            eval_entry_point: "/nonexistent/default.nix".into(),
            repo_dir: std::path::PathBuf::from("/nonexistent"),
            pr_config: None,
            fork: "origin".into(),
            upstream_url: None,
            managed: false,
        }
    }

    /// Dry run: dispatched updates are reported without nix or git.
    fn dry_run_config() -> UpdaterServiceConfig {
        UpdaterServiceConfig {
            session_id: String::new(),
            eval_entry_point: Arc::from("/nonexistent/default.nix"),
            pr_config: None,
            fork: Arc::from("origin"),
            run_passthru_tests: false,
            dry_run: true,
            concurrency: 1,
            pr_enhancements: crate::commands::pr_enhancements::PrEnhancementsConfig::default(),
            interactive: false,
            preserve_failures: false,
            commit_strategy: crate::cli::CommitStrategy::Worktrees,
            claude_fix: false,
            claude_fix_max_turns: 0,
            claude_fix_timeout: 0,
            executor: None,
        }
    }

    /// One source per attr (`o/<attr>`), never polled before (all due).
    fn sources(attrs: &[&str]) -> Vec<UpstreamIndexEntry> {
        attrs
            .iter()
            .map(|attr| UpstreamIndexEntry {
                upstream_key: format!("o/{attr}"),
                ..obs("r", attr, "1.0", "1.1", Decision::Dispatch).entry
            })
            .collect()
    }

    /// A poller whose sources all yield a new release to dispatch; `late`
    /// only answers once the shutdown signal arrived.
    fn dispatching_poller(stop: &Stop, late: &'static str) -> Poller {
        let stop = stop.clone();
        Arc::new(move |row: UpstreamIndexEntry| {
            let mut stop = stop.clone();
            Box::pin(async move {
                if row.attr_path == late {
                    stopped(&mut stop).await;
                }
                SourceOutcome {
                    observations: vec![Observation {
                        entry: row,
                        current: "1.0".into(),
                        latest: "1.1".into(),
                        decision: Decision::Dispatch,
                    }],
                    ..SourceOutcome::default()
                }
            })
        })
    }

    #[derive(Debug, sqlx::FromRow)]
    struct CycleRow {
        status: String,
        error: Option<String>,
        dispatched: i64,
        session_id: Option<String>,
    }

    async fn only_cycle(db: &Database) -> CycleRow {
        sqlx::query_as("SELECT status, error, dispatched, session_id FROM watch_cycles")
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    /// Sessions' `(packages_attempted, packages_skipped)`.
    async fn session_counts(db: &Database) -> Vec<(i64, i64)> {
        sqlx::query_as("SELECT packages_attempted, packages_skipped FROM update_sessions")
            .fetch_all(db.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn shutdown_during_poll_cancels_session_and_stops_dispatch() {
        let db = setup_db().await;
        let repos = vec![test_repo()];
        let config = dry_run_config();
        let (stop_tx, stop) = watch::channel(false);
        let mut reported = DryRunReported::new();
        let due = sources(&["early", "late"]);
        let session_config = serde_json::json!({});

        let cycle = run_poll_cycle(
            &due,
            MIN,
            dispatching_poller(&stop, "late"),
            &repos,
            &db,
            &config,
            &session_config,
            &mut reported,
            &stop,
        );
        // Signal once the first update has been dispatched and reported.
        let signal = async {
            let wait = async {
                while session_counts(&db).await != [(1, 1)] {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            };
            tokio::time::timeout(Duration::from_secs(10), wait)
                .await
                .expect("first update dispatched");
            stop_tx.send(true).unwrap();
        };
        tokio::join!(cycle, signal);

        // The source answering after the signal was not dispatched.
        assert_eq!(
            reported,
            DryRunReported::from([("r".into(), "early".into(), "1.1".into())])
        );
        let row = only_cycle(&db).await;
        assert_eq!(row.status, "failed");
        assert!(
            row.error
                .as_deref()
                .is_some_and(|e| e.contains("interrupted by a shutdown signal")),
            "{row:?}"
        );
        assert_eq!(row.dispatched, 1);
        let session_id = row.session_id.expect("cycle links its session");
        let session = db.get_session(&session_id).await.unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Cancelled);
        assert!(session.completed_at.is_some());
        // Counters of the work done before the signal are kept.
        assert_eq!(session_counts(&db).await, [(1, 1)]);
    }

    #[tokio::test]
    async fn shutdown_before_any_dispatch_opens_no_session() {
        let db = setup_db().await;
        let repos = vec![test_repo()];
        let config = dry_run_config();
        let (stop_tx, stop) = watch::channel(false);
        let mut reported = DryRunReported::new();
        stop_tx.send(true).unwrap();

        run_poll_cycle(
            &sources(&["a", "b"]),
            MIN,
            dispatching_poller(&stop, ""),
            &repos,
            &db,
            &config,
            &serde_json::json!({}),
            &mut reported,
            &stop,
        )
        .await;

        assert!(reported.is_empty());
        assert!(session_counts(&db).await.is_empty());
        let row = only_cycle(&db).await;
        assert_eq!(row.status, "failed");
        assert_eq!((row.dispatched, row.session_id), (0, None));
    }

    #[tokio::test]
    async fn uninterrupted_poll_completes_session() {
        let db = setup_db().await;
        let repos = vec![test_repo()];
        let config = dry_run_config();
        let (_stop_tx, stop) = watch::channel(false);
        let mut reported = DryRunReported::new();

        run_poll_cycle(
            &sources(&["a", "b"]),
            MIN,
            dispatching_poller(&stop, ""),
            &repos,
            &db,
            &config,
            &serde_json::json!({}),
            &mut reported,
            &stop,
        )
        .await;

        assert_eq!(reported.len(), 2);
        let row = only_cycle(&db).await;
        assert_eq!((row.status.as_str(), row.error), ("completed", None));
        assert_eq!(row.dispatched, 2);
        let session = db
            .get_session(&row.session_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(session.status, SessionStatus::Completed);
        assert_eq!(session_counts(&db).await, [(2, 2)]);
    }

    /// A shutdown signal while the cycle waits for its updates (group jobs,
    /// sent once the sweep is done) aborts the update in flight and the
    /// group queued behind it, cancels the session and interrupts the cycle.
    #[tokio::test]
    async fn shutdown_while_waiting_for_updates_aborts_them() {
        use crate::commands::run::JobExecutor;
        use crate::database::PackageInfo;
        use tokio::sync::oneshot;

        let db = setup_db().await;
        // Groups `g` (g1, g2) and `h` (h1), every member outdated.
        for attr in ["g1", "g2", "h1"] {
            db.upsert_package_info("r", attr, "1.0", &PackageInfo::default())
                .await
                .unwrap();
            db.record_update_available("r", attr, "1.0", "1.1")
                .await
                .unwrap();
        }
        let parents: BTreeMap<String, String> = [("g1", "g"), ("g2", "g"), ("h1", "h")]
            .into_iter()
            .map(|(c, p)| (c.to_owned(), p.to_owned()))
            .collect();
        db.replace_groups("r", &parents, &crate::groups::resolve_roots(&parents))
            .await
            .unwrap();

        // Each job reports that it started and waits for its gate; past it,
        // it would record its members as updated.
        let (started_tx, mut started) = mpsc::unbounded_channel();
        let executor_db = db.clone();
        let mut config = dry_run_config();
        config.dry_run = false;
        config.executor = Some(JobExecutor(Arc::new(move |job: UpdateJob| {
            let started_tx = started_tx.clone();
            let db = executor_db.clone();
            Box::pin(async move {
                let (gate_tx, gate) = oneshot::channel::<()>();
                started_tx.send((job.label(), gate_tx)).unwrap();
                if gate.await.is_ok() {
                    for req in job.requests() {
                        db.record_successful_update(
                            "r",
                            &req.attr_path,
                            &req.current_version,
                            &req.new_version,
                            None,
                            None,
                        )
                        .await
                        .unwrap();
                    }
                }
                Vec::new()
            })
        })));

        let repos = vec![test_repo()];
        let (stop_tx, stop) = watch::channel(false);
        let mut reported = DryRunReported::new();
        let session_config = serde_json::json!({});
        let due = sources(&["g1", "g2", "h1"]);
        let cycle = run_poll_cycle(
            &due,
            MIN,
            dispatching_poller(&stop, ""),
            &repos,
            &db,
            &config,
            &session_config,
            &mut reported,
            &stop,
        );
        let signal = async {
            let (label, gate) = tokio::time::timeout(Duration::from_secs(10), started.recv())
                .await
                .expect("a group job started")
                .expect("executor alive");
            stop_tx.send(true).unwrap();
            (label, gate)
        };
        let ((), (label, gate)) = tokio::time::timeout(Duration::from_secs(30), async {
            tokio::join!(cycle, signal)
        })
        .await
        .expect("the cycle stops on the signal");

        // Only the first group started (one update at a time); it was
        // aborted, so its gate is gone, and the second never started.
        assert_eq!(label, "group g");
        assert!(gate.send(()).is_err(), "the update in flight was aborted");
        assert!(started.try_recv().is_err(), "no other job started");

        let row = only_cycle(&db).await;
        assert_eq!(row.status, "failed");
        let session_id = row.session_id.expect("cycle links its session");
        assert_eq!(
            row.error.as_deref(),
            Some(
                format!("interrupted by a shutdown signal (session {session_id} cancelled)")
                    .as_str()
            ),
        );
        assert_eq!(row.dispatched, 3);
        let session = db.get_session(&session_id).await.unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Cancelled);
        assert_eq!(session_counts(&db).await, [(2, 0)]);
        for attr in ["g1", "g2", "h1"] {
            let record = db.get_update_record("r", attr).await.unwrap().unwrap();
            assert_eq!(record.status, UpdateStatus::Outdated, "{attr}");
            assert_eq!(record.proposed_version, None, "{attr}");
        }
    }
}
