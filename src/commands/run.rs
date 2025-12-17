use futures::{StreamExt, pin_mut};
use tracing::{debug, info, warn};

use crate::database::Database;
use crate::nix;
use crate::nix::nix_eval_jobs::NixEvalItem;
use crate::package::PackageMetadata;
use crate::vcs_sources::{SemverStrategy, UpstreamSource};

pub async fn run(file: String, database_path: String) -> anyhow::Result<()> {
    info!("Running nix-eval-jobs on: {}", file);

    // Expand tilde in database path
    let expanded_db_path = shellexpand::tilde(&database_path).to_string();

    // Initialize database
    let db = Database::new(&expanded_db_path).await?;
    info!("Database initialized at: {}", expanded_db_path);

    let stream = nix::run_eval::run_nix_eval_jobs(file.clone());
    pin_mut!(stream);

    let mut drvs = Vec::new();
    let mut error_count = 0;
    let mut skipped_count = 0;
    let mut checked_count = 0;
    let mut updated_count = 0;
    let mut failed_count = 0;

    // Consume the stream, processing each item as it arrives
    while let Some(result) = stream.next().await {
        match result {
            Ok(NixEvalItem::Drv(drv)) => {
                drvs.push(drv.clone());

                // Check if we should attempt an update for this package
                let attr_path = &drv.attr;

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

                // Attempt to check for updates
                match check_and_update_package(&db, &file, &drv).await {
                    Ok(UpdateResult::Updated {
                        old_version,
                        new_version,
                    }) => {
                        info!(
                            "{}: Updated from {} to {}",
                            attr_path, old_version, new_version
                        );
                        updated_count += 1;
                    },
                    Ok(UpdateResult::NoUpdateNeeded {
                        current_version,
                        latest_version,
                    }) => {
                        debug!(
                            "{}: No update needed (current: {}, latest: {})",
                            attr_path, current_version, latest_version
                        );
                    },
                    Ok(UpdateResult::Skipped(reason)) => {
                        debug!("{}: Skipped - {}", attr_path, reason);
                    },
                    Err(e) => {
                        warn!("{}: Failed to check for updates: {}", attr_path, e);
                        failed_count += 1;
                    },
                }
            },
            Ok(NixEvalItem::Error(e)) => {
                debug!("Evaluation error: {:?}", e);
                error_count += 1;
            },
            Err(e) => {
                return Err(e);
            },
        }
    }

    // Display summary
    info!("Evaluation complete!");
    info!("Total derivations: {}", drvs.len());
    if error_count > 0 {
        info!("Evaluation errors: {}", error_count);
    }
    info!("Update summary:");
    info!("  Checked: {}", checked_count);
    info!("  Skipped (backoff): {}", skipped_count);
    info!("  Updated: {}", updated_count);
    info!("  Failed: {}", failed_count);

    // Count by system
    let mut systems = std::collections::HashMap::new();
    for drv in &drvs {
        *systems.entry(&drv.system).or_insert(0) += 1;
    }

    info!("Derivations by system:");
    for (system, count) in systems {
        info!("  {}: {}", system, count);
    }

    Ok(())
}

#[derive(Debug)]
enum UpdateResult {
    Updated {
        old_version: String,
        new_version: String,
    },
    NoUpdateNeeded {
        current_version: String,
        latest_version: String,
    },
    Skipped(String),
}

/// Check if a package needs updating and attempt to update it
async fn check_and_update_package(
    db: &Database,
    eval_entry_point: &str,
    drv: &crate::nix::nix_eval_jobs::NixEvalDrv,
) -> anyhow::Result<UpdateResult> {
    let attr_path = &drv.attr;

    // Extract package metadata to get current version
    let metadata = match PackageMetadata::from_attr_path(eval_entry_point, attr_path).await {
        Ok(m) => m,
        Err(e) => {
            debug!("{}: Failed to extract metadata: {}", attr_path, e);
            return Ok(UpdateResult::Skipped(
                "Could not extract metadata".to_string(),
            ));
        },
    };

    let current_version = &metadata.version;
    debug!("{}: Current version: {}", attr_path, current_version);

    // Determine upstream source
    let upstream_source = if let Some(ref src_url) = metadata.src_url {
        match UpstreamSource::from_url(src_url) {
            Some(source) => source,
            None => {
                debug!("{}: Could not parse upstream source from URL", attr_path);
                return Ok(UpdateResult::Skipped("Unsupported source".to_string()));
            },
        }
    } else if let Some(ref pname) = metadata.pname {
        UpstreamSource::PyPI {
            pname: pname.clone(),
        }
    } else {
        debug!("{}: No source URL or pname found", attr_path);
        return Ok(UpdateResult::Skipped("No source info".to_string()));
    };

    // Fetch latest compatible release (using Latest strategy)
    let best_release = match upstream_source
        .get_compatible_release(current_version, SemverStrategy::Latest)
        .await
    {
        Ok(release) => release,
        Err(e) => {
            debug!("{}: Failed to fetch upstream release: {}", attr_path, e);
            // Record no update available
            if let Err(db_err) = db
                .record_no_update(attr_path, current_version, "unknown")
                .await
            {
                warn!("{}: Failed to record no update: {}", attr_path, db_err);
            }
            return Ok(UpdateResult::Skipped(
                "Could not fetch upstream".to_string(),
            ));
        },
    };

    let latest_version = UpstreamSource::get_version(&best_release);
    debug!("{}: Latest version: {}", attr_path, latest_version);

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
        return Ok(UpdateResult::NoUpdateNeeded {
            current_version: current_version.to_string(),
            latest_version: latest_version.to_string(),
        });
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
                return Ok(UpdateResult::Skipped("Update already proposed".to_string()));
            } else {
                // Proposed version differs from latest - attempt new update
                info!(
                    "{}: New version {} available (previously proposed {})",
                    attr_path, latest_version, proposed
                );
            }
        }
    }

    // Update is needed - for now, just record it
    // In a full implementation, this would call the update logic
    info!(
        "{}: Update available: {} -> {}",
        attr_path, current_version, latest_version
    );

    // Record as proposed update for now
    if let Err(e) = db
        .record_proposed_update(attr_path, current_version, &latest_version, &latest_version)
        .await
    {
        warn!("{}: Failed to record proposed update: {}", attr_path, e);
    }

    Ok(UpdateResult::Updated {
        old_version: current_version.to_string(),
        new_version: latest_version.to_string(),
    })
}
