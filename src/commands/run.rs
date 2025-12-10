use futures::{StreamExt, pin_mut};
use tracing::{debug, info};

use crate::nix;
use crate::nix::nix_eval_jobs::NixEvalItem;

pub async fn run(file: String) -> anyhow::Result<()> {
    info!("Running nix-eval-jobs on: {}", file);

    let stream = nix::run_eval::run_nix_eval_jobs(file);
    pin_mut!(stream);

    let mut drvs = Vec::new();
    let mut error_count = 0;

    // Consume the stream, processing each item as it arrives
    while let Some(result) = stream.next().await {
        match result {
            Ok(NixEvalItem::Drv(drv)) => {
                // TODO: Actually attempt to update the package
                drvs.push(drv);
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
