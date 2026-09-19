//! Main event loop for the watch command.
//!
//! Periodically polls upstream feeds/APIs, detects new releases, and dispatches
//! [`UpdateRequest`]s to the existing updater service.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore, mpsc};
use tracing::{debug, info, warn};

use super::feeds::{poll_source, upstream_source_from_index};
use super::index;
use crate::commands::run::{UpdateRequest, UpdaterServiceConfig};
use crate::database::{Database, UpstreamIndexEntry};
use crate::vcs_sources::{UpstreamSource, compare_version_components, extract_version_from_tag};

/// Run the watch event loop.
///
/// 1. Builds the upstream index on startup.
/// 2. Polls all indexed sources at `poll_interval`.
/// 3. Rebuilds the index at `index_refresh_interval`.
/// 4. Sends `UpdateRequest`s to the updater service when new versions are found.
pub async fn run_watch_loop(
    file: &str,
    db: Database,
    updater_config: UpdaterServiceConfig,
    poll_interval: Duration,
    index_refresh_interval: Duration,
    dry_run: bool,
) -> anyhow::Result<()> {
    // Build initial upstream index
    let stats = index::build_upstream_index(file, &db, None).await?;
    info!(
        "Initial index: {} packages indexed, {} skipped, {} errors",
        stats.indexed, stats.skipped, stats.errors
    );

    // Main loop: poll → dispatch → sleep → repeat
    let mut poll_ticker = tokio::time::interval(poll_interval);
    let mut index_ticker = tokio::time::interval(index_refresh_interval);

    // Skip the first immediate tick for both intervals (the index was just built,
    // and the first poll happens right after).
    poll_ticker.tick().await;
    index_ticker.tick().await;

    // Load initial source list; refreshed after each re-index.
    let mut sources = db.get_all_upstream_sources().await?;
    info!("Watching {} unique upstream sources", sources.len());

    loop {
        tokio::select! {
            _ = poll_ticker.tick() => {
                info!("Polling {} upstream sources for new releases...", sources.len());

                let (tx, rx) = mpsc::unbounded_channel();

                // Spawn the updater service for this poll cycle
                let session_id = db.create_session(
                    Some(r#"{"mode": "watch"}"#.to_owned()),
                ).await.unwrap_or_else(|e| {
                    warn!("Failed to create session: {}", e);
                    uuid::Uuid::new_v4().to_string()
                });

                let mut cycle_config = updater_config.clone();
                cycle_config.session_id = session_id.clone();

                let db_updater = db.clone();
                let updater_handle = tokio::spawn(async move {
                    cycle_config.run_service(rx, db_updater).await
                });

                // Poll all sources and send updates
                let dispatched = poll_and_dispatch(
                    &sources,
                    &db,
                    &tx,
                    file,
                    dry_run,
                ).await;

                // Close the sender to signal the updater that no more requests
                // are coming for this cycle.
                drop(tx);

                // Wait for all updates to complete
                match updater_handle.await {
                    Ok((updated, failed)) => {
                        if dispatched > 0 || updated > 0 {
                            info!(
                                "Poll cycle complete: {} dispatched, {} updated, {} failed",
                                dispatched, updated, failed
                            );
                        } else {
                            debug!("Poll cycle complete: no new releases detected");
                        }

                        // Finalize session
                        if let Err(e) = db.finalize_session(
                            &session_id,
                            if failed > 0 {
                                crate::database::SessionStatus::Failed
                            } else {
                                crate::database::SessionStatus::Completed
                            },
                            updated + failed,
                            updated,
                            failed,
                            0,
                        ).await {
                            warn!("Failed to finalize session: {}", e);
                        }
                    },
                    Err(e) => {
                        warn!("Updater task panicked: {}", e);
                    },
                }
            },
            _ = index_ticker.tick() => {
                info!("Refreshing upstream index...");
                match index::build_upstream_index(file, &db, None).await {
                    Ok(stats) => {
                        info!(
                            "Index refreshed: {} indexed, {} skipped, {} errors",
                            stats.indexed, stats.skipped, stats.errors
                        );
                        // Reload source list so new packages are polled
                        match db.get_all_upstream_sources().await {
                            Ok(new_sources) => {
                                info!("Now watching {} upstream sources", new_sources.len());
                                sources = new_sources;
                            },
                            Err(e) => {
                                warn!("Failed to reload sources after re-index: {}", e);
                            },
                        }
                    },
                    Err(e) => {
                        warn!("Index refresh failed: {}", e);
                    },
                }
            },
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down watch service...");
                break;
            },
        }
    }

    Ok(())
}

/// Poll all indexed upstream sources and dispatch [`UpdateRequest`]s for new versions.
///
/// Returns the number of updates dispatched.
async fn poll_and_dispatch(
    sources: &[UpstreamIndexEntry],
    db: &Database,
    tx: &mpsc::UnboundedSender<UpdateRequest>,
    eval_entry_point: &str,
    dry_run: bool,
) -> usize {
    // Rate-limit concurrent feed fetches to avoid hammering APIs
    let semaphore = Arc::new(Semaphore::new(8));
    let dispatched = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // Deduplicate by (pname-approximation, version) to avoid queueing the same
    // update from multiple attr_paths that alias the same package.
    let queued: Arc<Mutex<HashSet<(String, String)>>> = Arc::new(Mutex::new(HashSet::new()));

    let mut handles = Vec::new();

    for entry in sources {
        let source = match upstream_source_from_index(
            &entry.upstream_type,
            &entry.upstream_key,
            entry.instance.as_deref(),
        ) {
            Some(s) => s,
            None => continue,
        };

        let db_clone = db.clone();
        let tx_clone = tx.clone();
        let sem_clone = Arc::clone(&semaphore);
        let dispatched_clone = Arc::clone(&dispatched);
        let queued_clone = Arc::clone(&queued);
        let upstream_type = entry.upstream_type.clone();
        let upstream_key = entry.upstream_key.clone();
        let instance = entry.instance.clone();
        let eval_entry = eval_entry_point.to_owned();

        let handle = tokio::spawn(async move {
            let _permit = sem_clone.acquire().await.expect("semaphore closed");

            if let Err(e) = poll_single_source(
                &source,
                &upstream_type,
                &upstream_key,
                instance.as_deref(),
                &db_clone,
                &tx_clone,
                &eval_entry,
                dry_run,
                &dispatched_clone,
                &queued_clone,
            )
            .await
            {
                debug!("{}/{}: poll failed: {}", upstream_type, upstream_key, e);
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.await.ok();
    }

    dispatched.load(std::sync::atomic::Ordering::Relaxed)
}

/// Poll a single upstream source, compare against indexed versions, and dispatch
/// updates for any attr_paths that have a newer version available.
#[allow(clippy::too_many_arguments)]
async fn poll_single_source(
    source: &UpstreamSource,
    upstream_type: &str,
    upstream_key: &str,
    instance: Option<&str>,
    db: &Database,
    tx: &mpsc::UnboundedSender<UpdateRequest>,
    _eval_entry_point: &str,
    dry_run: bool,
    dispatched: &std::sync::atomic::AtomicUsize,
    queued: &Mutex<HashSet<(String, String)>>,
) -> anyhow::Result<()> {
    // Fetch releases from this source
    let releases = poll_source(source).await?;

    if releases.is_empty() {
        return Ok(());
    }

    // Find the best (newest non-prerelease) version
    let best = releases.iter().filter(|r| !r.is_prerelease).max_by(|a, b| {
        let va = extract_version_from_tag(&a.tag_name);
        let vb = extract_version_from_tag(&b.tag_name);
        compare_version_components(va, vb)
    });

    let best = match best {
        Some(r) => r,
        None => return Ok(()),
    };

    let latest_version = extract_version_from_tag(&best.tag_name).to_owned();

    // Look up all attr_paths that track this source
    let entries = db
        .get_attr_paths_for_source(upstream_type, upstream_key, instance)
        .await?;

    for entry in &entries {
        let current = entry.current_version.as_deref().unwrap_or("");

        // Skip if already at latest
        if current == latest_version {
            continue;
        }

        // Skip if the "new" version is actually older
        if compare_version_components(&latest_version, current) != std::cmp::Ordering::Greater {
            continue;
        }

        // Respect database backoff
        match db.should_check_update(&entry.attr_path).await {
            Ok(false) => {
                debug!("{}: in backoff period, skipping", entry.attr_path);
                continue;
            },
            Err(e) => {
                debug!("{}: backoff check failed: {}", entry.attr_path, e);
            },
            Ok(true) => {},
        }

        // Deduplicate
        let dedup_key = (entry.upstream_key.clone(), latest_version.clone());
        {
            let mut q = queued.lock().await;
            if !q.insert(dedup_key) {
                debug!(
                    "{}: update already queued for {} {}",
                    entry.attr_path, entry.upstream_key, latest_version
                );
                continue;
            }
        }

        if dry_run {
            info!(
                "{}: would update {} -> {} (dry-run)",
                entry.attr_path, current, latest_version
            );
        } else {
            info!(
                "{}: new release detected: {} -> {}",
                entry.attr_path, current, latest_version
            );
        }

        let req = UpdateRequest {
            attr_path: entry.attr_path.clone(),
            drv_path: None,
            current_version: current.to_owned(),
            new_version: latest_version.clone(),
        };

        if let Err(e) = tx.send(req) {
            warn!("{}: failed to send update request: {}", entry.attr_path, e);
        }

        dispatched.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    Ok(())
}
