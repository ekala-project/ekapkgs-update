//! Query command for searching and filtering update failures

use std::collections::HashMap;

use anyhow::Context;
use chrono::{DateTime, Utc};

use crate::database::{Database, PhaseRecord};

/// Query update failures with flexible filtering
pub async fn query(
    database_path: String,
    error_type: Option<String>,
    phase: Option<String>,
    status: Option<String>,
    package_pattern: Option<String>,
    since_days: Option<u32>,
    limit: Option<usize>,
    group_by_error: bool,
) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&database_path).to_string();
    let db = Database::new(&expanded_path).await?;

    // Convert since_days to DateTime
    let since = since_days.map(|days| Utc::now() - chrono::Duration::days(days as i64));

    // Query phase records first
    let results = db
        .query_phases(
            error_type.as_deref(),
            phase.as_deref(),
            status.as_deref(),
            package_pattern.as_deref(),
            since,
            limit,
        )
        .await
        .context("Failed to query phases")?;

    if !results.is_empty() {
        println!("\nFound {} matching phase record(s)\n", results.len());

        if group_by_error {
            display_grouped_by_error(&results);
        } else {
            display_table(&results);
        }
        return Ok(());
    }

    // Fall back to update_logs for failures recorded by the run command
    let is_failed_query = status.as_deref() == Some("failed") || status.is_none();
    if is_failed_query {
        let failed_logs = db
            .query_failed_logs(package_pattern.as_deref(), since, limit)
            .await
            .context("Failed to query update logs")?;

        if !failed_logs.is_empty() {
            println!("\nFound {} failed update(s)\n", failed_logs.len());
            display_failed_logs(&failed_logs);
            return Ok(());
        }
    }

    println!("No matching records found.");
    Ok(())
}

/// Display results grouped by error type
fn display_grouped_by_error(results: &[PhaseRecord]) {
    let mut by_error: HashMap<String, Vec<&PhaseRecord>> = HashMap::new();
    for record in results {
        let error_type = record
            .error_type
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        by_error.entry(error_type).or_default().push(record);
    }

    // Sort by count descending
    let mut sorted: Vec<_> = by_error.iter().collect();
    sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    for (error_type, records) in sorted {
        println!("● {} ({} occurrence(s))", error_type, records.len());
        for record in records.iter().take(5) {
            println!("  - {}", record.attr_path);
        }
        if records.len() > 5 {
            println!("  ... and {} more", records.len() - 5);
        }
        println!();
    }
}

/// Display results as a table
fn display_table(results: &[PhaseRecord]) {
    println!(
        "{:<40} {:<20} {:<20} {:<15}",
        "Package", "Phase", "Error Type", "When"
    );
    println!("{}", "-".repeat(100));

    for record in results.iter().take(50) {
        let when = format_relative_time(&record.started_at);
        let error_type = record.error_type.as_deref().unwrap_or("N/A");

        println!(
            "{:<40} {:<20} {:<20} {:<15}",
            truncate(&record.attr_path, 40),
            truncate(&record.phase, 20),
            truncate(error_type, 20),
            when
        );
    }

    if results.len() > 50 {
        println!(
            "\n... and {} more (use --limit to see more)",
            results.len() - 50
        );
    }
}

/// Display failed update logs from the update_logs table
fn display_failed_logs(logs: &[crate::database::UpdateLog]) {
    println!(
        "{:<40} {:<15} {:<15} {:<15} {}",
        "Package", "Old", "New", "When", "Error"
    );
    println!("{}", "-".repeat(120));

    for log in logs.iter().take(50) {
        let when = format_relative_time(&log.timestamp_as_datetime());
        let old = log.old_version.as_deref().unwrap_or("?");
        let new = log.new_version.as_deref().unwrap_or("?");
        // First line of error, truncated
        let error_summary = log
            .error_log
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(50)
            .collect::<String>();

        println!(
            "{:<40} {:<15} {:<15} {:<15} {}",
            truncate(&log.attr_path, 40),
            truncate(old, 15),
            truncate(new, 15),
            when,
            error_summary
        );
    }

    if logs.len() > 50 {
        println!(
            "\n... and {} more (use --limit to see more)",
            logs.len() - 50
        );
    }
}

/// Format a relative time string (e.g., "2 hours ago", "3 days ago")
fn format_relative_time(dt: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let diff = now.signed_duration_since(*dt);

    if diff.num_seconds() < 60 {
        "just now".to_string()
    } else if diff.num_minutes() < 60 {
        format!("{}m ago", diff.num_minutes())
    } else if diff.num_hours() < 24 {
        format!("{}h ago", diff.num_hours())
    } else if diff.num_days() < 7 {
        format!("{}d ago", diff.num_days())
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}

/// Truncate a string to a maximum length, adding "..." if truncated
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
