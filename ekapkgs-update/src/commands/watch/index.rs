//! Upstream index builder: scans all packages via `nix-eval-jobs`, determines
//! each package's [`UpstreamSource`], and stores the mapping in the
//! `upstream_index` database table.

use std::sync::Arc;

use futures::{StreamExt, pin_mut};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::database::Database;
use crate::nix;
use crate::nix::nix_eval_jobs::NixEvalItem;
use crate::package::PackageMetadata;
use crate::vcs_sources::UpstreamSource;

/// Result statistics from an index build run.
#[derive(Debug)]
pub struct IndexStats {
    pub indexed: usize,
    pub skipped: usize,
    pub errors: usize,
}

/// Build (or rebuild) the upstream index by evaluating all packages.
///
/// Clears the existing index, then streams packages from `nix-eval-jobs`,
/// extracts metadata, determines the upstream source, and stores the mapping
/// in the database.
pub async fn build_upstream_index(
    file: &str,
    db: &Database,
    max_eval_workers: Option<usize>,
) -> anyhow::Result<IndexStats> {
    info!("Building upstream index from {}", file);

    db.clear_upstream_index().await?;

    let stream = nix::run_eval::run_nix_eval_jobs_with_workers(file.to_owned(), max_eval_workers);
    pin_mut!(stream);

    // Limit concurrent metadata extraction to avoid OOM from parallel
    // nix-instantiate processes.
    let semaphore = Arc::new(Semaphore::new(8));

    let mut indexed = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;

    while let Some(result) = stream.next().await {
        match result {
            Ok(NixEvalItem::Drv(drv)) => {
                let attr_path = drv.attr.clone();
                let db_clone = db.clone();
                let file_clone = file.to_owned();
                let sem_clone = Arc::clone(&semaphore);

                tokio::spawn(async move {
                    let _permit = sem_clone.acquire().await.expect("indexer semaphore closed");

                    if let Err(e) = index_single_package(&db_clone, &file_clone, &attr_path).await {
                        debug!("{}: indexing failed: {}", attr_path, e);
                    }
                });

                indexed += 1;
            },
            Ok(NixEvalItem::Error(e)) => {
                if e.is_function_error() {
                    skipped += 1;
                } else {
                    debug!("Evaluation error for {}: {}", e.attr, e.error);
                    errors += 1;
                }
            },
            Err(e) => {
                warn!("Stream error during indexing: {}", e);
                break;
            },
        }
    }

    let count = db.upstream_index_count().await.unwrap_or(0);
    info!(
        "Upstream index built: {} packages evaluated, {} indexed, {} skipped, {} errors",
        indexed + skipped + errors,
        count,
        skipped,
        errors
    );

    Ok(IndexStats {
        indexed: count as usize,
        skipped,
        errors,
    })
}

/// Extract metadata for a single package and insert it into the upstream index.
async fn index_single_package(
    db: &Database,
    eval_entry_point: &str,
    attr_path: &str,
) -> anyhow::Result<()> {
    let metadata = PackageMetadata::from_attr_path(eval_entry_point, attr_path).await?;

    // Skip packages that opt out
    if metadata.skip == Some(true) {
        debug!("{}: skipped (ekapkgs-update.skip = true)", attr_path);
        return Ok(());
    }

    // Determine upstream source (same priority as checker.rs)
    let upstream_source = if let Some(ref gh_repo) = metadata.github_repo {
        let parts: Vec<&str> = gh_repo.splitn(2, '/').collect();
        if parts.len() == 2 {
            UpstreamSource::GitHub {
                owner: parts[0].to_owned(),
                repo: parts[1].to_owned(),
            }
        } else {
            debug!(
                "{}: invalid github-repo '{}': expected 'owner/repo'",
                attr_path, gh_repo
            );
            return Ok(());
        }
    } else if let Some(ref src_url) = metadata.src_url {
        match UpstreamSource::from_url(src_url) {
            Some(source) => source,
            None => {
                debug!("{}: could not parse upstream source from URL", attr_path);
                return Ok(());
            },
        }
    } else if let Some(ref pname) = metadata.pname {
        UpstreamSource::PyPI {
            pname: pname.clone(),
        }
    } else {
        debug!("{}: no source URL or pname found", attr_path);
        return Ok(());
    };

    db.upsert_upstream_index(
        attr_path,
        upstream_source.source_type(),
        &upstream_source.source_key(),
        upstream_source.instance(),
        &metadata.version,
        metadata.src_url.as_deref(),
    )
    .await?;

    debug!(
        "{}: indexed as {} / {}",
        attr_path,
        upstream_source.source_type(),
        upstream_source.source_key()
    );

    Ok(())
}
