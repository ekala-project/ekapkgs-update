//! Phase instrumentation for tracking update workflow execution
//!
//! This module provides wrappers that automatically track phase execution,
//! including start/end times, success/failure status, and error details.

use std::future::Future;
use std::time::Instant;

use tracing::{debug, info, warn};

use super::errors::UpdateError;
use super::types::UpdatePhase;
use crate::database::Database;

/// Execute a phase with full instrumentation
///
/// This wrapper automatically:
/// - Records phase start in the database
/// - Times the execution
/// - Logs progress
/// - Records success/failure with error details
/// - Returns the phase result
///
/// # Example
///
/// ```ignore
/// let result = execute_phase(
///     &session_id,
///     UpdatePhase::Build,
///     "python3Packages.numpy",
///     &db,
///     async {
///         // Your build logic here
///         build_package().await
///     }
/// ).await?;
/// ```
pub async fn execute_phase<F, T>(
    session_id: &str,
    phase: UpdatePhase,
    attr_path: &str,
    db: &Database,
    f: F,
) -> Result<T, UpdateError>
where
    F: Future<Output = Result<T, UpdateError>>,
{
    // Record phase start in database
    let phase_id = db
        .record_phase_start(session_id, attr_path, phase)
        .await
        .map_err(|e| UpdateError::InfrastructureError {
            phase,
            component: "database".to_string(),
            details: format!("Failed to record phase start: {}", e),
        })?;

    info!("{}: {} - starting", attr_path, phase.description());
    let start = Instant::now();

    // Execute the phase
    let result = f.await;
    let duration = start.elapsed();

    // Record result in database
    match &result {
        Ok(_) => {
            debug!(
                "{}: {} - succeeded in {:?}",
                attr_path,
                phase.description(),
                duration
            );
            if let Err(e) = db.record_phase_success(phase_id, duration).await {
                warn!(
                    "{}: Failed to record phase success in database: {}",
                    attr_path, e
                );
            }
        }
        Err(error) => {
            warn!(
                "{}: {} - failed after {:?}: {}",
                attr_path,
                phase.description(),
                duration,
                error.summary()
            );
            // Note: artifacts_path would be set by preservation logic
            if let Err(e) = db.record_phase_failure(phase_id, duration, error, None).await {
                warn!(
                    "{}: Failed to record phase failure in database: {}",
                    attr_path, e
                );
            }
        }
    }

    result
}

/// Execute a phase with instrumentation and artifact preservation on failure
///
/// This is similar to `execute_phase` but also accepts an optional artifacts path
/// to record when failures are preserved for later inspection.
pub async fn execute_phase_with_preservation<F, T>(
    session_id: &str,
    phase: UpdatePhase,
    attr_path: &str,
    db: &Database,
    f: F,
    artifacts_path_fn: impl FnOnce(&UpdateError) -> Option<String>,
) -> Result<T, UpdateError>
where
    F: Future<Output = Result<T, UpdateError>>,
{
    let phase_id = db
        .record_phase_start(session_id, attr_path, phase)
        .await
        .map_err(|e| UpdateError::InfrastructureError {
            phase,
            component: "database".to_string(),
            details: format!("Failed to record phase start: {}", e),
        })?;

    info!("{}: {} - starting", attr_path, phase.description());
    let start = Instant::now();

    let result = f.await;
    let duration = start.elapsed();

    match &result {
        Ok(_) => {
            debug!(
                "{}: {} - succeeded in {:?}",
                attr_path,
                phase.description(),
                duration
            );
            if let Err(e) = db.record_phase_success(phase_id, duration).await {
                warn!(
                    "{}: Failed to record phase success in database: {}",
                    attr_path, e
                );
            }
        }
        Err(error) => {
            warn!(
                "{}: {} - failed after {:?}: {}",
                attr_path,
                phase.description(),
                duration,
                error.summary()
            );

            // Get artifacts path from the provided function
            let artifacts_path = artifacts_path_fn(error);

            if let Err(e) = db
                .record_phase_failure(phase_id, duration, error, artifacts_path)
                .await
            {
                warn!(
                    "{}: Failed to record phase failure in database: {}",
                    attr_path, e
                );
            }
        }
    }

    result
}
