//! Prompt construction for the LLM autofix pipeline.
//!
//! Builds compact, structured prompts optimized for small models (Qwen 2.7B).
//! Each error type gets a focused prompt with only the most relevant context
//! to stay within tight token budgets (~2000 tokens total).

use serde_json::Value;

use crate::llm::types::ChatMessage;

use super::queue::AutofixAttemptRecord;
use super::retriever::SimilarFix;

/// System prompt — kept ultra-concise for small models.
const SYSTEM_PROMPT: &str = "\
You fix Nix package build errors. Output ONLY valid JSON, no explanation.
Format: {\"files\":[{\"path\":\"FILE\",\"operations\":[{\"type\":\"replace\",\"old\":\"EXACT_OLD_TEXT\",\"new\":\"NEW_TEXT\"}]}]}";

/// Build the chat messages for an LLM fix attempt.
///
/// The prompt is tailored to the error type and includes only the most
/// relevant context. Similar successful fixes (from RAG retrieval) are
/// included as few-shot examples. Previous failed attempts are included
/// so the model can try a different approach.
pub fn build_prompt(
    attr_path: &str,
    error_context: &Value,
    nix_file_path: &str,
    nix_file_content: &str,
    build_log: Option<&str>,
    previous_attempt: Option<&AutofixAttemptRecord>,
    similar_fixes: &[SimilarFix],
) -> Vec<ChatMessage> {
    let error_type = error_context["error_type"]
        .as_str()
        .unwrap_or("unknown");

    let suspected_cause = error_context["details"]
        .get("suspected_cause")
        .and_then(|v| v.get("cause"))
        .and_then(|v| v.as_str());

    let mut user_content = String::with_capacity(2000);

    user_content.push_str(&format!("Package: {attr_path}\n"));
    user_content.push_str(&format!("File: {nix_file_path}\n\n"));

    // Include similar successful fixes as few-shot examples (RAG)
    if !similar_fixes.is_empty() {
        user_content.push_str("Similar fixes that worked before:\n");
        for (i, fix) in similar_fixes.iter().enumerate() {
            user_content.push_str(&format!(
                "Example {}: {}\nFix: {}\n",
                i + 1,
                truncate_log(&fix.error_summary, 150),
                truncate_log(&fix.fix_json, 300),
            ));
        }
        user_content.push('\n');
    }

    // Dispatch to error-type-specific prompt builders
    match (error_type, suspected_cause) {
        ("build_error", Some("obsolete_patch")) => {
            build_obsolete_patch_prompt(&mut user_content, error_context, nix_file_content);
        },
        ("build_error", Some("missing_dependency")) => {
            build_missing_dep_prompt(&mut user_content, error_context, nix_file_content, build_log);
        },
        ("test_error", _) => {
            build_test_error_prompt(&mut user_content, error_context, nix_file_content, build_log);
        },
        _ => {
            build_generic_build_prompt(&mut user_content, error_context, nix_file_content, build_log);
        },
    }

    // Add toolchain hints
    if let Some(hints) = toolchain_hints(build_log.unwrap_or_default()) {
        user_content.push_str(&format!("\nHint: {hints}\n"));
    }

    // Include previous attempt context for retries
    if let Some(prev) = previous_attempt {
        user_content.push_str("\n--- PREVIOUS ATTEMPT FAILED ---\n");
        if let Some(ref changes) = prev.changes_json {
            user_content.push_str(&format!("Tried: {changes}\n"));
        }
        if let Some(ref stderr) = prev.build_stderr {
            user_content.push_str("Result:\n");
            user_content.push_str(&truncate_log(stderr, 600));
            user_content.push('\n');
        }
        user_content.push_str("Try a different approach.\n");
    }

    vec![
        ChatMessage::system(SYSTEM_PROMPT),
        ChatMessage::user(user_content),
    ]
}

/// Whether this error type is potentially fixable by an LLM.
///
/// Non-fixable types (network, git, infrastructure, hash mismatch) are
/// skipped immediately without calling the LLM.
pub fn is_llm_fixable(error_type: &str) -> bool {
    matches!(error_type, "build_error" | "test_error")
}

// ── Error-type-specific prompt builders ──────────────────────────────────────

fn build_obsolete_patch_prompt(out: &mut String, error_context: &Value, nix_content: &str) {
    let patch_file = error_context["details"]
        .get("suspected_cause")
        .and_then(|v| v.get("patch_file"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    out.push_str(&format!(
        "Error: Build failed because patch \"{patch_file}\" is reversed/already applied.\n"
    ));
    out.push_str("Action: Remove this patch from the patches list.\n\n");

    let patches_section = extract_section(nix_content, "patches", 15);
    out.push_str("Relevant Nix code:\n```nix\n");
    out.push_str(&patches_section);
    out.push_str("\n```\n");
}

fn build_missing_dep_prompt(
    out: &mut String,
    error_context: &Value,
    nix_content: &str,
    build_log: Option<&str>,
) {
    let package = error_context["details"]
        .get("suspected_cause")
        .and_then(|v| v.get("package"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    out.push_str(&format!(
        "Error: Build failed because dependency \"{package}\" is missing.\n"
    ));
    out.push_str("Action: Add the missing dependency to buildInputs or nativeBuildInputs.\n\n");

    let inputs_section = extract_section(nix_content, "buildInputs", 20);
    out.push_str("Relevant Nix code:\n```nix\n");
    out.push_str(&inputs_section);
    out.push_str("\n```\n");

    if let Some(log) = build_log {
        out.push_str("\nBuild error (last lines):\n```\n");
        out.push_str(&truncate_log(log, 400));
        out.push_str("\n```\n");
    }
}

fn build_test_error_prompt(
    out: &mut String,
    _error_context: &Value,
    nix_content: &str,
    build_log: Option<&str>,
) {
    out.push_str("Error: Package tests failed after version update.\n");
    out.push_str(
        "Action: Fix the test configuration (e.g., disable broken tests, update test expectations, \
         add test dependencies).\n\n",
    );

    // Show test-related sections
    let test_section = extract_section(nix_content, "checkPhase", 15);
    if !test_section.is_empty() {
        out.push_str("Test configuration:\n```nix\n");
        out.push_str(&test_section);
        out.push_str("\n```\n");
    }

    let disable_section = extract_section(nix_content, "disabledTests", 10);
    if !disable_section.is_empty() {
        out.push_str("\nDisabled tests:\n```nix\n");
        out.push_str(&disable_section);
        out.push_str("\n```\n");
    }

    if let Some(log) = build_log {
        out.push_str("\nTest output (last lines):\n```\n");
        out.push_str(&truncate_log(log, 600));
        out.push_str("\n```\n");
    }
}

fn build_generic_build_prompt(
    out: &mut String,
    _error_context: &Value,
    nix_content: &str,
    build_log: Option<&str>,
) {
    out.push_str("Error: Package build failed after version update.\n");
    out.push_str("Action: Fix the Nix expression so the package builds successfully.\n\n");

    // Show a relevant portion of the Nix file (first 50 lines or around buildPhase)
    let relevant = extract_section(nix_content, "buildPhase", 30);
    let section = if relevant.is_empty() {
        // Fall back to showing the beginning of the file
        nix_content
            .lines()
            .take(50)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        relevant
    };

    out.push_str("Nix expression:\n```nix\n");
    out.push_str(&section);
    out.push_str("\n```\n");

    if let Some(log) = build_log {
        out.push_str("\nBuild error (last lines):\n```\n");
        out.push_str(&truncate_log(log, 800));
        out.push_str("\n```\n");
    }
}

// ── Toolchain hints ──────────────────────────────────────────────────────────

/// Return a short hint about the build system based on error log content.
fn toolchain_hints(stderr: &str) -> Option<&'static str> {
    let lower = stderr.to_lowercase();
    if lower.contains("cmake") || lower.contains("cmakelists") {
        Some("CMake package: cmake goes in nativeBuildInputs, libraries in buildInputs.")
    } else if lower.contains("cargo") || lower.contains("crate") {
        Some("Rust/Cargo package: build tools in nativeBuildInputs, cargoHash may need updating.")
    } else if lower.contains("meson") {
        Some("Meson package: meson/ninja in nativeBuildInputs, use mesonFlags to configure.")
    } else if lower.contains("setup.py") || lower.contains("setuptools") || lower.contains("pip") {
        Some("Python package: dependencies go in propagatedBuildInputs, build deps in nativeBuildInputs.")
    } else if lower.contains("npm") || lower.contains("node_modules") {
        Some("Node.js package: npmDepsHash may need updating.")
    } else if lower.contains("go build") || lower.contains("vendor") {
        Some("Go package: vendorHash may need updating, deps in buildInputs.")
    } else {
        None
    }
}

// ── Utility functions ────────────────────────────────────────────────────────

/// Extract lines around a keyword in Nix source code.
///
/// Returns up to `context_lines` lines before and after the first occurrence
/// of `keyword`.
fn extract_section(content: &str, keyword: &str, context_lines: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let Some(idx) = lines.iter().position(|l| l.contains(keyword)) else {
        return String::new();
    };

    let start = idx.saturating_sub(context_lines / 2);
    let end = (idx + context_lines).min(lines.len());

    lines[start..end].join("\n")
}

/// Truncate a log to the last `max_chars` characters, splitting at newlines.
fn truncate_log(log: &str, max_chars: usize) -> String {
    if log.len() <= max_chars {
        return log.to_owned();
    }

    let tail = &log[log.len() - max_chars..];
    // Find the first newline to avoid cutting mid-line
    match tail.find('\n') {
        Some(idx) => format!("...{}", &tail[idx..]),
        None => format!("...{tail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_llm_fixable() {
        assert!(is_llm_fixable("build_error"));
        assert!(is_llm_fixable("test_error"));
        assert!(!is_llm_fixable("network_error"));
        assert!(!is_llm_fixable("hash_mismatch_error"));
        assert!(!is_llm_fixable("git_error"));
        assert!(!is_llm_fixable("infrastructure_error"));
        assert!(!is_llm_fixable("version_constraint_error"));
        assert!(!is_llm_fixable("metadata_error"));
    }

    #[test]
    fn test_truncate_log_short() {
        let log = "line1\nline2\nline3";
        assert_eq!(truncate_log(log, 100), log);
    }

    #[test]
    fn test_truncate_log_long() {
        let log = "aaaa\nbbbb\ncccc\ndddd\neeee";
        let result = truncate_log(log, 10);
        assert!(result.starts_with("..."));
        assert!(result.len() <= 14); // "..." + at most 10 chars + newline adjustment
    }

    #[test]
    fn test_extract_section_found() {
        let content = "line1\nline2\nbuildInputs = [\n  foo\n];\nline6\nline7";
        let section = extract_section(content, "buildInputs", 4);
        assert!(section.contains("buildInputs"));
    }

    #[test]
    fn test_extract_section_not_found() {
        let content = "line1\nline2\nline3";
        let section = extract_section(content, "nonexistent", 4);
        assert!(section.is_empty());
    }

    #[test]
    fn test_toolchain_hints() {
        assert!(toolchain_hints("CMake Error at CMakeLists.txt").is_some());
        assert!(toolchain_hints("cargo build failed").is_some());
        assert!(toolchain_hints("some random error").is_none());
    }
}
