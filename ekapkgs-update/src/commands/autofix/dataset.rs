//! Training dataset export for fine-tuning the autofix LLM.
//!
//! Exports autofix attempt data as JSONL files in either SFT (supervised
//! fine-tuning) or DPO (direct preference optimization) format.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, bail};
use serde::Serialize;
use tracing::info;

use crate::database::Database;

/// Dataset export format.
#[derive(Debug, Clone, Copy)]
pub enum DatasetFormat {
    /// Supervised fine-tuning: messages array with system/user/assistant.
    Sft,
    /// Direct preference optimization: prompt + chosen + rejected pairs.
    Dpo,
}

impl DatasetFormat {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s.to_lowercase().as_str() {
            "sft" => Ok(Self::Sft),
            "dpo" => Ok(Self::Dpo),
            _ => bail!("Unknown dataset format: '{}'. Use 'sft' or 'dpo'.", s),
        }
    }
}

/// Quality filter for dataset export.
#[derive(Debug, Clone, Copy)]
pub enum QualityFilter {
    /// Only attempts where the fix built successfully.
    VerifiedSuccess,
    /// Only attempts where the build failed.
    BuildFailed,
    /// Only attempts where the LLM output couldn't be parsed.
    ParseError,
    /// All attempts regardless of outcome.
    All,
}

impl QualityFilter {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s.to_lowercase().as_str() {
            "verified_success" | "success" => Ok(Self::VerifiedSuccess),
            "build_failed" | "failed" => Ok(Self::BuildFailed),
            "parse_error" => Ok(Self::ParseError),
            "all" => Ok(Self::All),
            _ => bail!(
                "Unknown quality filter: '{}'. Use 'verified_success', 'build_failed', \
                 'parse_error', or 'all'.",
                s
            ),
        }
    }

    fn matches_status(&self, status: &str) -> bool {
        match self {
            Self::VerifiedSuccess => status == "success",
            Self::BuildFailed => status == "build_failed",
            Self::ParseError => status == "parse_error",
            Self::All => true,
        }
    }
}

/// Configuration for dataset export.
pub struct DatasetExportConfig {
    pub format: DatasetFormat,
    pub quality: QualityFilter,
    pub error_type: Option<String>,
    pub since_days: Option<u32>,
    pub min_samples: Option<usize>,
    pub output: Option<PathBuf>,
}

/// SFT training sample (one JSONL line).
#[derive(Serialize)]
struct SftSample {
    messages: Vec<SftMessage>,
    metadata: SampleMetadata,
}

#[derive(Serialize)]
struct SftMessage {
    role: String,
    content: String,
}

/// DPO training sample (one JSONL line).
#[derive(Serialize)]
struct DpoSample {
    prompt: Vec<SftMessage>,
    chosen: String,
    rejected: String,
    metadata: SampleMetadata,
}

#[derive(Serialize)]
struct SampleMetadata {
    attr_path: String,
    error_type: String,
    attempt_number: i64,
    build_success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queue_id: Option<i64>,
}

/// Export the training dataset.
pub async fn export_dataset(db: &Database, config: DatasetExportConfig) -> anyhow::Result<()> {
    // Get all queue items
    let items = db.get_autofix_history(None, None).await?;

    if items.is_empty() {
        println!("No autofix data found.");
        return Ok(());
    }

    // Collect all samples
    let mut samples: Vec<(SampleMetadata, Option<String>, Option<String>, String)> = Vec::new();

    for item in &items {
        // Filter by error type
        if let Some(ref et) = config.error_type {
            if &item.error_type != et {
                continue;
            }
        }

        // Filter by date
        if let Some(days) = config.since_days {
            let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(days));
            if item.created_at < cutoff {
                continue;
            }
        }

        let attempts = db.get_autofix_attempts(item.id).await?;

        for attempt in &attempts {
            if !config.quality.matches_status(&attempt.status) {
                continue;
            }

            let metadata = SampleMetadata {
                attr_path: item.attr_path.clone(),
                error_type: item.error_type.clone(),
                attempt_number: attempt.attempt_number,
                build_success: attempt.build_success,
                queue_id: Some(item.id),
            };

            samples.push((
                metadata,
                attempt.prompt_text.clone(),
                attempt.response_text.clone(),
                attempt.status.clone(),
            ));
        }
    }

    // Check minimum samples
    if let Some(min) = config.min_samples {
        if samples.len() < min {
            println!(
                "Only {} samples found (minimum: {}). Not exporting.",
                samples.len(),
                min
            );
            return Ok(());
        }
    }

    info!("Exporting {} samples", samples.len());

    // Open output
    let mut writer: Box<dyn Write> = if let Some(ref path) = config.output {
        Box::new(
            std::fs::File::create(path)
                .with_context(|| format!("create output file {}", path.display()))?,
        )
    } else {
        Box::new(std::io::stdout())
    };

    match config.format {
        DatasetFormat::Sft => {
            export_sft(&mut writer, &samples)?;
        },
        DatasetFormat::Dpo => {
            export_dpo(&mut writer, db, &items).await?;
        },
    }

    if let Some(ref path) = config.output {
        println!("Exported {} samples to {}", samples.len(), path.display());
    }

    Ok(())
}

/// Export samples in SFT format (one messages array per line).
fn export_sft(
    writer: &mut dyn Write,
    samples: &[(SampleMetadata, Option<String>, Option<String>, String)],
) -> anyhow::Result<()> {
    for (metadata, prompt_text, response_text, _status) in samples {
        let Some(prompt) = prompt_text else { continue };
        let Some(response) = response_text else {
            continue;
        };

        // Parse the prompt back into system/user messages
        let messages = parse_prompt_to_messages(prompt, response);

        let sample = SftSample {
            messages,
            metadata: SampleMetadata {
                attr_path: metadata.attr_path.clone(),
                error_type: metadata.error_type.clone(),
                attempt_number: metadata.attempt_number,
                build_success: metadata.build_success,
                queue_id: metadata.queue_id,
            },
        };

        serde_json::to_writer(&mut *writer, &sample)?;
        writeln!(writer)?;
    }

    Ok(())
}

/// Export samples in DPO format (chosen/rejected pairs).
///
/// Finds queue items that have both successful and failed attempts,
/// then pairs them.
async fn export_dpo(
    writer: &mut dyn Write,
    db: &Database,
    items: &[super::queue::AutofixQueueItem],
) -> anyhow::Result<()> {
    let mut pairs_written = 0;

    for item in items {
        let attempts = db.get_autofix_attempts(item.id).await?;

        // Find a successful attempt and a failed attempt
        let chosen = attempts.iter().find(|a| a.status == "success");
        let rejected = attempts
            .iter()
            .find(|a| a.status == "build_failed" || a.status == "apply_error");

        let (Some(chosen), Some(rejected)) = (chosen, rejected) else {
            continue;
        };

        let Some(ref prompt) = chosen.prompt_text else {
            continue;
        };
        let Some(ref chosen_response) = chosen.response_text else {
            continue;
        };
        let Some(ref rejected_response) = rejected.response_text else {
            continue;
        };

        // Parse prompt into messages (system + user only, no assistant)
        let prompt_messages = parse_prompt_to_prompt_only(prompt);

        let sample = DpoSample {
            prompt: prompt_messages,
            chosen: chosen_response.clone(),
            rejected: rejected_response.clone(),
            metadata: SampleMetadata {
                attr_path: item.attr_path.clone(),
                error_type: item.error_type.clone(),
                attempt_number: chosen.attempt_number,
                build_success: Some(true),
                queue_id: Some(item.id),
            },
        };

        serde_json::to_writer(&mut *writer, &sample)?;
        writeln!(writer)?;
        pairs_written += 1;
    }

    info!("Exported {} DPO pairs", pairs_written);
    Ok(())
}

/// Parse a stored prompt string back into SFT message format.
///
/// The prompt is stored as "[role] content\n---\n[role] content".
fn parse_prompt_to_messages(prompt: &str, response: &str) -> Vec<SftMessage> {
    let mut messages = Vec::new();

    for section in prompt.split("\n---\n") {
        if let Some(content) = section.strip_prefix("[system] ") {
            messages.push(SftMessage {
                role: "system".to_owned(),
                content: content.to_owned(),
            });
        } else if let Some(content) = section.strip_prefix("[user] ") {
            messages.push(SftMessage {
                role: "user".to_owned(),
                content: content.to_owned(),
            });
        }
    }

    messages.push(SftMessage {
        role: "assistant".to_owned(),
        content: response.to_owned(),
    });

    messages
}

/// Parse a stored prompt string into prompt-only messages (no assistant).
fn parse_prompt_to_prompt_only(prompt: &str) -> Vec<SftMessage> {
    let mut messages = Vec::new();

    for section in prompt.split("\n---\n") {
        if let Some(content) = section.strip_prefix("[system] ") {
            messages.push(SftMessage {
                role: "system".to_owned(),
                content: content.to_owned(),
            });
        } else if let Some(content) = section.strip_prefix("[user] ") {
            messages.push(SftMessage {
                role: "user".to_owned(),
                content: content.to_owned(),
            });
        }
    }

    messages
}
