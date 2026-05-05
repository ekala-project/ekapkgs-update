pub mod eval_cache;
pub mod flake;
pub mod nix_eval_jobs;
pub mod rebuild_count;
pub mod run_eval;

use std::borrow::Cow;

use tokio::process::Command;
use tracing::debug;

/// Normalize a Nix entry point path by prepending `./` if needed
///
/// Ensures that relative paths are properly prefixed with `./` for use in Nix
/// import expressions, while leaving absolute paths and already-prefixed paths unchanged.
///
/// # Arguments
/// * `entry_point` - The entry point path to normalize
///
/// # Returns
/// A normalized path suitable for use in `import` expressions
///
/// # Examples
/// ```
/// # use ekapkgs_update::nix::normalize_entry_point;
/// assert_eq!(normalize_entry_point("default.nix"), "./default.nix");
/// assert_eq!(normalize_entry_point("./default.nix"), "./default.nix");
/// assert_eq!(
///     normalize_entry_point("/absolute/path.nix"),
///     "/absolute/path.nix"
/// );
/// ```
pub fn normalize_entry_point(entry_point: &str) -> Cow<'_, str> {
    if entry_point.starts_with("./") || entry_point.starts_with('/') {
        Cow::Borrowed(entry_point)
    } else {
        Cow::Owned(format!("./{entry_point}"))
    }
}

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
        .to_owned();

    Ok(result)
}

/// Check if a package uses mkManyVariants pattern by evaluating '<pkg> ? variants'
pub async fn is_many_variants_package(
    eval_entry_point: &str,
    attr_path: &str,
) -> anyhow::Result<bool> {
    let normalized_entry = normalize_entry_point(eval_entry_point);
    let check_expr =
        format!("with import {normalized_entry} {{ }}; toString ({attr_path} ? variants)");

    match eval_nix_expr(&check_expr).await {
        Ok(result) => {
            let is_many_variants = result.trim() == "1";
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

/// Get list of all variant names for a mkManyVariants package
///
/// Evaluates `builtins.attrNames pkg.variants` to get all variant attribute names.
///
/// # Arguments
/// * `eval_entry_point` - Path to the Nix file to import (e.g., "default.nix")
/// * `attr_path` - The package attribute path (e.g., "pkgs.isl")
///
/// # Returns
/// A vector of variant names (e.g., ["v0_20", "v0_23", "v0_27"])
///
/// # Errors
/// Returns an error if the package is not mkManyVariants or evaluation fails
pub async fn get_variants_list(
    eval_entry_point: &str,
    attr_path: &str,
) -> anyhow::Result<Vec<String>> {
    let normalized_entry = normalize_entry_point(eval_entry_point);
    let expr = format!(
        "with import {normalized_entry} {{ }}; builtins.toJSON (builtins.attrNames \
         {attr_path}.variants)"
    );

    let output = eval_nix_expr(&expr).await?;
    let variants: Vec<String> = serde_json::from_str(&output)?;

    debug!("{} has variants: {:?}", attr_path, variants);
    Ok(variants)
}

/// Get version for a specific variant
///
/// # Arguments
/// * `eval_entry_point` - Path to the Nix file to import
/// * `attr_path` - The package attribute path
/// * `variant_name` - The variant to query (e.g., "v0_20")
///
/// # Returns
/// The version string for the specified variant
pub async fn get_variant_version(
    eval_entry_point: &str,
    attr_path: &str,
    variant_name: &str,
) -> anyhow::Result<String> {
    let normalized_entry = normalize_entry_point(eval_entry_point);
    let expr = format!(
        "with import {normalized_entry} {{ }}; {attr_path}.variants.{variant_name}.version"
    );

    eval_nix_expr(&expr).await
}

/// Check if a package has a specific attribute
///
/// Evaluates a Nix expression using the `?` operator to check if an attribute exists
/// on a package. This works for both simple attributes (e.g., "version") and nested
/// attributes (e.g., "passthru.tests").
///
/// # Arguments
/// * `eval_entry_point` - Path to the Nix file to import (e.g., "default.nix")
/// * `attr_path` - The package attribute path (e.g., "pkgs.hello")
/// * `attribute_name` - The attribute to check for (e.g., "version" or "passthru.tests")
///
/// # Returns
/// * `Ok(true)` - The attribute exists
/// * `Ok(false)` - The attribute doesn't exist or evaluation failed
///
/// # Errors
/// This function is designed to be forgiving - evaluation errors are logged and
/// result in `Ok(false)` rather than propagating the error. This makes it suitable
/// for checking optional attributes.
///
/// # Examples
/// ```no_run
/// # use ekapkgs_update::nix::has_attr;
/// # async fn example() -> anyhow::Result<()> {
/// // Check for a simple attribute
/// let has_version = has_attr("default.nix", "pkgs.hello", "version").await?;
///
/// // Check for a nested attribute
/// let has_tests = has_attr("default.nix", "pkgs.mypackage", "passthru.tests").await?;
/// # Ok(())
/// # }
/// ```
pub async fn has_attr(
    eval_entry_point: &str,
    attr_path: &str,
    attribute_name: &str,
) -> anyhow::Result<bool> {
    let normalized_entry = normalize_entry_point(eval_entry_point);
    let check_expr =
        format!("with import {normalized_entry} {{ }}; toString({attr_path} ? {attribute_name})");

    match eval_nix_expr(&check_expr).await {
        Ok(result) => {
            // TODO: we could probably do --json, and get an actual `true` or `false` from the eval
            let has_attribute = result.trim() == "1";
            if has_attribute {
                debug!("{} has attribute '{}'", attr_path, attribute_name);
            }
            Ok(has_attribute)
        },
        Err(e) => {
            debug!(
                "Failed to check if {} has attribute '{}': {}",
                attr_path, attribute_name, e
            );
            Ok(false)
        },
    }
}

pub async fn has_passthru_tests(eval_entry_point: &str, attr_path: &str) -> anyhow::Result<bool> {
    let passthru_attr = format!("{}.{}", attr_path, "passthru");
    has_attr(eval_entry_point, &passthru_attr, "tests").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_entry_point_simple() {
        assert_eq!(normalize_entry_point("default.nix"), "./default.nix");
        assert_eq!(normalize_entry_point("./default.nix"), "./default.nix");
    }

    #[test]
    fn test_normalize_entry_point_nested() {
        assert_eq!(
            normalize_entry_point("/absolute/path.nix"),
            "/absolute/path.nix"
        );
        assert_eq!(
            normalize_entry_point("path/to/default.nix"),
            "./path/to/default.nix"
        );

        assert_eq!(
            normalize_entry_point("./path/to/default.nix"),
            "./path/to/default.nix"
        );
        assert_eq!(
            normalize_entry_point("/absolute/path/to/default.nix"),
            "/absolute/path/to/default.nix"
        );
    }
}
