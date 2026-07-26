//! Claude Code CLI subagent for automated build fix attempts.
//!
//! When a package update fails to build, this module can invoke the `claude`
//! CLI in the worktree to attempt an automated fix. Claude gets full context
//! (build error, diff, Nix file content) and uses its agentic tools (Read,
//! Edit, Grep, etc.) to modify files. The fix is then validated by running
//! `nix-build` before accepting it.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::commands::update::build_nix_expr;

/// Configuration for Claude Code fix attempts.
#[derive(Debug, Clone)]
pub struct ClaudeFixConfig {
    /// Maximum number of agent turns Claude may take.
    pub max_turns: u32,
    /// Timeout in seconds for the entire fix attempt.
    pub timeout_secs: u64,
}

/// Outcome of a Claude Code fix attempt.
#[derive(Debug)]
pub enum ClaudeFixResult {
    /// Claude's edits resolved the build failure.
    Fixed,
    /// Claude attempted a fix but the build still fails.
    StillBroken { stderr: String },
    /// The `claude` CLI binary was not found on PATH.
    ClaudeNotAvailable,
    /// The fix attempt exceeded the configured timeout.
    TimedOut,
    /// The `claude` process exited with a non-zero status or failed to start.
    InvocationError { message: String },
}

/// Attempt to fix a failed package build using Claude Code CLI.
///
/// Invokes `claude` in the worktree directory with a detailed prompt
/// describing the build failure. After Claude exits, validates by
/// re-running `nix-build`.
#[allow(clippy::too_many_arguments)]
pub async fn attempt_claude_fix(
    worktree_path: &Path,
    worktree_entry_point: &str,
    attr_path: &str,
    error_message: &str,
    build_log: Option<&str>,
    nix_file_relative_path: &str,
    nix_file_content: &str,
    current_version: &str,
    new_version: &str,
    config: &ClaudeFixConfig,
) -> ClaudeFixResult {
    // Check that `claude` is available on PATH
    match Command::new("claude").arg("--version").output().await {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            debug!("{}: claude CLI found: {}", attr_path, version.trim());
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!(
                "{}: claude --version exited non-zero: {}",
                attr_path, stderr
            );
            return ClaudeFixResult::ClaudeNotAvailable;
        },
        Err(e) => {
            debug!("{}: claude CLI not found on PATH: {}", attr_path, e);
            return ClaudeFixResult::ClaudeNotAvailable;
        },
    }

    // Generate the diff of changes made so far in the worktree
    let diff = match generate_diff(worktree_path).await {
        Ok(d) => d,
        Err(e) => {
            warn!(
                "{}: Failed to generate diff for Claude prompt: {}",
                attr_path, e
            );
            String::from("(diff unavailable)")
        },
    };

    // Build the prompt
    let prompt = build_claude_prompt(
        attr_path,
        current_version,
        new_version,
        nix_file_relative_path,
        nix_file_content,
        &diff,
        error_message,
        build_log,
        worktree_entry_point,
    );

    info!(
        "{}: Invoking Claude Code CLI to attempt build fix (max_turns={}, timeout={}s)",
        attr_path, config.max_turns, config.timeout_secs
    );

    // Spawn claude CLI
    let max_turns_str = config.max_turns.to_string();
    let mut child = match Command::new("claude")
        .args([
            "--print",
            "--dangerously-skip-permissions",
            "--max-turns",
            &max_turns_str,
            "--allowedTools",
            "Read,Edit,Write,Bash,Grep,Glob",
        ])
        .current_dir(worktree_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return ClaudeFixResult::InvocationError {
                message: format!("Failed to spawn claude process: {e}"),
            };
        },
    };

    // Write prompt to stdin
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(prompt.as_bytes()).await {
            return ClaudeFixResult::InvocationError {
                message: format!("Failed to write prompt to claude stdin: {e}"),
            };
        }
        drop(stdin); // Close stdin so claude can proceed
    }

    // Wait with timeout
    let timeout = Duration::from_secs(config.timeout_secs);
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return ClaudeFixResult::InvocationError {
                message: format!("Failed to wait for claude process: {e}"),
            };
        },
        Err(_) => {
            warn!(
                "{}: Claude Code fix attempt timed out after {}s",
                attr_path, config.timeout_secs
            );
            // Kill the timed-out process
            // child has been consumed by wait_with_output, but the timeout
            // means it hasn't completed — the drop will handle cleanup
            return ClaudeFixResult::TimedOut;
        },
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        warn!(
            "{}: Claude exited with status {}: {}",
            attr_path,
            output.status,
            if stderr.is_empty() { &stdout } else { &stderr }
        );
        return ClaudeFixResult::InvocationError {
            message: format!("claude exited with status {}", output.status),
        };
    }

    let claude_output = String::from_utf8_lossy(&output.stdout);
    debug!(
        "{}: Claude response ({} chars): {}",
        attr_path,
        claude_output.len(),
        &claude_output[..claude_output.len().min(500)]
    );

    // Verify Claude actually changed files before running a (potentially expensive)
    // validation build. Without this check, a no-op Claude run would "succeed"
    // because the old code still builds fine.
    let has_changes = has_uncommitted_changes(worktree_path).await;
    if !has_changes {
        info!(
            "{}: Claude made no file changes — skipping validation",
            attr_path
        );
        return ClaudeFixResult::StillBroken {
            stderr: "Claude made no file changes".to_owned(),
        };
    }

    // Validate: re-run nix-build to check if Claude's edits fixed the build
    info!("{}: Validating Claude's fix with nix-build", attr_path);

    match build_nix_expr(worktree_entry_point, attr_path, None).await {
        Ok((true, _stdout, _stderr)) => {
            info!("{}: Claude's fix validated — build succeeded!", attr_path);
            ClaudeFixResult::Fixed
        },
        Ok((false, _stdout, stderr)) => {
            warn!(
                "{}: Claude's fix did not resolve the build failure",
                attr_path
            );
            ClaudeFixResult::StillBroken { stderr }
        },
        Err(e) => {
            warn!(
                "{}: Failed to run validation build after Claude fix: {}",
                attr_path, e
            );
            ClaudeFixResult::StillBroken {
                stderr: format!("{e:#}"),
            }
        },
    }
}

/// Build the prompt for Claude Code CLI.
///
/// Provides full context about the failed build: package identity, the diff
/// of changes already made, the build error, and the complete Nix file content.
#[allow(clippy::too_many_arguments)]
fn build_claude_prompt(
    attr_path: &str,
    current_version: &str,
    new_version: &str,
    nix_file_relative_path: &str,
    nix_file_content: &str,
    diff: &str,
    error_message: &str,
    build_log: Option<&str>,
    worktree_entry_point: &str,
) -> String {
    let mut prompt = String::with_capacity(16_000);

    prompt.push_str("# Task: Fix Nix Package Build Failure\n\n");

    prompt.push_str("## Package\n\n");
    prompt.push_str(&format!("- **Attribute path**: `{attr_path}`\n"));
    prompt.push_str(&format!(
        "- **Version update**: {current_version} -> {new_version}\n"
    ));
    prompt.push_str(&format!("- **Nix file**: `{nix_file_relative_path}`\n"));
    prompt.push_str(&format!(
        "- **Eval entry point**: `{worktree_entry_point}`\n"
    ));
    prompt.push_str(&format!(
        "- **Build command**: `nix-build {worktree_entry_point} -A {attr_path}`\n\n"
    ));

    prompt.push_str("## What happened\n\n");
    prompt.push_str(
        "The package version and source hash were updated automatically, but the build failed. \
         Your job is to fix the Nix expression so the package builds successfully.\n\n",
    );

    prompt.push_str("## Changes already made (git diff)\n\n");
    prompt.push_str("```diff\n");
    if diff.len() > 10_000 {
        prompt.push_str(&diff[..10_000]);
        prompt.push_str("\n... (truncated)\n");
    } else {
        prompt.push_str(diff);
    }
    prompt.push_str("\n```\n\n");

    prompt.push_str("## Error message\n\n");
    prompt.push_str("```\n");
    let error_tail = truncate_tail(error_message, 5_000);
    prompt.push_str(&error_tail);
    prompt.push_str("\n```\n\n");

    if let Some(log) = build_log {
        prompt.push_str("## Build log (last 10KB)\n\n");
        prompt.push_str("```\n");
        let log_tail = truncate_tail(log, 10_000);
        prompt.push_str(&log_tail);
        prompt.push_str("\n```\n\n");
    }

    prompt.push_str("## Current Nix file content\n\n");
    prompt.push_str("```nix\n");
    prompt.push_str(nix_file_content);
    prompt.push_str("\n```\n\n");

    prompt.push_str("## Instructions\n\n");
    prompt.push_str("1. Read the build error carefully to understand the failure.\n");
    prompt.push_str(
        "2. Edit the Nix file(s) in this directory to fix the issue. Common fixes include:\n",
    );
    prompt.push_str("   - Removing obsolete patches that no longer apply\n");
    prompt.push_str("   - Adding missing dependencies to `buildInputs` or `nativeBuildInputs`\n");
    prompt.push_str("   - Updating test configurations (`disabledTests`, `disabledTestPaths`)\n");
    prompt.push_str("   - Fixing phase overrides (`postPatch`, `postInstall`, etc.)\n");
    prompt.push_str("3. Only modify `.nix` files in this directory.\n");
    prompt.push_str("4. Do **NOT** run `nix-build` — the harness will validate your changes.\n");
    prompt.push_str("5. Do **NOT** create git commits.\n");

    prompt
}

/// Check whether the error message indicates a failure that Claude could
/// plausibly fix by editing Nix files. Hash-extraction errors, metadata
/// errors, and version-attribute-not-found errors are not fixable this way.
pub fn should_attempt_claude_fix(error_message: &str) -> bool {
    let dominated = [
        "Could not extract correct",
        "Attribute 'version' not found",
        "Could not extract correct hash",
        "Could not extract correct cargoHash",
        "Could not extract correct vendorHash",
        "Could not extract correct npmDepsHash",
        "Source is not from a supported VCS platform",
        "No source URL or pname found",
        "Empty position returned from meta.position",
    ];
    !dominated.iter().any(|pat| error_message.contains(pat))
}

/// Check if a directory has uncommitted changes (staged or unstaged).
async fn has_uncommitted_changes(dir: &Path) -> bool {
    let output = Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "status", "--porcelain"])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            !stdout.trim().is_empty()
        },
        _ => false,
    }
}

/// Generate a git diff of uncommitted changes in the worktree.
async fn generate_diff(worktree_path: &Path) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["-C", &worktree_path.to_string_lossy(), "diff", "HEAD"])
        .output()
        .await?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        anyhow::bail!(
            "Failed to generate diff: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

/// Return the last `max_chars` of a string, splitting at a newline boundary.
fn truncate_tail(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_owned();
    }
    let tail = &text[text.len() - max_chars..];
    match tail.find('\n') {
        Some(idx) => format!("...\n{}", &tail[idx + 1..]),
        None => format!("...{tail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_claude_prompt_contains_required_sections() {
        let prompt = build_claude_prompt(
            "hello",
            "1.0",
            "2.0",
            "pkgs/hello/default.nix",
            "{ stdenv }: stdenv.mkDerivation { pname = \"hello\"; }",
            "diff --git a/pkgs/hello/default.nix b/pkgs/hello/default.nix\n-version = \
             \"1.0\";\n+version = \"2.0\";",
            "build failed: missing dependency libfoo",
            Some("error: builder for '/nix/store/...' failed"),
            "./default.nix",
        );

        assert!(prompt.contains("hello"));
        assert!(prompt.contains("1.0"));
        assert!(prompt.contains("2.0"));
        assert!(prompt.contains("pkgs/hello/default.nix"));
        assert!(prompt.contains("missing dependency libfoo"));
        assert!(prompt.contains("builder for"));
        assert!(prompt.contains("stdenv.mkDerivation"));
        assert!(prompt.contains("Do **NOT** run `nix-build`"));
        assert!(prompt.contains("Do **NOT** create git commits"));
    }

    #[test]
    fn test_truncate_tail_short() {
        let text = "line1\nline2\nline3";
        assert_eq!(truncate_tail(text, 100), text);
    }

    #[test]
    fn test_truncate_tail_long() {
        let text = "aaaa\nbbbb\ncccc\ndddd\neeee";
        let result = truncate_tail(text, 10);
        assert!(result.starts_with("..."));
        assert!(result.contains("eeee"));
    }

    #[test]
    fn test_should_attempt_claude_fix() {
        // Hash extraction errors are not fixable
        assert!(!should_attempt_claude_fix(
            "Could not extract correct cargoHash from build error:"
        ));
        assert!(!should_attempt_claude_fix(
            "Could not extract correct hash from build error:"
        ));

        // Attribute not found is not fixable
        assert!(!should_attempt_claude_fix("Attribute 'version' not found"));

        // Real build errors are fixable
        assert!(should_attempt_claude_fix(
            "Package build failed after update with no reversed patches detected."
        ));
        assert!(should_attempt_claude_fix(
            "builder for '/nix/store/...' failed"
        ));
    }
}
