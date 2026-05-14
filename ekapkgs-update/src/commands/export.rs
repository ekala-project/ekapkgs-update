//! Export command for LLM-friendly failure context

use std::path::PathBuf;

use anyhow::{Context, bail};
use serde::Serialize;
use serde_json::json;
use tokio::fs;
use tracing::info;

use crate::commands::run::preservation::list_preserved_failures;
use crate::database::Database;

/// Export format for failure context
#[derive(Debug, Clone, Copy)]
pub enum ExportFormat {
    Json,
    Markdown,
}

impl ExportFormat {
    pub fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.to_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "markdown" | "md" => Ok(Self::Markdown),
            _ => bail!("Unknown export format: {}. Use 'json' or 'markdown'", s),
        }
    }
}

/// Complete context for LLM analysis
#[derive(Debug, Serialize)]
struct LlmContext {
    package: String,
    session_id: String,
    failed_phase: String,
    timestamp: String,
    error: serde_json::Value,
    diff: Option<String>,
    build_log: Option<String>,
    test_output: Option<String>,
    metadata: Option<serde_json::Value>,
    previous_attempts: Vec<PreviousAttempt>,
}

#[derive(Debug, Serialize)]
struct PreviousAttempt {
    session_id: String,
    timestamp: String,
    phases: Vec<PhaseInfo>,
}

#[derive(Debug, Serialize)]
struct PhaseInfo {
    phase: String,
    status: String,
    duration_ms: Option<i64>,
    error_type: Option<String>,
}

/// Export failure context as LLM-friendly JSON or Markdown
pub async fn export(
    database_path: String,
    attr_path: String,
    format: ExportFormat,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&database_path).to_string();
    let db = Database::new(&expanded_path).await?;

    info!("Exporting failure context for: {}", attr_path);

    // Find the most recent preserved failure
    let artifacts_list = list_preserved_failures().await?;
    let artifact = artifacts_list
        .iter()
        .find(|a| a.attr_path == attr_path)
        .with_context(|| format!("No preserved artifacts found for {}", attr_path))?;

    info!(
        "Found preserved failure from session {}",
        artifact.session_id
    );

    // Load error context
    let error_context_path = artifact.error_context_path.clone();
    let error_json = if error_context_path.exists() {
        let content = fs::read_to_string(&error_context_path).await?;
        serde_json::from_str(&content).unwrap_or_else(|_| json!({"error": "Failed to parse"}))
    } else {
        json!({"error": "Error context not found"})
    };

    // Load diff
    let diff = if artifact.diff_path.exists() {
        Some(fs::read_to_string(&artifact.diff_path).await?)
    } else {
        None
    };

    // Load build log
    let build_log = if let Some(ref log_path) = artifact.build_log_path {
        if log_path.exists() {
            Some(fs::read_to_string(log_path).await?)
        } else {
            None
        }
    } else {
        None
    };

    // Load test output
    let test_output = if let Some(ref test_path) = artifact.test_output_path {
        if test_path.exists() {
            Some(fs::read_to_string(test_path).await?)
        } else {
            None
        }
    } else {
        None
    };

    // Load metadata
    let metadata = if artifact.metadata_path.exists() {
        let content = fs::read_to_string(&artifact.metadata_path).await?;
        serde_json::from_str(&content).ok()
    } else {
        None
    };

    // Get historical context from database
    let phases = db.get_phases_by_attr(&attr_path).await?;
    let mut sessions: std::collections::HashMap<String, Vec<_>> = std::collections::HashMap::new();
    for phase in &phases {
        sessions
            .entry(phase.session_id.clone())
            .or_default()
            .push(phase);
    }

    // Build previous attempts list (excluding current session)
    let mut previous_attempts = Vec::new();
    for (session_id, session_phases) in sessions.iter() {
        if session_id != &artifact.session_id {
            if let Some(first_phase) = session_phases.first() {
                previous_attempts.push(PreviousAttempt {
                    session_id: session_id.clone(),
                    timestamp: first_phase.started_at.to_rfc3339(),
                    phases: session_phases
                        .iter()
                        .map(|p| PhaseInfo {
                            phase: p.phase.clone(),
                            status: p.status.clone(),
                            duration_ms: p.duration_ms,
                            error_type: p.error_type.clone(),
                        })
                        .collect(),
                });
            }
        }
    }

    // Sort by timestamp descending
    previous_attempts.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    // Build complete context
    let context = LlmContext {
        package: attr_path.clone(),
        session_id: artifact.session_id.clone(),
        failed_phase: artifact.failed_phase.as_str().to_string(),
        timestamp: artifact.timestamp.to_rfc3339(),
        error: error_json,
        diff,
        build_log,
        test_output,
        metadata,
        previous_attempts,
    };

    // Export in requested format
    let exported = match format {
        ExportFormat::Json => export_json(&context)?,
        ExportFormat::Markdown => export_markdown(&context)?,
    };

    // Write to output or stdout
    if let Some(output_path) = output {
        fs::write(&output_path, &exported).await?;
        println!("Exported to: {}", output_path.display());
    } else {
        println!("{}", exported);
    }

    Ok(())
}

/// Export context as JSON
fn export_json(context: &LlmContext) -> anyhow::Result<String> {
    serde_json::to_string_pretty(context).context("Failed to serialize context")
}

/// Export context as Markdown
fn export_markdown(context: &LlmContext) -> anyhow::Result<String> {
    let mut md = String::new();

    md.push_str("# Update Failure Analysis\n\n");

    md.push_str("## Package Information\n\n");
    md.push_str(&format!("- **Package**: `{}`\n", context.package));
    md.push_str(&format!("- **Session ID**: `{}`\n", context.session_id));
    md.push_str(&format!("- **Failed Phase**: `{}`\n", context.failed_phase));
    md.push_str(&format!("- **Timestamp**: `{}`\n", context.timestamp));
    md.push_str("\n");

    md.push_str("## Error Details\n\n");
    md.push_str("```json\n");
    md.push_str(&serde_json::to_string_pretty(&context.error)?);
    md.push_str("\n```\n\n");

    if let Some(ref diff) = context.diff {
        md.push_str("## Changes Made (Git Diff)\n\n");
        md.push_str("```diff\n");
        // Truncate large diffs
        let truncated = if diff.len() > 10000 {
            format!(
                "{}...\n\n(diff truncated to 10KB, full diff available in artifacts)",
                &diff[..10000]
            )
        } else {
            diff.clone()
        };
        md.push_str(&truncated);
        md.push_str("\n```\n\n");
    }

    if let Some(ref build_log) = context.build_log {
        md.push_str("## Build Log\n\n");
        md.push_str("```\n");
        // Show last 5000 chars of build log (most relevant errors are at the end)
        let truncated = if build_log.len() > 5000 {
            format!("...\n\n{}", &build_log[build_log.len() - 5000..])
        } else {
            build_log.clone()
        };
        md.push_str(&truncated);
        md.push_str("\n```\n\n");
    }

    if let Some(ref test_output) = context.test_output {
        md.push_str("## Test Output\n\n");
        md.push_str("```\n");
        let truncated = if test_output.len() > 5000 {
            format!("...\n\n{}", &test_output[test_output.len() - 5000..])
        } else {
            test_output.clone()
        };
        md.push_str(&truncated);
        md.push_str("\n```\n\n");
    }

    if let Some(ref metadata) = context.metadata {
        md.push_str("## Package Metadata\n\n");
        md.push_str("```json\n");
        md.push_str(&serde_json::to_string_pretty(metadata)?);
        md.push_str("\n```\n\n");
    }

    if !context.previous_attempts.is_empty() {
        md.push_str("## Previous Update Attempts\n\n");
        for attempt in &context.previous_attempts {
            md.push_str(&format!(
                "### Session `{}` ({})\n\n",
                &attempt.session_id[..8],
                attempt.timestamp
            ));

            md.push_str("| Phase | Status | Duration |\n");
            md.push_str("|-------|--------|----------|\n");
            for phase in &attempt.phases {
                let duration = if let Some(ms) = phase.duration_ms {
                    format!("{:.1}s", ms as f64 / 1000.0)
                } else {
                    "---".to_string()
                };
                md.push_str(&format!(
                    "| {} | {} | {} |\n",
                    phase.phase, phase.status, duration
                ));
            }
            md.push_str("\n");
        }
    }

    md.push_str("---\n\n");
    md.push_str("*Generated by ekapkgs-update export command*\n");

    Ok(md)
}
