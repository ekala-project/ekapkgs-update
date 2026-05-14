//! Status command for monitoring update sessions

use chrono::Utc;

use crate::database::{Database, SessionStatus};

/// Show status of current/recent update runs
pub async fn status(database_path: String) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&database_path).to_string();
    let db = Database::new(&expanded_path).await?;

    // Get most recent sessions
    let sessions = db.get_recent_sessions(10).await?;

    if sessions.is_empty() {
        println!("No update sessions found.");
        return Ok(());
    }

    // Find the most recent session
    let current = &sessions[0];
    let is_running = current.status == SessionStatus::Running;

    println!("\n{}", "=".repeat(80));
    if is_running {
        println!("Current Session (RUNNING)");
    } else {
        println!("Most Recent Session");
    }
    println!("{}", "=".repeat(80));
    println!();
    println!("Session ID: {}", current.id);
    println!("Status:     {:?}", current.status);
    println!(
        "Started:    {}",
        current.started_at.format("%Y-%m-%d %H:%M:%S")
    );

    if let Some(completed) = current.completed_at {
        let duration = completed.signed_duration_since(current.started_at);
        let mins = duration.num_minutes();
        let secs = duration.num_seconds() % 60;
        println!(
            "Completed:  {} ({}m {}s)",
            completed.format("%Y-%m-%d %H:%M:%S"),
            mins,
            secs
        );
    } else if is_running {
        let duration = Utc::now().signed_duration_since(current.started_at);
        let mins = duration.num_minutes();
        let secs = duration.num_seconds() % 60;
        println!("Running:    {}m {}s", mins, secs);
    }

    println!();
    println!("Progress:");
    println!("  Attempted:  {}", current.packages_attempted);
    println!("  Succeeded:  {}", current.packages_succeeded);
    println!("  Failed:     {}", current.packages_failed);
    println!("  Skipped:    {}", current.packages_skipped);

    // Show currently running phases if session is active
    if is_running {
        let running_phases = db.get_running_phases(&current.id).await?;

        if !running_phases.is_empty() {
            println!();
            println!("Currently Updating:");
            for phase in running_phases {
                let elapsed = Utc::now().signed_duration_since(phase.started_at);
                let mins = elapsed.num_minutes();
                let secs = elapsed.num_seconds() % 60;
                println!(
                    "  {} - {} ({}m {}s)",
                    phase.attr_path, phase.phase, mins, secs
                );
            }
        }
    }

    // Show recent session history
    if sessions.len() > 1 {
        println!();
        println!("{}", "=".repeat(80));
        println!("Recent Sessions");
        println!("{}", "=".repeat(80));
        println!();
        println!(
            "{:<25} {:<12} {:>10} {:>10} {:>10}",
            "Started", "Status", "Success", "Failed", "Duration"
        );
        println!("{}", "-".repeat(80));

        for session in &sessions[1..] {
            let duration_str = if let Some(completed) = session.completed_at {
                let dur = completed.signed_duration_since(session.started_at);
                let mins = dur.num_minutes();
                format!("{}m", mins)
            } else {
                "---".to_string()
            };

            let status_str = format!("{:?}", session.status);

            println!(
                "{:<25} {:<12} {:>10} {:>10} {:>10}",
                session.started_at.format("%Y-%m-%d %H:%M:%S"),
                status_str,
                session.packages_succeeded,
                session.packages_failed,
                duration_str
            );
        }
    }

    println!();
    println!("{}", "=".repeat(80));
    println!();

    Ok(())
}
