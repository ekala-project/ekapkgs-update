//! Main processing loop for the autofix queue.
//!
//! Dequeues failed updates one at a time, sends them to the LLM for a fix
//! proposal, applies and validates the fix, and records the outcome.

use std::path::PathBuf;

use anyhow::Context;
use tokio::fs;
use tracing::{debug, info, warn};

use super::config::AutofixConfig;
use super::prompt::{build_prompt, is_llm_fixable};
use super::retriever::{build_error_summary, retrieve_similar_fixes, store_embedding};
use super::validator::{ValidationResult, apply_and_validate};
use crate::database::Database;
use crate::llm::LlmClient;

/// Statistics from a processing run.
#[derive(Debug, Default)]
pub struct ProcessingStats {
    pub processed: usize,
    pub fixed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub escalated: usize,
    pub llm_errors: usize,
    pub parse_errors: usize,
}

impl std::fmt::Display for ProcessingStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Processed: {}, Fixed: {}, Failed: {}, Skipped: {}, Escalated: {}, LLM errors: {}, Parse errors: {}",
            self.processed, self.fixed, self.failed, self.skipped,
            self.escalated, self.llm_errors, self.parse_errors
        )
    }
}

/// Process the autofix queue serially.
///
/// Returns aggregate statistics for the run.
pub async fn process_queue(
    db: &Database,
    llm: &LlmClient,
    config: &AutofixConfig,
) -> anyhow::Result<ProcessingStats> {
    // Crash recovery: reset any items stuck in "processing"
    let reset = db.reset_stale_autofix_processing().await?;
    if reset > 0 {
        info!("Reset {} stale processing items back to queued", reset);
    }

    let mut stats = ProcessingStats::default();
    let mut items_processed = 0usize;

    loop {
        // Check processing limit
        if let Some(limit) = config.limit {
            if items_processed >= limit {
                info!("Reached processing limit of {}", limit);
                break;
            }
        }

        // Dequeue next item
        let Some(item) = db.dequeue_next_autofix().await? else {
            debug!("Queue is empty, stopping");
            break;
        };

        items_processed += 1;
        stats.processed += 1;

        info!(
            "{}: Processing autofix (attempt {}/{})",
            item.attr_path,
            item.attempts + 1,
            item.max_attempts
        );

        // Check if error type is LLM-fixable
        if !is_llm_fixable(&item.error_type) {
            info!(
                "{}: Error type '{}' is not LLM-fixable, skipping",
                item.attr_path, item.error_type
            );
            db.update_autofix_status(item.id, "skipped").await?;
            stats.skipped += 1;
            continue;
        }

        // Filter by requested error types
        if let Some(ref types) = config.error_types {
            if !types.iter().any(|t| t == &item.error_type) {
                debug!(
                    "{}: Skipping (error type '{}' not in filter)",
                    item.attr_path, item.error_type
                );
                // Put it back as queued (don't count against attempts)
                db.update_autofix_status(item.id, "skipped").await?;
                stats.skipped += 1;
                continue;
            }
        }

        // Load failure artifacts
        let artifacts_path = match &item.artifacts_path {
            Some(p) => PathBuf::from(p),
            None => {
                warn!("{}: No artifacts path, skipping", item.attr_path);
                db.update_autofix_status(item.id, "skipped").await?;
                stats.skipped += 1;
                continue;
            },
        };

        // Load error context
        let error_context = load_json_file(&artifacts_path.join("error-context.json")).await;
        let error_context = error_context.unwrap_or_else(|e| {
            warn!("{}: Failed to load error context: {}", item.attr_path, e);
            serde_json::json!({"error_type": item.error_type})
        });

        // Load build log
        let build_log = fs::read_to_string(artifacts_path.join("build.log"))
            .await
            .ok();

        // Find the Nix file in the worktree and read it
        let worktree_path = artifacts_path.join("worktree");
        if !worktree_path.exists() {
            warn!(
                "{}: Worktree not found at {}, skipping",
                item.attr_path,
                worktree_path.display()
            );
            db.update_autofix_status(item.id, "skipped").await?;
            stats.skipped += 1;
            continue;
        }

        let nix_file_info = find_nix_file(&worktree_path, &item.attr_path).await;
        let (nix_file_path, nix_file_content) = match nix_file_info {
            Ok((path, content)) => (path, content),
            Err(e) => {
                warn!("{}: Failed to find Nix file: {}", item.attr_path, e);
                db.update_autofix_status(item.id, "skipped").await?;
                stats.skipped += 1;
                continue;
            },
        };

        // Get previous attempts for retry context
        let previous_attempts = db.get_autofix_attempts(item.id).await?;
        let last_attempt = previous_attempts.last();

        // RAG: retrieve similar successful fixes
        let error_summary = build_error_summary(
            &item.attr_path,
            &item.error_type,
            build_log.as_deref(),
        );
        let similar_fixes = retrieve_similar_fixes(
            db, llm, &item.error_type, &error_summary,
        )
        .await
        .unwrap_or_default();

        // Build prompt (with RAG examples if available)
        let messages = build_prompt(
            &item.attr_path,
            &error_context,
            &nix_file_path,
            &nix_file_content,
            build_log.as_deref(),
            last_attempt,
            &similar_fixes,
        );

        let prompt_text = messages
            .iter()
            .map(|m| format!("[{}] {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n---\n");

        if config.dry_run {
            info!("{}: [dry-run] Would send prompt to LLM", item.attr_path);
            println!("\n--- Prompt for {} ---\n{}\n---\n", item.attr_path, prompt_text);
            db.update_autofix_status(item.id, "queued").await?;
            continue;
        }

        // Call LLM
        let response = match llm.chat_completion(messages).await {
            Ok(r) => r,
            Err(e) => {
                warn!("{}: LLM request failed: {}", item.attr_path, e);
                db.record_autofix_attempt(
                    item.id,
                    item.attempts + 1,
                    Some(&prompt_text),
                    None,
                    None,
                    false,
                    None,
                    None,
                    "llm_error",
                    Some(&format!("{e:#}")),
                    None,
                    None,
                )
                .await?;

                handle_failure(db, &item, &mut stats).await?;
                stats.llm_errors += 1;
                continue;
            },
        };

        let response_text = response.content().unwrap_or_default().to_owned();
        let usage = response.usage.as_ref();
        let prompt_tokens = usage.map(|u| i64::from(u.prompt_tokens));
        let completion_tokens = usage.map(|u| i64::from(u.completion_tokens));

        debug!(
            "{}: LLM response ({} chars): {}",
            item.attr_path,
            response_text.len(),
            &response_text[..response_text.len().min(200)]
        );

        // Parse JSON from response
        let changes = match extract_json(&response_text) {
            Ok(json) => json,
            Err(e) => {
                warn!("{}: Failed to parse LLM response: {}", item.attr_path, e);
                db.record_autofix_attempt(
                    item.id,
                    item.attempts + 1,
                    Some(&prompt_text),
                    Some(&response_text),
                    None,
                    false,
                    None,
                    None,
                    "parse_error",
                    Some(&format!("{e:#}")),
                    prompt_tokens,
                    completion_tokens,
                )
                .await?;

                handle_failure(db, &item, &mut stats).await?;
                stats.parse_errors += 1;
                continue;
            },
        };

        let changes_json_str = serde_json::to_string(&changes).unwrap_or_default();

        // Apply and validate
        let validation = apply_and_validate(
            &worktree_path,
            &item.attr_path,
            &changes,
            &config.eval_entry_point,
        )
        .await?;

        match validation {
            ValidationResult::BuildSuccess => {
                info!("{}: LLM fix validated successfully!", item.attr_path);

                let attempt_id = db.record_autofix_attempt(
                    item.id,
                    item.attempts + 1,
                    Some(&prompt_text),
                    Some(&response_text),
                    Some(&changes_json_str),
                    true,
                    Some(true),
                    None,
                    "success",
                    None,
                    prompt_tokens,
                    completion_tokens,
                )
                .await?;

                // Store embedding for RAG (successful fix — high value)
                if let Err(e) = store_embedding(
                    db, llm, attempt_id, &item.error_type,
                    &error_summary, Some(&changes_json_str), true,
                ).await {
                    debug!("{}: Failed to store embedding (non-fatal): {}", item.attr_path, e);
                }

                db.update_autofix_status(item.id, "fixed").await?;
                stats.fixed += 1;
            },
            ValidationResult::BuildFailed { stderr } => {
                warn!("{}: LLM fix did not resolve build failure", item.attr_path);

                let attempt_id = db.record_autofix_attempt(
                    item.id,
                    item.attempts + 1,
                    Some(&prompt_text),
                    Some(&response_text),
                    Some(&changes_json_str),
                    true,
                    Some(false),
                    Some(&stderr),
                    "build_failed",
                    None,
                    prompt_tokens,
                    completion_tokens,
                )
                .await?;

                // Store embedding for RAG (failed fix — useful for negative examples)
                if let Err(e) = store_embedding(
                    db, llm, attempt_id, &item.error_type,
                    &error_summary, None, false,
                ).await {
                    debug!("{}: Failed to store embedding (non-fatal): {}", item.attr_path, e);
                }

                handle_failure(db, &item, &mut stats).await?;
            },
            ValidationResult::ApplyFailed { error } => {
                warn!("{}: Failed to apply LLM changes: {}", item.attr_path, error);

                db.record_autofix_attempt(
                    item.id,
                    item.attempts + 1,
                    Some(&prompt_text),
                    Some(&response_text),
                    Some(&changes_json_str),
                    false,
                    None,
                    None,
                    "apply_error",
                    Some(&error),
                    prompt_tokens,
                    completion_tokens,
                )
                .await?;

                handle_failure(db, &item, &mut stats).await?;
            },
            ValidationResult::EvalFailed { stderr } => {
                warn!("{}: Nix eval failed after applying fix", item.attr_path);

                db.record_autofix_attempt(
                    item.id,
                    item.attempts + 1,
                    Some(&prompt_text),
                    Some(&response_text),
                    Some(&changes_json_str),
                    true,
                    Some(false),
                    Some(&stderr),
                    "build_failed",
                    Some("Nix evaluation failed"),
                    prompt_tokens,
                    completion_tokens,
                )
                .await?;

                handle_failure(db, &item, &mut stats).await?;
            },
        }
    }

    Ok(stats)
}

/// Handle a failed attempt: re-queue if under max attempts, otherwise escalate.
async fn handle_failure(
    db: &Database,
    item: &super::queue::AutofixQueueItem,
    stats: &mut ProcessingStats,
) -> anyhow::Result<()> {
    if item.attempts + 1 >= item.max_attempts {
        info!(
            "{}: Max attempts ({}) reached, escalating to human",
            item.attr_path, item.max_attempts
        );
        db.update_autofix_status(item.id, "escalated").await?;
        stats.escalated += 1;
    } else {
        debug!(
            "{}: Re-queuing for another attempt ({}/{})",
            item.attr_path,
            item.attempts + 1,
            item.max_attempts
        );
        db.update_autofix_status(item.id, "queued").await?;
        stats.failed += 1;
    }
    Ok(())
}

// ── JSON extraction ──────────────────────────────────────────────────────────

/// Extract a JSON object from an LLM response.
///
/// Tries multiple strategies since small models often wrap JSON in markdown
/// or add commentary:
/// 1. Direct parse of the full response
/// 2. Extract from ```json ... ``` code block
/// 3. Extract from ``` ... ``` code block
/// 4. Regex match for outermost `{ ... }`
fn extract_json(response: &str) -> anyhow::Result<serde_json::Value> {
    let trimmed = response.trim();

    // Strategy 1: Direct parse
    if let Ok(value) = serde_json::from_str(trimmed) {
        return Ok(value);
    }

    // Strategy 2: ```json ... ``` code block
    if let Some(json_str) = extract_code_block(trimmed, "json") {
        if let Ok(value) = serde_json::from_str(json_str) {
            return Ok(value);
        }
    }

    // Strategy 3: ``` ... ``` code block (any language or none)
    if let Some(json_str) = extract_code_block(trimmed, "") {
        if let Ok(value) = serde_json::from_str(json_str) {
            return Ok(value);
        }
    }

    // Strategy 4: Find outermost { ... } via brace matching
    if let Some(json_str) = extract_braced(trimmed) {
        if let Ok(value) = serde_json::from_str(json_str) {
            return Ok(value);
        }
    }

    anyhow::bail!("Could not extract valid JSON from LLM response")
}

/// Extract content from a markdown code block.
fn extract_code_block<'a>(text: &'a str, lang: &str) -> Option<&'a str> {
    let pattern = if lang.is_empty() {
        "```"
    } else {
        // We'll handle the lang prefix manually
        "```"
    };

    let start_marker = if lang.is_empty() {
        "```\n"
    } else {
        "" // handled below
    };

    // Find ```json or ```
    let start = if lang.is_empty() {
        text.find(start_marker)?
    } else {
        let marker = format!("```{lang}");
        text.find(&marker)?
    };

    // Skip past the opening marker line
    let content_start = text[start..].find('\n')? + start + 1;

    // Find closing ```
    let end = text[content_start..].find(pattern)? + content_start;

    Some(text[content_start..end].trim())
}

/// Find the outermost `{ ... }` in the text via brace matching.
fn extract_braced(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0i32;

    for (i, ch) in text[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            },
            _ => {},
        }
    }

    None
}

// ── File loading helpers ─────────────────────────────────────────────────────

async fn load_json_file(path: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("parse JSON from {}", path.display()))
}

/// Find the Nix file for a package in a worktree.
///
/// Tries to locate the file by checking the diff or by searching for
/// common patterns.
async fn find_nix_file(
    worktree_path: &std::path::Path,
    attr_path: &str,
) -> anyhow::Result<(String, String)> {
    // Try to find modified .nix files in the worktree via git diff
    let diff_output = tokio::process::Command::new("git")
        .args([
            "-C",
            &worktree_path.to_string_lossy(),
            "diff",
            "--name-only",
            "HEAD",
        ])
        .output()
        .await?;

    if diff_output.status.success() {
        let files = String::from_utf8_lossy(&diff_output.stdout);
        for file in files.lines() {
            if file.ends_with(".nix") {
                let full_path = worktree_path.join(file);
                if full_path.exists() {
                    let content = fs::read_to_string(&full_path).await?;
                    return Ok((file.to_owned(), content));
                }
            }
        }
    }

    // Fallback: look for default.nix in a path derived from the attr_path
    let parts: Vec<&str> = attr_path.split('.').collect();
    let candidates = vec![
        format!("pkgs/{}/default.nix", parts.last().unwrap_or(&"")),
        "default.nix".to_owned(),
    ];

    for candidate in candidates {
        let full_path = worktree_path.join(&candidate);
        if full_path.exists() {
            let content = fs::read_to_string(&full_path).await?;
            return Ok((candidate, content));
        }
    }

    anyhow::bail!(
        "Could not find Nix file for {} in {}",
        attr_path,
        worktree_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_direct() {
        let input = r#"{"files":[{"path":"test.nix","operations":[]}]}"#;
        let result = extract_json(input).unwrap();
        assert!(result["files"].is_array());
    }

    #[test]
    fn test_extract_json_code_block() {
        let input = "Here's the fix:\n```json\n{\"files\":[{\"path\":\"x.nix\",\"operations\":[]}]}\n```\n";
        let result = extract_json(input).unwrap();
        assert!(result["files"].is_array());
    }

    #[test]
    fn test_extract_json_bare_code_block() {
        let input = "```\n{\"files\":[{\"path\":\"x.nix\",\"operations\":[]}]}\n```";
        let result = extract_json(input).unwrap();
        assert!(result["files"].is_array());
    }

    #[test]
    fn test_extract_json_with_commentary() {
        let input = "I will fix the build error by adding the missing dependency.\n{\"files\":[{\"path\":\"x.nix\",\"operations\":[]}]}\nDone.";
        let result = extract_json(input).unwrap();
        assert!(result["files"].is_array());
    }

    #[test]
    fn test_extract_json_garbage() {
        let input = "I don't know how to fix this.";
        assert!(extract_json(input).is_err());
    }

    #[test]
    fn test_extract_braced_nested() {
        let input = r#"text {outer: {inner: "value"}} more"#;
        let result = extract_braced(input).unwrap();
        assert_eq!(result, r#"{outer: {inner: "value"}}"#);
    }
}
