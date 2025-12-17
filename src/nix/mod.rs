pub mod nix_eval_jobs;
pub mod run_eval;

use tokio::process::Command;
use tracing::debug;

/// Evaluate a Nix expression and return the result as a string
///
/// Executes `nix-instantiate --eval -E <expr> --raw` to evaluate arbitrary Nix expressions
/// and returns the result as a trimmed, unquoted string.
///
/// # Arguments
/// * `expr` - The Nix expression to evaluate
///
/// # Returns
/// The evaluated result as a string, with whitespace trimmed and quotes removed
///
/// # Errors
/// Returns an error if:
/// - The nix-instantiate command fails to execute
/// - The evaluation fails (invalid Nix syntax, undefined variables, etc.)
/// - The output cannot be decoded as UTF-8
///
/// # Example
/// ```no_run
/// # use ekapkgs_update::nix::eval_nix_expr;
/// # async fn example() -> anyhow::Result<()> {
/// let version = eval_nix_expr("with import ./. {}; pkgs.hello.version").await?;
/// println!("Hello version: {}", version);
/// # Ok(())
/// # }
/// ```
pub async fn eval_nix_expr(expr: impl AsRef<str>) -> anyhow::Result<String> {
    let expr = expr.as_ref();

    let output = Command::new("nix-instantiate")
        .arg("--eval")
        .arg("-E")
        .arg(expr)
        .arg("--raw")
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("nix-instantiate evaluation failed: {}", stderr.trim());
    }

    let result = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches('"')
        .to_string();

    Ok(result)
}

/// Check if a package uses mkManyVariants pattern by evaluating '<pkg> ? variants'
pub async fn is_many_variants_package(
    eval_entry_point: &str,
    attr_path: &str,
) -> anyhow::Result<bool> {
    let check_expr = format!(
        "with import ./{} {{ }}; {} ? variants",
        eval_entry_point, attr_path
    );

    match eval_nix_expr(&check_expr).await {
        Ok(result) => {
            let is_many_variants = result.trim() == "true";
            if is_many_variants {
                debug!("{} is a mkManyVariants package", attr_path);
            }
            Ok(is_many_variants)
        },
        Err(e) => {
            debug!("Failed to check if {} is mkManyVariants: {}", attr_path, e);
            Ok(false)
        },
    }
}
