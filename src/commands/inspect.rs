//! Inspect command for detailed failure analysis

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use tokio::fs;
use tracing::info;

use crate::commands::update::errors::UpdateError;
use crate::database::{Database, PhaseRecord};

/// Inspect a package's update history and failure details
pub async fn inspect(database_path: String, identifier: String) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&database_path).to_string();
    let db = Database::new(&expanded_path).await?;

    info!("Inspecting update history for: {}", identifier);

    // Try to find the most recent failures for this package
    let phases = db.get_phases_by_attr(&identifier).await?;

    if phases.is_empty() {
        println!("No update history found for package: {}", identifier);
        return Ok(());
    }

    // Group phases by session
    let mut sessions: HashMap<String, Vec<PhaseRecord>> = HashMap::new();
    for phase in phases {
        sessions
            .entry(phase.session_id.clone())
            .or_default()
            .push(phase);
    }

    // Show most recent session
    let (session_id, session_phases) = sessions
        .iter()
        .max_by_key(|(_, phases)| phases.first().map(|p| &p.started_at))
        .context("No sessions found")?;

    println!("\n{}", "=".repeat(80));
    println!("Update History: {}", identifier);
    println!("{}", "=".repeat(80));
    println!();
    println!("Session ID: {}", session_id);

    // Determine overall status
    let failed_phase = session_phases.iter().find(|p| p.status == "failed");
    let status_str = if let Some(failed) = failed_phase {
        format!("Failed at {} phase", failed.phase)
    } else {
        "Completed successfully".to_string()
    };
    println!("Status:     {}", status_str);

    // Calculate total duration
    let total_duration: i64 = session_phases.iter().filter_map(|p| p.duration_ms).sum();
    let total_secs = total_duration as f64 / 1000.0;
    println!("Duration:   {:.1}s", total_secs);
    println!();

    // Show phase timeline
    println!("Phase Timeline:");
    println!("{}", "-".repeat(80));
    println!("{:30} {:10} {:>10}", "Phase", "Status", "Duration");
    println!("{}", "-".repeat(80));

    for phase in session_phases.iter() {
        let status_symbol = match phase.status.as_str() {
            "success" => "✓",
            "failed" => "✗",
            "skipped" => "⊘",
            _ => "⋯",
        };

        let duration_str = if let Some(ms) = phase.duration_ms {
            format!("{:.1}s", ms as f64 / 1000.0)
        } else {
            "---".to_string()
        };

        println!(
            "{} {:28} {:10} {:>10}",
            status_symbol, phase.phase, phase.status, duration_str
        );
    }
    println!();

    // Show error details if failed
    if let Some(failed) = failed_phase {
        println!("{}", "=".repeat(80));
        println!("Error Details - {} Phase", failed.phase);
        println!("{}", "=".repeat(80));
        println!();

        if let Some(error_json) = &failed.error_details {
            match serde_json::from_str::<UpdateError>(error_json) {
                Ok(error) => {
                    println!("Error Type: {}", error.error_type_name());
                    println!("Summary:    {}", error.summary());
                    println!();
                    println!("Details:");
                    println!("{}", error.details());
                    println!();

                    // Show suggested actions
                    let actions = error.suggested_actions();
                    if !actions.is_empty() {
                        println!("Suggested Actions:");
                        for (i, action) in actions.iter().enumerate() {
                            println!("  {}. {}", i + 1, action);
                        }
                        println!();
                    }
                },
                Err(e) => {
                    println!("Failed to parse error details: {}", e);
                    println!("Raw JSON: {}", error_json);
                    println!();
                },
            }
        }

        // Show artifacts if preserved
        if let Some(artifacts_path) = &failed.artifacts_path {
            println!("{}", "=".repeat(80));
            println!("Preserved Artifacts");
            println!("{}", "=".repeat(80));
            println!();
            println!("Location: {}", artifacts_path);

            let base_path = PathBuf::from(artifacts_path);
            if base_path.exists() {
                println!();
                println!("Available files:");
                list_artifacts(&base_path).await?;

                // Load and show error context for LLM
                let error_context_path = base_path.join("error-context.json");
                if error_context_path.exists() {
                    let context = fs::read_to_string(&error_context_path).await?;
                    println!();
                    println!("{}", "=".repeat(80));
                    println!("Context for LLM Analysis");
                    println!("{}", "=".repeat(80));
                    println!();
                    println!("{}", context);
                }
            } else {
                println!("  (Artifacts no longer exist at this location)");
            }
            println!();
        }
    }

    // Show history summary
    println!("{}", "=".repeat(80));
    println!("Update History ({} session(s))", sessions.len());
    println!("{}", "=".repeat(80));
    println!();

    let mut session_list: Vec<_> = sessions.iter().collect();
    session_list.sort_by(|(_, a), (_, b)| {
        b.first()
            .map(|p| &p.started_at)
            .cmp(&a.first().map(|p| &p.started_at))
    });

    for (sid, phases) in session_list.iter().take(5) {
        let first = phases.first().unwrap();
        let has_failure = phases.iter().any(|p| p.status == "failed");
        let status = if has_failure { "Failed" } else { "Success" };
        let timestamp = first.started_at.format("%Y-%m-%d %H:%M:%S");

        println!("  {} | {} | Session: {}", timestamp, status, &sid[..8]);
    }

    if sessions.len() > 5 {
        println!("  ... and {} more", sessions.len() - 5);
    }

    println!();
    println!("{}", "=".repeat(80));

    Ok(())
}

/// List artifacts in a directory with sizes
async fn list_artifacts(base_path: &PathBuf) -> anyhow::Result<()> {
    let mut entries = fs::read_dir(base_path).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_name = entry.file_name();
        let file_type = entry.file_type().await?;
        let metadata = entry.metadata().await?;

        let size_str = if file_type.is_dir() {
            "DIR".to_string()
        } else {
            format_size(metadata.len())
        };

        println!("  - {} ({})", file_name.to_string_lossy(), size_str);
    }
    Ok(())
}

/// Format file size in human-readable format
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
