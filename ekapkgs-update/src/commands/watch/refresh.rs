//! Keep each repo's upstream index at the commit its checkout is on.
//!
//! An index records the commit it was built from
//! (`watched_repos.indexed_commit`). Every head-check interval, the watcher
//! asks each managed repo's upstream for its base branch (`git ls-remote`, no
//! fetch). When that moved, the checkout is synced; when the checkout's
//! `HEAD` then differs from the indexed commit, the repo is re-indexed.
//! Unmanaged checkouts are re-indexed when their `HEAD` moves. A webhook
//! delivery (push, merged PR) triggers the same check for its repo at once;
//! the interval check then only catches missed deliveries.
//!
//! This is what notices merged PRs: re-indexing registers the merged version
//! as the package's current version, which marks its proposal merged
//! ([`Database::upsert_package_info`]) and makes the next update start from
//! it. A full index costs one `iets eval-jobs` pass (seconds), so there is no
//! attempt to re-evaluate only the changed packages.
//!
//! Sync, index builds, update worktree creation and branch-mode updates all
//! hold the checkout lock ([`crate::git::checkout_lock`]), so this runs
//! alongside poll cycles without moving `HEAD` under a running update.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::index::IndexTarget;
use super::{ResolvedRepo, index, sync};
use crate::database::{Database, IndexOutcome, RepoSyncStatus, WatchCycleKind, WatchCycleStats};

/// What an index build depends on besides the checkout: the running binary
/// (a new build may index differently), the entry points and the select
/// expression.
pub(super) fn index_config(repo: &ResolvedRepo) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    format!(
        "exe={exe}\nenumerate={}\nselect={}\neval={}",
        repo.enumerate_entry_point,
        repo.select.as_deref().unwrap_or(""),
        repo.eval_entry_point
    )
}

/// Build one repo's index at `commit` (its `HEAD`), recording it as an
/// `index` watch cycle and in `watched_repos`. The caller holds the checkout
/// lock, so `HEAD` cannot move during the build.
pub(super) async fn index_repo_locked(
    repo: &ResolvedRepo,
    db: &Database,
    config: String,
    commit: Option<String>,
) {
    let cycle = db
        .start_watch_cycle(WatchCycleKind::Index, Some(&repo.name))
        .await
        .map_err(|e| warn!("[{}] Failed to record index cycle: {:#}", repo.label(), e))
        .ok();
    if let Err(e) = db.record_repo_index_start(&repo.name).await {
        warn!("[{}] Failed to record index start: {:#}", repo.label(), e);
    }

    let started = Instant::now();
    let target = IndexTarget {
        repo_name: &repo.name,
        enumerate_file: &repo.enumerate_entry_point,
        select: repo.select.as_deref(),
        repo_dir: Some(&repo.repo_dir),
    };
    let stats = index::build_upstream_index(target, db, None).await;
    let outcome = IndexOutcome {
        error: stats.stream_error.clone(),
        duration_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
        evaluated: stats.evaluated,
        indexed: stats.indexed,
        skipped: stats.skipped,
        errors: stats.errors,
        config: Some(config),
        commit,
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

/// Check every repo for new commits every `interval`, and the repos named
/// on `triggers` (webhook deliveries) right away, re-indexing the ones that
/// moved. Runs until the task is aborted.
pub(super) async fn watch_heads(
    repos: Arc<Vec<ResolvedRepo>>,
    db: Database,
    interval: Option<Duration>,
    mut triggers: Option<mpsc::UnboundedReceiver<String>>,
) {
    // Upstream heads already synced to, per repo. Starts empty, so the first
    // check syncs every managed checkout once.
    let mut synced: HashMap<String, String> = HashMap::new();
    let mut ticker = interval.map(|interval| {
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker
    });
    loop {
        tokio::select! {
            () = tick(ticker.as_mut()) => {
                for repo in repos.iter() {
                    check_repo(repo, &db, &mut synced).await;
                }
            },
            name = recv(triggers.as_mut()) => {
                let Some(name) = name else {
                    triggers = None;
                    continue;
                };
                // Coalesce a burst (push + pull_request for one merge).
                let mut names = BTreeSet::from([name]);
                if let Some(rx) = triggers.as_mut() {
                    while let Ok(name) = rx.try_recv() {
                        names.insert(name);
                    }
                }
                for repo in repos.iter().filter(|r| names.contains(&r.name)) {
                    debug!("[{}] Checking for new commits (webhook)", repo.label());
                    // Sync even if the upstream head looks unchanged.
                    synced.remove(&repo.name);
                    check_repo(repo, &db, &mut synced).await;
                }
            },
        }
    }
}

async fn tick(ticker: Option<&mut tokio::time::Interval>) {
    match ticker {
        Some(ticker) => {
            ticker.tick().await;
        },
        None => std::future::pending().await,
    }
}

async fn recv(rx: Option<&mut mpsc::UnboundedReceiver<String>>) -> Option<String> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Sync and re-index `repo` if its upstream branch or its `HEAD` moved.
async fn check_repo(repo: &ResolvedRepo, db: &Database, synced: &mut HashMap<String, String>) {
    let remote = sync::remote_head(repo).await.unwrap_or_else(|e| {
        debug!(
            "[{}] Could not read the upstream head: {:#}",
            repo.label(),
            e
        );
        None
    });
    let local = sync::local_head(&repo.repo_dir).await;
    let indexed = indexed_commit(repo, db).await;
    let upstream_moved = remote_moved(remote.as_deref(), synced.get(&repo.name));
    if !upstream_moved && !head_moved(local.as_deref(), indexed.as_deref()) {
        return;
    }

    let lock = crate::git::checkout_lock(&repo.repo_dir);
    let _checkout = lock.lock().await;
    if upstream_moved {
        let status = sync::sync_repo_locked(repo, db).await;
        // A failed fetch is retried at the next check; a diverged checkout
        // is not fetched again until upstream moves.
        if let (Some(remote), true) = (remote, status != RepoSyncStatus::Error) {
            synced.insert(repo.name.clone(), remote);
        }
    }
    // Re-read under the lock: the periodic refresh may have indexed
    // meanwhile.
    let head = sync::local_head(&repo.repo_dir).await;
    let indexed = indexed_commit(repo, db).await;
    if head_moved(head.as_deref(), indexed.as_deref()) {
        info!(
            "[{}] Checkout moved to {} (indexed at {}); re-indexing",
            repo.label(),
            short(head.as_deref()),
            short(indexed.as_deref())
        );
        index_repo_locked(repo, db, index_config(repo), head).await;
    }
}

async fn indexed_commit(repo: &ResolvedRepo, db: &Database) -> Option<String> {
    db.repo_index_state(&repo.name)
        .await
        .map_err(|e| warn!("[{}] Failed to read index state: {:#}", repo.label(), e))
        .ok()
        .flatten()
        .and_then(|state| state.indexed_commit)
}

/// Whether the upstream branch points somewhere the checkout was not synced
/// to yet. An unknown upstream head (unmanaged checkout, `ls-remote` failed)
/// never counts as moved.
fn remote_moved(remote: Option<&str>, synced: Option<&String>) -> bool {
    remote.is_some_and(|r| synced.is_none_or(|s| s != r))
}

/// Whether the checkout is on a commit the index was not built from. An
/// unreadable `HEAD` is left alone.
pub(super) fn head_moved(head: Option<&str>, indexed: Option<&str>) -> bool {
    head.is_some() && head != indexed
}

fn short(sha: Option<&str>) -> &str {
    sha.map_or("nothing", |s| s.get(..12).unwrap_or(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_cases() {
        let synced = "aaa".to_owned();
        // (upstream head, synced to, moved)
        let remote_cases = [
            (None, None, false),
            (None, Some(&synced), false),
            (Some("aaa"), None, true),
            (Some("aaa"), Some(&synced), false),
            (Some("bbb"), Some(&synced), true),
        ];
        for (remote, synced, expected) in remote_cases {
            assert_eq!(
                remote_moved(remote, synced),
                expected,
                "{remote:?} {synced:?}"
            );
        }

        // (HEAD, indexed commit, moved)
        let head_cases = [
            (None, None, false),
            (None, Some("aaa"), false),
            (Some("aaa"), None, true),
            (Some("aaa"), Some("aaa"), false),
            (Some("bbb"), Some("aaa"), true),
        ];
        for (head, indexed, expected) in head_cases {
            assert_eq!(head_moved(head, indexed), expected, "{head:?} {indexed:?}");
        }
    }
}
