//! Fix application and build validation for the autofix pipeline.
//!
//! Applies LLM-proposed JSON changes to a preserved worktree, then validates
//! the result by running `nix-instantiate` (syntax check) and `nix-build`
//! (full build). Reverts changes on any failure.

use std::path::Path;

use anyhow::Context;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::commands::apply::apply_json_operations;

/// Result of applying and validating a fix.
#[derive(Debug)]
pub enum ValidationResult {
    /// The fix was applied and the package built successfully.
    BuildSuccess,
    /// The fix was applied but the build failed.
    BuildFailed { stderr: String },
    /// The JSON changes could not be applied to the worktree.
    ApplyFailed { error: String },
    /// Nix evaluation (syntax check) failed after applying changes.
    EvalFailed { stderr: String },
}

/// Quote `s` as a Nix string literal (`"…"`), escaping `\`, `"` and `${`.
pub(crate) fn nix_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Apply LLM-proposed changes to a worktree and validate via build.
///
/// `entry_point` is the evaluation entry point *inside the worktree* (an
/// absolute path, see `processor::worktree_entry_point`), so the fixed tree
/// is the one evaluated and built, never the main checkout.
///
/// On any failure, the worktree is reverted to its pre-change state via
/// `git checkout -- .`.
pub async fn apply_and_validate(
    worktree_path: &Path,
    attr_path: &str,
    changes: &serde_json::Value,
    entry_point: &Path,
) -> anyhow::Result<ValidationResult> {
    info!("{}: Applying LLM fix to worktree", attr_path);

    // Apply the JSON operations
    if let Err(e) = apply_json_operations(changes, worktree_path).await {
        warn!("{}: Failed to apply changes: {}", attr_path, e);
        revert_worktree(worktree_path).await;
        return Ok(ValidationResult::ApplyFailed {
            error: format!("{e:#}"),
        });
    }

    debug!("{}: Changes applied, running syntax check", attr_path);

    // Syntax check via nix-instantiate
    let eval_result = Command::new("nix-instantiate")
        .args(["--eval", "--strict", "-E"])
        .arg(format!(
            "let pkgs = import {} {{ }}; in pkgs.{attr_path}",
            nix_string_literal(&entry_point.to_string_lossy())
        ))
        .current_dir(worktree_path)
        .output()
        .await
        .context("Failed to run nix-instantiate")?;

    if !eval_result.status.success() {
        let stderr = String::from_utf8_lossy(&eval_result.stderr).to_string();
        warn!("{}: Syntax check failed: {}", attr_path, stderr);
        revert_worktree(worktree_path).await;
        return Ok(ValidationResult::EvalFailed { stderr });
    }

    debug!("{}: Syntax check passed, building package", attr_path);

    // Full build validation
    let mut build_cmd = Command::new("nix-build");
    build_cmd
        .arg(entry_point)
        .args(["-A", attr_path])
        .current_dir(worktree_path);

    // Apply any extra nix-build args (--builders, --max-jobs)
    for arg in crate::commands::update::get_extra_nix_build_args() {
        build_cmd.arg(arg);
    }

    let build_result = build_cmd
        .output()
        .await
        .context("Failed to run nix-build")?;

    if build_result.status.success() {
        info!("{}: Build succeeded with LLM fix", attr_path);
        Ok(ValidationResult::BuildSuccess)
    } else {
        let stderr = String::from_utf8_lossy(&build_result.stderr).to_string();
        warn!("{}: Build failed after applying fix", attr_path);
        revert_worktree(worktree_path).await;
        Ok(ValidationResult::BuildFailed { stderr })
    }
}

/// Revert all changes in the worktree.
async fn revert_worktree(worktree_path: &Path) {
    let result = Command::new("git")
        .args([
            "-C",
            &worktree_path.to_string_lossy(),
            "checkout",
            "--",
            ".",
        ])
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            debug!("Reverted worktree: {}", worktree_path.display());
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(
                "Failed to revert worktree {}: {}",
                worktree_path.display(),
                stderr
            );
        },
        Err(e) => {
            warn!(
                "Failed to run git checkout in {}: {}",
                worktree_path.display(),
                e
            );
        },
    }
}

#[cfg(test)]
mod tests {
    use super::nix_string_literal;

    #[test]
    fn nix_string_literal_escapes_special_characters() {
        let cases = [
            ("/tmp/wt/default.nix", r#""/tmp/wt/default.nix""#),
            (r#"/a "b"/c"#, r#""/a \"b\"/c""#),
            (r"/a\b", r#""/a\\b""#),
            ("/a/${x}/$y", r#""/a/\${x}/$y""#),
        ];
        for (input, expected) in cases {
            assert_eq!(nix_string_literal(input), expected, "{input}");
        }
    }
}
