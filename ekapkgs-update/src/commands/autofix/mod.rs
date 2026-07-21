//! LLM-assisted automatic fix pipeline for failed package builds.
//!
//! This module provides a queue-based system that uses a local LLM to
//! automatically attempt fixes for packages that failed to build during
//! version updates. Failed updates are enqueued, processed serially by
//! the LLM, and validated via `nix-build`.
//!
//! ## Subcommands
//!
//! - `autofix run` — Process the queue (main entry point)
//! - `autofix enqueue` — Add preserved failures to the queue
//! - `autofix status` — Show queue statistics
//! - `autofix history` — Show per-package attempt details
//! - `autofix export-dataset` — Export training data as JSONL

pub mod config;
pub mod dataset;
pub mod processor;
pub mod prompt;
pub mod queue;
pub mod retriever;
pub mod validator;

use std::path::PathBuf;

use tracing::info;

use self::config::AutofixConfig;
use self::dataset::{DatasetExportConfig, DatasetFormat, QualityFilter};
use self::processor::process_queue;
use crate::commands::run::preservation::list_preserved_failures;
use crate::database::Database;
use crate::llm::LlmClient;

/// Run the autofix queue processor.
///
/// Initializes the LLM client, auto-enqueues any new preserved failures,
/// then processes the queue serially.
pub async fn run(config: AutofixConfig) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&config.database_path).to_string();
    let db = Database::new(&expanded_path).await?;

    // Auto-enqueue preserved failures that aren't already queued
    let enqueued = auto_enqueue(&db, config.max_attempts).await?;
    if enqueued > 0 {
        info!("Auto-enqueued {} preserved failures", enqueued);
    }

    // Show initial queue state
    let stats = db.get_autofix_queue_stats().await?;
    if stats.queued == 0 {
        println!("Autofix queue is empty. Nothing to process.");
        return Ok(());
    }

    println!(
        "Autofix queue: {} queued, {} processing, {} fixed, {} escalated",
        stats.queued, stats.processing, stats.fixed, stats.escalated
    );

    if config.dry_run {
        println!("\n[dry-run mode] Prompts will be displayed but not sent to LLM.\n");
    }

    // Initialize LLM client (skip in dry-run mode)
    let llm = if config.dry_run {
        // Create a dummy client — it won't be called in dry-run
        None
    } else {
        Some(LlmClient::from_env()?)
    };

    // For dry-run, we still need a client reference. Create a placeholder.
    let llm_ref;
    let dummy_llm;
    if let Some(ref client) = llm {
        llm_ref = client;
    } else {
        // In dry-run mode we still call process_queue, which will skip the
        // LLM call. We need a valid client reference though.
        dummy_llm = LlmClient::from_env().ok();
        if let Some(ref client) = dummy_llm {
            llm_ref = client;
        } else {
            // If we can't create a client at all in dry-run, just print
            // the queue and exit
            println!("\nEKAPKGS_LLM_BASE_URL not set. Set it to process the queue.");
            return Ok(());
        }
    }

    let run_stats = process_queue(&db, llm_ref, &config).await?;

    println!("\nAutofix run complete: {run_stats}");

    Ok(())
}

/// Show autofix queue status.
pub async fn status(database_path: String) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&database_path).to_string();
    let db = Database::new(&expanded_path).await?;

    let stats = db.get_autofix_queue_stats().await?;

    println!("Autofix Queue Status:");
    println!("  Queued:      {}", stats.queued);
    println!("  Processing:  {}", stats.processing);
    println!("  Fixed:       {}", stats.fixed);
    println!("  Escalated:   {}", stats.escalated);
    println!("  Skipped:     {}", stats.skipped);

    // Show recent fixes
    let history = db.get_autofix_history(None, Some(10)).await?;

    let recent_fixes: Vec<_> = history.iter().filter(|i| i.status == "fixed").collect();
    if !recent_fixes.is_empty() {
        println!("\nRecent Fixes:");
        for item in recent_fixes.iter().take(5) {
            println!(
                "  {} — fixed after {} attempt(s)",
                item.attr_path, item.attempts
            );
        }
    }

    let escalated: Vec<_> = history.iter().filter(|i| i.status == "escalated").collect();
    if !escalated.is_empty() {
        println!("\nEscalated (needs human):");
        for item in escalated.iter().take(5) {
            println!(
                "  {} — {} attempts exhausted",
                item.attr_path, item.max_attempts
            );
        }
    }

    Ok(())
}

/// Show autofix attempt history.
pub async fn history(
    database_path: String,
    package: Option<String>,
    limit: Option<usize>,
) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&database_path).to_string();
    let db = Database::new(&expanded_path).await?;

    let items = db
        .get_autofix_history(package.as_deref(), limit.or(Some(20)))
        .await?;

    if items.is_empty() {
        println!("No autofix history found.");
        return Ok(());
    }

    for item in &items {
        println!(
            "\n{} [{}] — {} attempt(s)",
            item.attr_path, item.status, item.attempts
        );
        println!("  Error: {} (phase: {})", item.error_type, item.failed_phase);
        println!("  Session: {}", item.session_id);

        let attempts = db.get_autofix_attempts(item.id).await?;
        for attempt in &attempts {
            println!(
                "  Attempt #{}: {} ({})",
                attempt.attempt_number,
                attempt.status,
                attempt
                    .completed_at
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "in progress".to_owned())
            );

            if let (Some(pt), Some(ct)) = (attempt.prompt_tokens, attempt.completion_tokens) {
                println!("    Tokens: {pt} prompt + {ct} completion");
            }

            if let Some(ref err) = attempt.error_message {
                println!("    Error: {err}");
            }
        }
    }

    Ok(())
}

/// Manually enqueue preserved failures.
pub async fn enqueue(
    database_path: String,
    package: Option<String>,
    session: Option<String>,
    max_attempts: i64,
) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&database_path).to_string();
    let db = Database::new(&expanded_path).await?;

    let artifacts = list_preserved_failures().await?;

    if artifacts.is_empty() {
        println!("No preserved failures found.");
        return Ok(());
    }

    let mut enqueued = 0;

    for artifact in &artifacts {
        // Filter by package
        if let Some(ref pkg) = package {
            if !artifact.attr_path.contains(pkg.as_str()) {
                continue;
            }
        }

        // Filter by session
        if let Some(ref sess) = session {
            if &artifact.session_id != sess {
                continue;
            }
        }

        // Skip if already queued
        if db.is_autofix_queued(&artifact.attr_path).await? {
            println!("  {} — already queued, skipping", artifact.attr_path);
            continue;
        }

        let artifacts_path = artifact
            .worktree_path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        db.enqueue_autofix(
            &artifact.attr_path,
            &artifact.session_id,
            artifact.failed_phase.as_str(),
            artifact.failed_phase.as_str(),
            &artifacts_path,
            max_attempts,
        )
        .await?;

        println!("  {} — enqueued", artifact.attr_path);
        enqueued += 1;
    }

    println!("\nEnqueued {} items for autofix.", enqueued);

    Ok(())
}

/// Export training dataset.
pub async fn export_dataset_cmd(
    database_path: String,
    format: String,
    quality: String,
    error_type: Option<String>,
    since_days: Option<u32>,
    min_samples: Option<usize>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&database_path).to_string();
    let db = Database::new(&expanded_path).await?;

    let config = DatasetExportConfig {
        format: DatasetFormat::parse(&format)?,
        quality: QualityFilter::parse(&quality)?,
        error_type,
        since_days,
        min_samples,
        output,
    };

    dataset::export_dataset(&db, config).await
}

/// Auto-enqueue all preserved failures that aren't already in the queue.
async fn auto_enqueue(db: &Database, max_attempts: i64) -> anyhow::Result<usize> {
    let artifacts = list_preserved_failures().await?;
    let mut count = 0;

    for artifact in &artifacts {
        if db.is_autofix_queued(&artifact.attr_path).await? {
            continue;
        }

        let artifacts_path = artifact
            .worktree_path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        db.enqueue_autofix(
            &artifact.attr_path,
            &artifact.session_id,
            artifact.failed_phase.as_str(),
            artifact.failed_phase.as_str(),
            &artifacts_path,
            max_attempts,
        )
        .await?;

        count += 1;
    }

    Ok(count)
}
