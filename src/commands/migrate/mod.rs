mod arguments;
mod final_attrs;
mod transforms;

use anyhow::Context;
pub use arguments::add_run_unit_tests_argument;
pub use final_attrs::{convert_to_final_attrs_pattern, fix_closing_brace};
use tracing::{debug, info};
pub use transforms::{add_unittests_to_passthru, ensure_do_check_false, update_test_comments};

/// Migrate a Nix package file to use runUnitTests pattern
///
/// This command performs the following transformations:
/// 1. Add runUnitTests to function arguments
/// 2. Convert stdenv.mkDerivation to use finalAttrs pattern
/// 3. Set doCheck = false
/// 4. Add unittests to passthru.tests
/// 5. Update closing braces
/// 6. Update test-related comments
pub async fn migrate(file: String, target: String) -> anyhow::Result<()> {
    info!("Starting migration for target: {}", target);

    // Determine if target is an attr path or file path
    let file_path = if std::path::Path::new(&target).exists() {
        // Target is a file path
        info!("Target is a file path: {}", target);
        target.clone()
    } else {
        // Target is an attr path - resolve it to a file
        info!("Target is an attribute path: {}", target);
        let normalized_entry = crate::nix::normalize_entry_point(&file);
        let position_expr = format!("with import {normalized_entry} {{ }}; {target}.meta.position");

        let position = crate::nix::eval_nix_expr(&position_expr)
            .await
            .context("Failed to resolve attribute path to file location")?;

        let (file_path, _line) = position
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {position}"))?;

        info!("Resolved to file: {}", file_path);
        file_path.to_owned()
    };

    // Read the file
    let content = tokio::fs::read_to_string(&file_path)
        .await
        .context("Failed to read Nix file")?;

    // Apply migrations
    let migrated_content = apply_run_unit_tests_migration(&content)?;

    // Validate the result parses correctly
    let parse = rnix::Root::parse(&migrated_content);
    if !parse.errors().is_empty() {
        let errors: Vec<String> = parse
            .errors()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        anyhow::bail!(
            "Migration would create invalid Nix syntax: {}",
            errors.join(", ")
        );
    }

    // Write back the migrated file
    tokio::fs::write(&file_path, migrated_content)
        .await
        .context("Failed to write migrated file")?;

    info!("✓ Successfully migrated {}", file_path);
    println!("Migrated: {file_path}");

    Ok(())
}

/// Apply the runUnitTests migration to a Nix file
///
/// Transformations:
/// 1. Add runUnitTests to function arguments
/// 2. Convert stdenv.mkDerivation rec { to stdenv.mkDerivation (finalAttrs: rec {
/// 3. Set doCheck = false;
/// 4. Add unittests = runUnitTests finalAttrs.finalPackage; to passthru.tests
/// 5. Update closing brace from } to })
/// 6. Update test-related comments
pub(crate) fn apply_run_unit_tests_migration(content: &str) -> anyhow::Result<String> {
    // First, validate that the file parses correctly
    let parse = rnix::Root::parse(content);
    if !parse.errors().is_empty() {
        let errors: Vec<String> = parse
            .errors()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        anyhow::bail!("Failed to parse Nix file: {}", errors.join(", "));
    }

    let mut result = content.to_owned();

    // Step 1: Add runUnitTests to function arguments if not already present
    if !result.contains("runUnitTests") {
        result = add_run_unit_tests_argument(&result)?;
        debug!("Added runUnitTests to function arguments");
    }

    // Step 2: Convert stdenv.mkDerivation rec { to stdenv.mkDerivation (finalAttrs: rec {
    if !result.contains("finalAttrs:") {
        result = convert_to_final_attrs_pattern(&result)?;
        debug!("Converted to finalAttrs pattern");
    }

    // Step 3: Update or add doCheck = false; if it doesn't exist or is true
    result = ensure_do_check_false(&result)?;
    debug!("Ensured doCheck = false");

    // Step 4: Add unittests to passthru.tests
    result = add_unittests_to_passthru(&result)?;
    debug!("Added unittests to passthru.tests");

    // Step 5: Update closing brace
    result = fix_closing_brace(&result)?;
    debug!("Fixed closing brace");

    // Step 6: Update test-related comments
    result = update_test_comments(&result);
    debug!("Updated test comments");

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_run_unit_tests_argument() {
        let content = r#"{
  lib,
  stdenv,
  fetchurl,
  gperf,
  nix-update-script,
  python3Packages,
}:

stdenv.mkDerivation rec {
  pname = "libseccomp";
"#;

        let result = add_run_unit_tests_argument(content);
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.contains("runUnitTests,"));
        assert!(updated.contains("python3Packages,"));
    }

    #[test]
    fn test_convert_to_final_attrs_pattern() {
        let content = "stdenv.mkDerivation rec {\n  pname = \"test\";";
        let result = convert_to_final_attrs_pattern(content).unwrap();
        assert!(result.contains("stdenv.mkDerivation (finalAttrs: rec {"));

        let content_no_rec = "stdenv.mkDerivation {\n  pname = \"test\";";
        let result_no_rec = convert_to_final_attrs_pattern(content_no_rec).unwrap();
        assert!(result_no_rec.contains("stdenv.mkDerivation (finalAttrs: {"));
    }

    #[test]
    fn test_ensure_do_check_false_existing_true() {
        let content = r#"  doCheck = true;

  passthru = {"#;

        let result = ensure_do_check_false(content).unwrap();
        assert!(result.contains("doCheck = false;"));
        assert!(!result.contains("doCheck = true;"));
    }

    #[test]
    fn test_ensure_do_check_false_already_false() {
        let content = r#"  doCheck = false;

  passthru = {"#;

        let result = ensure_do_check_false(content).unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn test_fix_closing_brace() {
        let content = r#"{
  pname = "test";

  meta = {
    description = "Test";
  };
}"#;

        let result = fix_closing_brace(content).unwrap();
        assert!(result.ends_with("})\n"));
    }
}
