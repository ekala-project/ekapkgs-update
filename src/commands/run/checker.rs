use futures::{StreamExt, pin_mut};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::types::UpdateRequest;
use crate::database::Database;
use crate::nix;
use crate::nix::nix_eval_jobs::NixEvalItem;
use crate::package::PackageMetadata;
use crate::vcs_sources::{SemverStrategy, UpstreamSource};

/// Service that monitors packages for new upstream releases
pub async fn release_checker_service(
    file: String,
    db: Database,
    tx: mpsc::UnboundedSender<UpdateRequest>,
    skip_unstable: bool,
    skip_repology: bool,
) -> (usize, usize, usize) {
    let stream = nix::run_eval::run_nix_eval_jobs(file.clone());
    pin_mut!(stream);

    // Create Repology client for cross-distribution version checking
    let repology_client = crate::repology::RepologyClient::new();

    let mut error_count = 0;
    let mut skipped_count = 0;
    let mut checked_count = 0;

    // Consume the stream, checking each package for updates
    while let Some(result) = stream.next().await {
        match result {
            Ok(NixEvalItem::Drv(drv)) => {
                let attr_path = &drv.attr;

                // Check backoff period
                match db.should_check_update(attr_path).await {
                    Ok(false) => {
                        debug!("{}: Skipping (in backoff period)", attr_path);
                        skipped_count += 1;
                        continue;
                    },
                    Ok(true) => {
                        debug!("{}: Checking for updates", attr_path);
                    },
                    Err(e) => {
                        warn!(
                            "{}: Database error checking update status: {}",
                            attr_path, e
                        );
                        // Continue checking anyway
                    },
                }

                checked_count += 1;

                // Clone data for async task
                let db_clone = db.clone();
                let drv_clone = drv.clone();
                let attr_path_clone = attr_path.clone();
                let file_clone = file.clone();
                let tx_clone = tx.clone();
                let repology_client_clone = repology_client.clone();

                // Spawn task to check this package
                tokio::spawn(async move {
                    if let Err(e) = check_for_update(
                        &db_clone,
                        &file_clone,
                        &drv_clone,
                        &attr_path_clone,
                        tx_clone,
                        skip_unstable,
                        &repology_client_clone,
                        skip_repology,
                    )
                    .await
                    {
                        debug!("{}: Error checking for update: {}", attr_path_clone, e);
                    }
                });
            },
            Ok(NixEvalItem::Error(e)) => {
                debug!("Evaluation error: {:?}", e);
                error_count += 1;
            },
            Err(e) => {
                warn!("Stream error: {}", e);
                break;
            },
        }
    }

    info!("Release checker service complete");
    (checked_count, skipped_count, error_count)
}

/// Check a single package for upstream updates
async fn check_for_update(
    db: &Database,
    eval_entry_point: &str,
    drv: &crate::nix::nix_eval_jobs::NixEvalDrv,
    attr_path: &str,
    tx: mpsc::UnboundedSender<UpdateRequest>,
    skip_unstable: bool,
    repology_client: &crate::repology::RepologyClient,
    skip_repology: bool,
) -> anyhow::Result<()> {
    // Extract package metadata
    let metadata = match PackageMetadata::from_attr_path(eval_entry_point, attr_path).await {
        Ok(m) => m,
        Err(e) => {
            debug!("{}: Failed to extract metadata: {}", attr_path, e);
            return Ok(());
        },
    };

    let current_version = &metadata.version;
    debug!("{}: Current version: {}", attr_path, current_version);

    // Skip packages with passthru.ekapkgs-update.skip = true
    if metadata.skip == Some(true) {
        debug!(
            "{}: Skipping due to passthru.ekapkgs-update.skip = true",
            attr_path
        );
        return Ok(());
    }

    // Skip packages with 'unstable' in version if flag is set
    if skip_unstable && current_version.contains("unstable") {
        debug!(
            "{}: Skipping due to --skip-unstable flag (version: {})",
            attr_path, current_version
        );
        return Ok(());
    }

    // Determine upstream source
    let upstream_source = if let Some(ref src_url) = metadata.src_url {
        match UpstreamSource::from_url(src_url) {
            Some(source) => source,
            None => {
                debug!("{}: Could not parse upstream source from URL", attr_path);
                return Ok(());
            },
        }
    } else if let Some(ref pname) = metadata.pname {
        UpstreamSource::PyPI {
            pname: pname.clone(),
        }
    } else {
        debug!("{}: No source URL or pname found", attr_path);
        return Ok(());
    };

    // Fetch latest compatible release
    let best_release = match upstream_source
        .get_compatible_release(current_version, SemverStrategy::Latest, None, None, None)
        .await
    {
        Ok(release) => release,
        Err(e) => {
            debug!("{}: Failed to fetch upstream release: {}", attr_path, e);

            // Try Repology as fallback (if enabled and pname available)
            if !skip_repology {
                if let Some(pname) = metadata.pname.as_ref() {
                    match crate::repology::check_repology_version(
                        db.pool(),
                        repology_client,
                        pname,
                        false,
                    )
                    .await
                    {
                        Ok(Some(repology_info)) => {
                            if let Some(repology_version) = repology_info.get_newest_version() {
                                if repology_version != *current_version {
                                    info!(
                                        "{}: Upstream check failed, but Repology suggests update \
                                         to {}",
                                        attr_path, repology_version
                                    );
                                    // Note: We still don't update since we can't verify the release
                                    // This is just informational
                                }
                            }
                        },
                        Ok(None) => {
                            debug!("{}: Package not found in Repology (fallback)", attr_path);
                        },
                        Err(rep_err) => {
                            debug!("{}: Repology fallback also failed: {}", attr_path, rep_err);
                        },
                    }
                }
            }

            // Record no update available
            if let Err(db_err) = db
                .record_no_update(attr_path, current_version, "unknown")
                .await
            {
                warn!("{}: Failed to record no update: {}", attr_path, db_err);
            }
            return Ok(());
        },
    };

    let latest_version = best_release.version().to_owned();
    debug!("{}: Latest version: {}", attr_path, latest_version);

    // Cross-check with Repology for validation (if enabled)
    if !skip_repology {
        if let Some(pname) = metadata.pname.as_ref() {
            match crate::repology::check_repology_version(db.pool(), repology_client, pname, false)
                .await
            {
                Ok(Some(repology_info)) => {
                    if let Some(repology_newest) = repology_info.get_newest_version() {
                        if repology_newest != latest_version {
                            info!(
                                "{}: Repology reports {} as newest (upstream says {})",
                                attr_path, repology_newest, latest_version
                            );
                        } else {
                            debug!(
                                "{}: Repology confirms {} is newest across distributions",
                                attr_path, latest_version
                            );
                        }
                    }
                },
                Ok(None) => {
                    debug!("{}: Package not found in Repology", attr_path);
                },
                Err(e) => {
                    debug!("{}: Repology check failed: {}", attr_path, e);
                },
            }
        }
    }

    // Check if update is needed
    if current_version == &latest_version {
        // No update needed - record in database
        if let Err(e) = db
            .record_no_update(attr_path, current_version, &latest_version)
            .await
        {
            warn!(
                "{}: Failed to record no update in database: {}",
                attr_path, e
            );
        }
        return Ok(());
    }

    // Check if there's a proposed version that differs from latest
    let record = db.get_update_record(attr_path).await?;
    if let Some(ref rec) = record {
        if let Some(ref proposed) = rec.proposed_version {
            if proposed == &latest_version {
                // Already proposed this version, still waiting for merge
                debug!(
                    "{}: Already proposed version {}, waiting for merge",
                    attr_path, proposed
                );
                if let Err(e) = db
                    .record_no_update(attr_path, current_version, &latest_version)
                    .await
                {
                    warn!("{}: Failed to update database: {}", attr_path, e);
                }
                return Ok(());
            } else {
                // Proposed version differs from latest - attempt new update
                info!(
                    "{}: New version {} available (previously proposed {})",
                    attr_path, latest_version, proposed
                );
            }
        }
    }

    // Update is available - send to updater service
    info!(
        "{}: Update available: {} -> {}",
        attr_path, current_version, latest_version
    );

    let update_req = UpdateRequest {
        attr_path: attr_path.to_owned(),
        drv: drv.clone(),
        current_version: current_version.clone(),
        new_version: latest_version.clone(),
    };

    if let Err(e) = tx.send(update_req) {
        warn!("{}: Failed to send update request: {}", attr_path, e);
    }

    Ok(())
}
