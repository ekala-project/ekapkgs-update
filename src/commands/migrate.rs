use anyhow::Context;
use regex::Regex;
use tracing::{debug, info};

use crate::nix::{eval_nix_expr, normalize_entry_point};

/// Migrate a package from nixpkgs paradigms to ekapkgs paradigms
///
/// This command transforms Nix package definitions to follow ekapkgs best practices:
/// - Adds runUnitTests to function arguments
/// - Converts stdenv.mkDerivation to use finalAttrs pattern
/// - Moves unit tests to passthru.tests using runUnitTests
/// - Sets doCheck = false when tests are moved to passthru
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
        let normalized_entry = normalize_entry_point(&file);
        let position_expr = format!(
            "with import {} {{ }}; {}.meta.position",
            normalized_entry, target
        );

        let position = eval_nix_expr(&position_expr)
            .await
            .context("Failed to resolve attribute path to file location")?;

        let (file_path, _line) = position
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("Unexpected position format: {}", position))?;

        info!("Resolved to file: {}", file_path);
        file_path.to_string()
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
        let errors: Vec<String> = parse.errors().iter().map(|e| e.to_string()).collect();
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
    println!("Migrated: {}", file_path);

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
        let errors: Vec<String> = parse.errors().iter().map(|e| e.to_string()).collect();
        anyhow::bail!("Failed to parse Nix file: {}", errors.join(", "));
    }

    let mut result = content.to_string();

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

/// Add runUnitTests to the function arguments
fn add_run_unit_tests_argument(content: &str) -> anyhow::Result<String> {
    // Pattern to match function arguments ending with }:
    // We want to add runUnitTests before the closing }:
    let pattern = Regex::new(r"(?s)(.*?)(\n\s*)(\}:)(.*)")?;

    if let Some(caps) = pattern.captures(content) {
        let before = &caps[1];
        let whitespace = &caps[2];
        let closing = &caps[3];
        let after = &caps[4];

        // Find the last argument to determine indentation
        let lines: Vec<&str> = before.lines().collect();
        let last_arg_line = lines
            .iter()
            .rev()
            .find(|line| line.trim().ends_with(','))
            .or_else(|| lines.iter().rev().find(|line| line.contains(',')))
            .unwrap_or(&lines[lines.len() - 1]);

        // Get indentation from last argument
        let indent = last_arg_line.len() - last_arg_line.trim_start().len();
        let indent_str = " ".repeat(indent);

        Ok(format!(
            "{}{}{}runUnitTests,{}{}{}",
            before, whitespace, indent_str, whitespace, closing, after
        ))
    } else {
        anyhow::bail!("Could not find function argument pattern in file")
    }
}

/// Convert stdenv.mkDerivation rec { to stdenv.mkDerivation (finalAttrs: rec {
fn convert_to_final_attrs_pattern(content: &str) -> anyhow::Result<String> {
    // Pattern to match: stdenv.mkDerivation rec {
    // or: stdenv.mkDerivation {
    let pattern = Regex::new(r"(stdenv\.mkDerivation\s+)(rec\s+)?(\{)")?;

    let result = pattern.replace(content, |caps: &regex::Captures| {
        let prefix = &caps[1];
        let has_rec = caps.get(2).is_some();

        if has_rec {
            format!("{}(finalAttrs: rec {{", prefix)
        } else {
            format!("{}(finalAttrs: {{", prefix)
        }
    });

    Ok(result.into_owned())
}

/// Ensure doCheck = false; exists and is set to false
fn ensure_do_check_false(content: &str) -> anyhow::Result<String> {
    // Check if doCheck exists
    let do_check_pattern = Regex::new(r"(?m)^[ \t]*doCheck\s*=\s*([^;]+);")?;

    if let Some(caps) = do_check_pattern.captures(content) {
        let value = caps[1].trim();
        if value == "false" {
            // Already false, no change needed
            return Ok(content.to_string());
        }

        // Change to false
        let result = do_check_pattern.replace(content, |caps: &regex::Captures| {
            let line = &caps[0];
            let indent = line.len() - line.trim_start().len();
            let indent_str = " ".repeat(indent);
            format!("{}doCheck = false;", indent_str)
        });

        Ok(result.into_owned())
    } else {
        // doCheck doesn't exist, we'll add it when we find a good place
        // For now, we'll add it before passthru or meta, or after buildInputs
        add_do_check_false(content)
    }
}

/// Add doCheck = false; to the file
fn add_do_check_false(content: &str) -> anyhow::Result<String> {
    // Try to add before passthru
    let passthru_pattern = Regex::new(r"(?m)(^\s*)(passthru\s*=)")?;
    if let Some(caps) = passthru_pattern.captures(content) {
        let indent = &caps[1];
        let result = passthru_pattern.replace(
            content,
            format!(
                "{}# Test suite is quite long, run as passthru\n{}doCheck = false;\n\n{}passthru =",
                indent, indent, indent
            ),
        );
        return Ok(result.into_owned());
    }

    // Try to add before meta
    let meta_pattern = Regex::new(r"(?m)(^\s*)(meta\s*=)")?;
    if let Some(caps) = meta_pattern.captures(content) {
        let indent = &caps[1];
        let result = meta_pattern.replace(
            content,
            format!(
                "{}# Test suite is quite long, run as passthru\n{}doCheck = false;\n\n{}meta =",
                indent, indent, indent
            ),
        );
        return Ok(result.into_owned());
    }

    // Otherwise, return as is (we'll let the user handle this case)
    debug!("Could not find appropriate place to add doCheck = false;");
    Ok(content.to_string())
}

/// Add unittests to passthru.tests
fn add_unittests_to_passthru(content: &str) -> anyhow::Result<String> {
    // Check if tests = { exists (simpler check)
    let tests_exists = Regex::new(r"(?m)^\s*tests\s*=\s*\{")?;

    if tests_exists.is_match(content) {
        // passthru.tests exists, add unittests to it
        let result = add_unittests_to_existing_tests(content)?;
        Ok(result)
    } else {
        // Check if passthru exists without tests
        let passthru_pattern = Regex::new(r"(?ms)(^\s*)(passthru\s*=\s*\{)")?;

        if passthru_pattern.is_match(content) {
            // passthru exists, add tests attribute
            add_tests_to_passthru(content)
        } else {
            // No passthru, create one
            add_passthru_with_tests(content)
        }
    }
}

/// Add unittests to existing passthru.tests
fn add_unittests_to_existing_tests(content: &str) -> anyhow::Result<String> {
    // Find the tests attribute and add unittests
    let tests_pattern = Regex::new(r"(?ms)(^\s*tests\s*=\s*\{)(.*?)(^\s*\};)")?;

    if let Some(_caps) = tests_pattern.captures(content) {
        let tests_header = &_caps[1];
        let tests_body = &_caps[2];
        let tests_closing = &_caps[3];

        // Get indentation from tests_closing
        let indent_len = tests_closing.len() - tests_closing.trim_start().len();
        let indent = " ".repeat(indent_len + 2);

        // Check if unittests already exists
        if tests_body.contains("unittests") {
            debug!("unittests already exists in passthru.tests");
            return Ok(content.to_string());
        }

        let result = format!(
            "{}{}\n{}unittests = runUnitTests finalAttrs.finalPackage;\n{}",
            tests_header, tests_body, indent, tests_closing
        );

        Ok(tests_pattern.replace(content, result).into_owned())
    } else {
        anyhow::bail!("Could not find tests pattern in passthru")
    }
}

/// Add tests attribute to existing passthru
fn add_tests_to_passthru(content: &str) -> anyhow::Result<String> {
    // Find passthru and add tests attribute before closing brace
    let passthru_pattern = Regex::new(r"(?ms)(^\s*passthru\s*=\s*\{)(.*?)(^\s*)(\};)")?;

    if let Some(caps) = passthru_pattern.captures(content) {
        let passthru_header = &caps[1];
        let passthru_body = &caps[2];
        let closing_indent = &caps[3];
        let closing_brace = &caps[4];

        // Get indentation
        let indent_len = closing_indent.len() - closing_indent.trim_start().len();
        let indent = " ".repeat(indent_len + 2);

        let result = format!(
            "{}{}{}tests = {{\n{}unittests = runUnitTests finalAttrs.finalPackage;\n{}}};",
            passthru_header, passthru_body, indent, indent, indent
        );

        // Add newline before closing if there was content
        let final_result = if !passthru_body.trim().is_empty() {
            format!("{}\n{}{}", result, closing_indent, closing_brace)
        } else {
            format!("{}{}{}", result, closing_indent, closing_brace)
        };

        Ok(passthru_pattern.replace(content, final_result).into_owned())
    } else {
        anyhow::bail!("Could not find passthru pattern")
    }
}

/// Add passthru with tests
fn add_passthru_with_tests(content: &str) -> anyhow::Result<String> {
    // Add passthru before meta
    let meta_pattern = Regex::new(r"(?m)(^\s*)(meta\s*=)")?;

    if let Some(caps) = meta_pattern.captures(content) {
        let indent = &caps[1];
        let result = meta_pattern.replace(
            content,
            format!(
                "{}passthru = {{\n{}  tests = {{\n{}    unittests = runUnitTests \
                 finalAttrs.finalPackage;\n{}  }};\n{}}};\n\n{}meta =",
                indent, indent, indent, indent, indent, indent
            ),
        );
        return Ok(result.into_owned());
    }

    // If no meta, add before the closing brace using simplified syntax
    // Find the closing brace
    let closing_pattern = Regex::new(r"(?ms)(.*)(^\})\s*$")?;

    if let Some(caps) = closing_pattern.captures(content) {
        let before = &caps[1];
        let closing = &caps[2];

        // Find the indentation from the last non-empty line before the closing brace
        let last_line_pattern = Regex::new(r"(?m)^([ \t]+)[^\s].*$")?;
        let indent = if let Some(indent_caps) = last_line_pattern.captures(before) {
            indent_caps[1].to_string()
        } else {
            "  ".to_string() // Default to 2 spaces
        };

        let result = format!(
            "{}\n{}passthru.tests.unittests = runUnitTests finalAttrs.finalPackage;\n{}",
            before, indent, closing
        );
        return Ok(result);
    }

    // If we still can't find it, just return as is
    debug!("Could not find appropriate place to add passthru");
    Ok(content.to_string())
}

/// Fix closing brace from } to })
fn fix_closing_brace(content: &str) -> anyhow::Result<String> {
    // Find the last closing brace that closes stdenv.mkDerivation
    // This should be the very last closing brace in the file (after meta)
    let pattern = Regex::new(r"(?ms)(.*)(^\})\s*$")?;

    if let Some(caps) = pattern.captures(content) {
        let before = &caps[1];
        let closing = &caps[2];

        // Check if it already ends with })
        if before.trim_end().ends_with(')') {
            return Ok(content.to_string());
        }

        Ok(format!("{}{})\n", before, closing))
    } else {
        Ok(content.to_string())
    }
}

/// Update test-related comments
fn update_test_comments(content: &str) -> String {
    // Replace TODO(corepkgs) comments about moving tests
    let todo_pattern =
        Regex::new(r"(?m)^\s*#.*TODO\(corepkgs\):.*move.*unittests.*passthru.*\n").unwrap();
    let result = todo_pattern.replace_all(content, "");

    // Update "Test suite is quite long" comment to include ", run as passthru"
    let check_comment_pattern =
        Regex::new(r"(?m)(^\s*#\s*Test suite is quite long)(\s*)$").unwrap();
    check_comment_pattern
        .replace_all(&result, "$1, run as passthru$2")
        .into_owned()
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
