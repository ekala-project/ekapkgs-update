//! Worktrees command for managing preserved failure artifacts

use anyhow::Context;
use tracing::info;

use super::run::preservation::{cleanup_old_failures, list_preserved_failures};

/// List all preserved failed worktrees
pub async fn list_worktrees(database_path: String) -> anyhow::Result<()> {
    let _expanded_path = shellexpand::tilde(&database_path).to_string();

    let artifacts = list_preserved_failures()
        .await
        .context("Failed to list preserved failures")?;

    if artifacts.is_empty() {
        println!("No preserved failure artifacts found.");
        return Ok(());
    }

    println!("\nPreserved Failure Artifacts ({} total):\n", artifacts.len());
    println!("{:<40} {:<20} {:<20}", "Package", "Failed Phase", "Timestamp");
    println!("{}", "-".repeat(80));

    for artifact in &artifacts {
        let timestamp = artifact.timestamp.format("%Y-%m-%d %H:%M:%S");
        println!(
            "{:<40} {:<20} {}",
            truncate(&artifact.attr_path, 40),
            artifact.failed_phase.as_str(),
            timestamp
        );
    }

    println!();
    Ok(())
}

/// Show details of a specific preserved worktree
pub async fn show_worktree(database_path: String, attr_path: String) -> anyhow::Result<()> {
    let _expanded_path = shellexpand::tilde(&database_path).to_string();

    let artifacts = list_preserved_failures()
        .await
        .context("Failed to list preserved failures")?;

    // Find the most recent artifact for this package
    let artifact = artifacts
        .iter()
        .find(|a| a.attr_path == attr_path)
        .with_context(|| format!("No preserved artifacts found for {}", attr_path))?;

    println!("\n{}", "=".repeat(80));
    println!("Preserved Failure Artifacts for {}", attr_path);
    println!("{}", "=".repeat(80));
    println!();
    println!("Session ID:    {}", artifact.session_id);
    println!("Failed Phase:  {}", artifact.failed_phase.as_str());
    println!("Timestamp:     {}", artifact.timestamp.format("%Y-%m-%d %H:%M:%S"));
    println!();
    println!("Artifact Locations:");
    println!("  Worktree:        {}", artifact.worktree_path.display());
    if let Some(ref build_log) = artifact.build_log_path {
        println!("  Build Log:       {}", build_log.display());
    }
    if let Some(ref test_output) = artifact.test_output_path {
        println!("  Test Output:     {}", test_output.display());
    }
    println!("  Diff:            {}", artifact.diff_path.display());
    println!("  Metadata:        {}", artifact.metadata_path.display());
    println!("  Error Context:   {}", artifact.error_context_path.display());
    println!();

    // Read and display error context
    if artifact.error_context_path.exists() {
        let error_context = tokio::fs::read_to_string(&artifact.error_context_path)
            .await
            .context("Failed to read error context")?;
        println!("Error Context:");
        println!("{}", error_context);
        println!();
    }

    println!("{}", "=".repeat(80));
    println!();

    Ok(())
}

/// Clean up old preserved worktrees
pub async fn clean_worktrees(older_than_days: u32) -> anyhow::Result<()> {
    info!("Cleaning preserved failures older than {} days", older_than_days);

    let removed = cleanup_old_failures(older_than_days)
        .await
        .context("Failed to clean up old failures")?;

    println!("Removed {} old failure artifact(s)", removed);
    Ok(())
}

/// Truncate a string to a maximum length, adding "..." if truncated
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
