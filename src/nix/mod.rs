pub mod nix_eval_jobs;
pub mod run_eval;

use tokio::process::Command;

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
        return Err(anyhow::anyhow!(
            "nix-instantiate evaluation failed: {}",
            stderr.trim()
        ));
    }

    let result = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches('"')
        .to_string();

    Ok(result)
}
