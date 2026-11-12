//! Upstream index builder: scans all packages via `nix-eval-jobs`, determines
//! each package's upstream source and version-selection settings, and stores
//! the mapping in the `upstream_index` database table.
//!
//! Every indexed package is also registered in the `updates` table (with its
//! pname, description, homepage and upstream) so the dashboard shows it
//! before the first poll.
//!
//! Re-indexing refreshes rows in place; rows for packages that disappeared
//! from the repo are pruned afterwards, unless the evaluation looks
//! incomplete (stream errors, nothing evaluated, or far fewer packages than
//! before), in which case the previous index is kept.

use std::sync::Arc;

use futures::{StreamExt, pin_mut};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::database::{Database, IndexedPackage, PackageInfo};
use crate::nix;
use crate::nix::nix_eval_jobs::NixEvalItem;
use crate::package::PackageMetadata;

/// Concurrent nix-instantiate metadata extractions while indexing.
const METADATA_CONCURRENCY: usize = 8;

/// Result statistics from an index build run.
#[derive(Debug, Default)]
pub struct IndexStats {
    /// Derivations produced by nix-eval-jobs.
    pub evaluated: usize,
    /// Packages written to the index.
    pub indexed: usize,
    /// skip=true, no upstream source, pinned, or metadata extraction failed.
    pub skipped: usize,
    /// Evaluation errors (excluding top-level functions).
    pub errors: usize,
    /// Stale index rows removed.
    pub pruned: u64,
    /// The evaluation stream itself failed; the index may be incomplete.
    pub stream_error: Option<String>,
}

enum PackageOutcome {
    Indexed,
    Skipped,
}

/// Build (or rebuild) the upstream index for `repo_name` by evaluating all
/// packages in `file`.
pub async fn build_upstream_index(
    repo_name: &str,
    file: &str,
    db: &Database,
    max_eval_workers: Option<usize>,
) -> IndexStats {
    let label = if repo_name.is_empty() {
        "default"
    } else {
        repo_name
    };
    info!("[{}] Building upstream index from {}", label, file);

    let started = chrono::Utc::now().to_rfc3339();
    let previous = db.upstream_index_count(Some(repo_name)).await.unwrap_or(0);
    let stream = nix::run_eval::run_nix_eval_jobs_with_workers(file.to_owned(), max_eval_workers);
    pin_mut!(stream);

    // Limit concurrent metadata extraction to avoid OOM from parallel
    // nix-instantiate processes.
    let semaphore = Arc::new(Semaphore::new(METADATA_CONCURRENCY));
    let mut tasks: JoinSet<PackageOutcome> = JoinSet::new();
    let mut stats = IndexStats::default();

    while let Some(result) = stream.next().await {
        match result {
            Ok(NixEvalItem::Drv(drv)) => {
                stats.evaluated += 1;
                let db = db.clone();
                let file = file.to_owned();
                let repo_name = repo_name.to_owned();
                let semaphore = Arc::clone(&semaphore);
                tasks.spawn(async move {
                    let Ok(_permit) = semaphore.acquire_owned().await else {
                        return PackageOutcome::Skipped;
                    };
                    index_single_package(&db, &repo_name, &file, &drv.attr)
                        .await
                        .unwrap_or_else(|e| {
                            debug!("{}: indexing failed: {:#}", drv.attr, e);
                            PackageOutcome::Skipped
                        })
                });
            },
            Ok(NixEvalItem::Error(e)) => {
                if e.is_function_error() {
                    stats.skipped += 1;
                } else {
                    debug!("Evaluation error for {}: {}", e.attr, e.error);
                    stats.errors += 1;
                }
            },
            Err(e) => {
                warn!("[{}] Stream error during indexing: {:#}", label, e);
                stats.stream_error.get_or_insert_with(|| format!("{e:#}"));
            },
        }
        // Reap finished tasks so the set doesn't grow unboundedly.
        while let Some(done) = tasks.try_join_next() {
            stats.tally(done);
        }
    }
    while let Some(done) = tasks.join_next().await {
        stats.tally(done);
    }

    if stats.stream_error.is_none() && stats.evaluated == 0 {
        stats.stream_error = Some("nix-eval-jobs produced no derivations".to_owned());
    }

    match prune_decision(&stats, previous) {
        Ok(()) => match db.prune_upstream_index(repo_name, &started).await {
            Ok(n) => stats.pruned = n,
            Err(e) => warn!("[{}] Failed to prune upstream index: {:#}", label, e),
        },
        Err(reason) => warn!("[{}] Keeping stale index entries: {}", label, reason),
    }

    info!(
        "[{}] Upstream index built: {} evaluated, {} indexed, {} skipped, {} errors, {} pruned",
        label, stats.evaluated, stats.indexed, stats.skipped, stats.errors, stats.pruned
    );
    stats
}

impl IndexStats {
    fn tally(&mut self, done: Result<PackageOutcome, tokio::task::JoinError>) {
        match done {
            Ok(PackageOutcome::Indexed) => self.indexed += 1,
            Ok(PackageOutcome::Skipped) => self.skipped += 1,
            Err(e) => {
                warn!("Indexing task panicked: {}", e);
                self.errors += 1;
            },
        }
    }
}

/// Whether rows not refreshed by this run may be deleted. A run that looks
/// incomplete keeps the previous index rather than forgetting packages.
fn prune_decision(stats: &IndexStats, previous: i64) -> Result<(), String> {
    if let Some(e) = &stats.stream_error {
        return Err(format!("evaluation incomplete ({e})"));
    }
    let indexed = i64::try_from(stats.indexed).unwrap_or(i64::MAX);
    if previous >= 20 && indexed * 2 < previous {
        return Err(format!(
            "only {indexed} packages indexed (previously {previous})"
        ));
    }
    Ok(())
}

/// Extract metadata for a single package and insert it into the upstream
/// index and the `updates` table.
async fn index_single_package(
    db: &Database,
    repo_name: &str,
    eval_entry_point: &str,
    attr_path: &str,
) -> anyhow::Result<PackageOutcome> {
    let metadata = PackageMetadata::from_attr_path(eval_entry_point, attr_path).await?;

    // Skip packages that opt out
    if metadata.skip == Some(true) {
        debug!("{}: skipped (ekapkgs-update.skip = true)", attr_path);
        return Ok(PackageOutcome::Skipped);
    }

    // Determine upstream source
    let upstream_source = match metadata.resolve_upstream_source() {
        Ok(source) => source,
        Err(e) => {
            debug!("{}: {}", attr_path, e);
            return Ok(PackageOutcome::Skipped);
        },
    };

    let Some(strategy) = crate::variant_strategy::effective_strategy(
        eval_entry_point,
        attr_path,
        metadata.semver_strategy,
    )
    .await
    else {
        debug!(
            "{}: skipped (pinned by attr_path version suffix)",
            attr_path
        );
        return Ok(PackageOutcome::Skipped);
    };
    let strategy_name = strategy.to_string();
    let upstream_key = upstream_source.source_key();

    db.upsert_indexed_package(&IndexedPackage {
        repo_name,
        attr_path,
        upstream_type: upstream_source.source_type(),
        upstream_key: &upstream_key,
        instance: upstream_source.instance(),
        current_version: &metadata.version,
        src_url: metadata.src_url.as_deref(),
        pname: metadata.pname.as_deref(),
        semver_strategy: Some(&strategy_name),
        version_regex: metadata.version_regex.as_deref(),
        include_prereleases: metadata.include_prereleases.unwrap_or(false),
    })
    .await?;

    db.upsert_package_info(
        repo_name,
        attr_path,
        &metadata.version,
        &PackageInfo::from_metadata(&metadata, Some(&upstream_source), Some(strategy)),
    )
    .await?;

    debug!(
        "{}: indexed as {} / {} ({})",
        attr_path,
        upstream_source.source_type(),
        upstream_key,
        strategy_name
    );

    Ok(PackageOutcome::Indexed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_guard() {
        let stats = |indexed, err: Option<&str>| IndexStats {
            evaluated: indexed,
            indexed,
            stream_error: err.map(str::to_owned),
            ..IndexStats::default()
        };
        assert!(prune_decision(&stats(100, None), 120).is_ok());
        assert!(prune_decision(&stats(5, None), 10).is_ok());
        assert!(prune_decision(&stats(0, None), 0).is_ok());
        assert!(prune_decision(&stats(40, None), 100).is_err());
        assert!(prune_decision(&stats(100, Some("boom")), 100).is_err());
    }
}
