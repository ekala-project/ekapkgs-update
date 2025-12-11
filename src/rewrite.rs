//! Nix file rewriting utilities using AST validation and text manipulation

use regex::Regex;

/// Find and update an attribute value in a Nix file using regex with rnix validation
///
/// # Arguments
/// * `content` - The Nix file content as a string
/// * `attr_name` - The attribute name to find (e.g., "version", "hash")
/// * `new_value` - The new value to set (without quotes)
/// * `old_value` - Optional old value to match (for safety)
///
/// # Returns
/// The updated content if successful, or an error if:
/// - The file has invalid Nix syntax
/// - The attribute is not found
/// - The old value doesn't match (if specified)
/// - The replacement would create invalid syntax
///
/// # Example
/// ```
/// use ekapkgs_update::rewrite::find_and_update_attr;
///
/// let content = r#"{ version = "1.0.0"; }"#;
/// let result = find_and_update_attr(content, "version", "2.0.0", Some("1.0.0"));
/// assert!(result.is_ok());
/// ```
pub fn find_and_update_attr(
    content: &str,
    attr_name: &str,
    new_value: &str,
    old_value: Option<&str>,
) -> anyhow::Result<String> {
    // First, validate that the file parses correctly
    let parse = rnix::Root::parse(content);
    if !parse.errors().is_empty() {
        let errors: Vec<String> = parse.errors().iter().map(|e| e.to_string()).collect();
        return Err(anyhow::anyhow!(
            "Failed to parse Nix file: {}",
            errors.join(", ")
        ));
    }

    // Build regex pattern to match: attr_name = "value";
    // This handles various whitespace patterns
    let pattern = if let Some(old) = old_value {
        // Match specific old value
        format!(
            r#"(?m)(\s*{}\s*=\s*"){}("\s*;)"#,
            regex::escape(attr_name),
            regex::escape(old)
        )
    } else {
        // Match any value
        format!(
            r#"(?m)(\s*{}\s*=\s*")([^"]*)("\s*;)"#,
            regex::escape(attr_name)
        )
    };

    let re = Regex::new(&pattern)?;

    // Check if the attribute exists
    if !re.is_match(content) {
        anyhow::bail!("Attribute '{}' not found in Nix file", attr_name);
    }

    // Replace the attribute value
    let result = re.replace_all(content, |caps: &regex::Captures| {
        format!("{}{}{}", &caps[1], new_value, &caps[caps.len() - 1])
    });

    // Validate the result parses correctly
    let result_parse = rnix::Root::parse(&result);
    if !result_parse.errors().is_empty() {
        anyhow::bail!("Replacement would create invalid Nix syntax");
    }

    Ok(result.into_owned())
}

/// Check if the patches array is empty
///
/// # Arguments
/// * `content` - The Nix file content as a string
///
/// # Returns
/// true if patches attribute exists and is an empty array (or only contains comments), false
/// otherwise
pub fn is_patches_array_empty(content: &str) -> bool {
    // Use regex to detect empty patches array, ignoring comments
    // Matches: patches = [ ]; or patches = [ # comment ]; or patches = [\n  # comment\n];
    // Pattern explanation:
    // - (?m)^ - start of line (multiline mode)
    // - \s*patches\s*=\s*\[ - matches "patches = ["
    // - (?:\s*(?:#[^\n]*)?)* - matches any number of lines with only whitespace/comments
    // - \s*\]\s*; - matches "];"
    let empty_pattern = Regex::new(r"(?ms)^\s*patches\s*=\s*\[\s*(?:#[^\n]*\n?\s*)*\]\s*;").ok();

    if let Some(regex) = empty_pattern {
        regex.is_match(content)
    } else {
        false
    }
}

/// Remove the patches attribute from a Nix file
///
/// # Arguments
/// * `content` - The Nix file content as a string
///
/// # Returns
/// The updated content with the patches attribute removed, or an error if:
/// - The file has invalid Nix syntax
/// - The patches attribute is not found
/// - The removal would create invalid syntax
pub fn remove_patches_attribute(content: &str) -> anyhow::Result<String> {
    // First, validate that the file parses correctly
    let parse = rnix::Root::parse(content);
    if !parse.errors().is_empty() {
        let errors: Vec<String> = parse.errors().iter().map(|e| e.to_string()).collect();
        anyhow::bail!("Failed to parse Nix file: {}", errors.join(", "));
    }

    // Pattern to match the entire patches attribute (including comments)
    // Matches: patches = [ ]; or patches = [ # comment ]; or multiline with only comments
    // Only removes the line itself and its immediate newline, preserving following whitespace
    let pattern = r"\n?(?m)^\s*patches\s*=\s*\[\s*(?:#[^\n]*\n?\s*)*\]\s*;";
    let regex = Regex::new(pattern)?;

    if !regex.is_match(content) {
        anyhow::bail!("Empty patches attribute not found in Nix file");
    }

    let result = regex.replace(content, "");

    // Validate the result parses correctly
    let result_parse = rnix::Root::parse(&result);
    if !result_parse.errors().is_empty() {
        anyhow::bail!("Removal would create invalid Nix syntax");
    }

    Ok(result.into_owned())
}

/// Remove a patch from the patches array in a Nix file
///
/// # Arguments
/// * `content` - The Nix file content as a string
/// * `patch_name` - The patch filename to remove (e.g., "fix-build.patch")
///
/// # Returns
/// The updated content with the patch removed, or an error if:
/// - The file has invalid Nix syntax
/// - The patches attribute is not found
/// - The patch is not found in the array
/// - The removal would create invalid syntax
///
/// This function uses regex-based removal since rnix doesn't provide easy
/// whitespace-preserving AST manipulation for array elements.
pub fn remove_patch_from_array(content: &str, patch_name: &str) -> anyhow::Result<String> {
    // First, validate that the file parses correctly
    let parse = rnix::Root::parse(content);
    if !parse.errors().is_empty() {
        let errors: Vec<String> = parse.errors().iter().map(|e| e.to_string()).collect();
        return Err(anyhow::anyhow!(
            "Failed to parse Nix file: {}",
            errors.join(", ")
        ));
    }

    // Build regex pattern to match the patch entry in the array
    // Handles various formats:
    // - ./patch-name.patch
    // - (fetchpatch { name = "patch-name.patch"; ... })
    // We need to match the entire line including potential trailing comma and whitespace

    // Pattern 1: Simple path reference like ./patch-name.patch
    // Match the whole line with leading whitespace and optional trailing comma
    let simple_pattern = format!(r#"(?m)^\s*\.\/{}(?:,)?\s*$\n?"#, regex::escape(patch_name));

    let simple_regex = Regex::new(&simple_pattern)?;

    if simple_regex.is_match(content) {
        let result = simple_regex.replace(content, "");

        // Validate the result parses correctly
        let result_parse = rnix::Root::parse(&result);
        if !result_parse.errors().is_empty() {
            anyhow::bail!("Removal would create invalid Nix syntax");
        }

        return Ok(result.into_owned());
    }

    // Pattern 2: fetchpatch or other complex expression
    // Look for lines containing the patch name within a fetchpatch call or similar
    // This is more complex - we need to find the entire expression
    let fetch_pattern = format!(
        r#"(?ms)^\s*\(fetchpatch\s+\{{[^}}]*{}[^}}]*\}}\)[\s,]*\n"#,
        regex::escape(patch_name)
    );

    let fetch_regex = Regex::new(&fetch_pattern)?;

    if fetch_regex.is_match(content) {
        let result = fetch_regex.replace(content, "");

        // Validate the result parses correctly
        let result_parse = rnix::Root::parse(&result);
        if !result_parse.errors().is_empty() {
            anyhow::bail!("Removal would create invalid Nix syntax");
        }

        return Ok(result.into_owned());
    }

    // If we didn't find the patch, return an error
    anyhow::bail!("Patch '{}' not found in patches array", patch_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_and_update_attr_simple() {
        let content = r#"{
  version = "1.0.0";
  hash = "sha256-old";
}"#;

        let result = find_and_update_attr(content, "version", "2.0.0", Some("1.0.0"));
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.contains(r#"version = "2.0.0";"#));
        assert!(!updated.contains(r#"version = "1.0.0";"#));
    }

    #[test]
    fn test_find_and_update_attr_hash() {
        let content = r#"{
  version = "1.0.0";
  hash = "sha256-oldhashabcdefg";
}"#;

        let result = find_and_update_attr(
            content,
            "hash",
            "sha256-newhashabcdefg",
            Some("sha256-oldhashabcdefg"),
        );
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.contains(r#"hash = "sha256-newhashabcdefg";"#));
        assert!(!updated.contains("sha256-oldhashabcdefg"));
    }

    #[test]
    fn test_find_and_update_attr_not_found() {
        let content = r#"{
  version = "1.0.0";
}"#;

        let result = find_and_update_attr(content, "hash", "newvalue", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_find_and_update_attr_wrong_old_value() {
        let content = r#"{
  version = "1.0.0";
}"#;

        let result = find_and_update_attr(content, "version", "2.0.0", Some("9.9.9"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_find_and_update_attr_preserves_formatting() {
        let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  src = {
    hash = "sha256-abc";
  };
}"#;

        let result = find_and_update_attr(content, "version", "2.0.0", Some("1.0.0"));
        assert!(result.is_ok());
        let updated = result.unwrap();

        // Check that the structure is preserved
        assert!(updated.contains("pname"));
        assert!(updated.contains("src ="));
        assert!(updated.contains(r#"version = "2.0.0";"#));
    }

    #[test]
    fn test_find_and_update_attr_invalid_syntax() {
        let content = r#"{
  version = "1.0.0"
  # missing semicolon
}"#;

        let result = find_and_update_attr(content, "version", "2.0.0", None);
        // Should fail during initial parse validation
        assert!(result.is_err());
    }

    #[test]
    fn test_find_and_update_attr_multiple_occurrences() {
        let content = r#"{
  version = "1.0.0";
  oldVersion = "1.0.0";
}"#;

        let result = find_and_update_attr(content, "version", "2.0.0", Some("1.0.0"));
        assert!(result.is_ok());
        let updated = result.unwrap();

        // Should only update the 'version' attribute, not 'oldVersion'
        assert!(updated.contains(r#"version = "2.0.0";"#));
        assert!(updated.contains(r#"oldVersion = "1.0.0";"#));
    }

    #[test]
    fn test_find_and_update_attr_with_special_chars() {
        let content = r#"{
  version = "1.0.0+build.123";
}"#;

        let result = find_and_update_attr(
            content,
            "version",
            "2.0.0+build.456",
            Some("1.0.0+build.123"),
        );
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.contains(r#"version = "2.0.0+build.456";"#));
    }

    #[test]
    fn test_remove_patch_from_array_simple() {
        let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [
    ./fix-build.patch
    ./add-feature.patch
    ./security-fix.patch
  ];
}"#;

        let result = remove_patch_from_array(content, "fix-build.patch");
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(!updated.contains("fix-build.patch"));
        assert!(updated.contains("add-feature.patch"));
        assert!(updated.contains("security-fix.patch"));
    }

    #[test]
    fn test_remove_patch_from_array_middle_element() {
        let content = r#"{
  patches = [
    ./first.patch
    ./middle.patch
    ./last.patch
  ];
}"#;

        let result = remove_patch_from_array(content, "middle.patch");
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.contains("first.patch"));
        assert!(!updated.contains("middle.patch"));
        assert!(updated.contains("last.patch"));
    }

    #[test]
    fn test_remove_patch_from_array_not_found() {
        let content = r#"{
  patches = [
    ./existing.patch
  ];
}"#;

        let result = remove_patch_from_array(content, "nonexistent.patch");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_remove_patch_from_array_last_element() {
        let content = r#"{
  patches = [
    ./first.patch
    ./second.patch
    ./third.patch
  ];
}"#;

        let result = remove_patch_from_array(content, "third.patch");
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(updated.contains("first.patch"));
        assert!(updated.contains("second.patch"));
        assert!(!updated.contains("third.patch"));
    }

    #[test]
    fn test_is_patches_array_empty_true() {
        let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [ ];
}"#;

        assert!(is_patches_array_empty(content));
    }

    #[test]
    fn test_is_patches_array_empty_compact() {
        let content = r#"{
  pname = "mypackage";
  patches = [];
}"#;

        assert!(is_patches_array_empty(content));
    }

    #[test]
    fn test_is_patches_array_empty_false() {
        let content = r#"{
  patches = [
    ./some.patch
  ];
}"#;

        assert!(!is_patches_array_empty(content));
    }

    #[test]
    fn test_is_patches_array_empty_no_patches() {
        let content = r#"{
  pname = "mypackage";
  version = "1.0.0";
}"#;

        assert!(!is_patches_array_empty(content));
    }

    #[test]
    fn test_is_patches_array_empty_with_single_line_comment() {
        let content = r#"{
  pname = "mypackage";

  patches = [ # all patches removed
  ];
}"#;

        assert!(is_patches_array_empty(content));
    }

    #[test]
    fn test_is_patches_array_empty_with_multiline_comments() {
        let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [
    # This patch was removed
    # Another comment
  ];
}"#;

        assert!(is_patches_array_empty(content));
    }

    #[test]
    fn test_is_patches_array_empty_with_mixed_whitespace_and_comments() {
        let content = r#"{
  patches = [

    # Comment after blank line

    # Another comment

  ];
}"#;

        assert!(is_patches_array_empty(content));
    }

    #[test]
    fn test_remove_patches_attribute() {
        let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [ ];

  src = fetchurl {
    url = "https://example.com/file.tar.gz";
  };
}"#;

        let result = remove_patches_attribute(content);
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(!updated.contains("patches"));
        assert!(updated.contains("pname"));
        assert!(updated.contains("src"));
    }

    #[test]
    fn test_remove_patches_attribute_not_found() {
        let content = r#"{
  pname = "mypackage";
  version = "1.0.0";
}"#;

        let result = remove_patches_attribute(content);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_remove_patches_attribute_non_empty() {
        let content = r#"{
  patches = [
    ./some.patch
  ];
}"#;

        let result = remove_patches_attribute(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_patches_attribute_with_comments() {
        let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [
    # All patches were removed
    # This is now empty
  ];

  src = fetchurl {
    url = "https://example.com/file.tar.gz";
  };
}"#;

        let result = remove_patches_attribute(content);
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(!updated.contains("patches"));
        assert!(!updated.contains("All patches were removed"));
        assert!(updated.contains("pname"));
        assert!(updated.contains("src"));
    }

    #[test]
    fn test_remove_patches_attribute_with_inline_comment() {
        let content = r#"{
  pname = "mypackage";

  patches = [ # obsolete patches removed
  ];
}"#;

        let result = remove_patches_attribute(content);
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(!updated.contains("patches"));
        assert!(!updated.contains("obsolete"));
    }

    #[test]
    fn test_remove_patches_attribute_preserves_blank_lines() {
        let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [ ];

  src = fetchurl {
    url = "https://example.com/file.tar.gz";
  };
}"#;

        let result = remove_patches_attribute(content);
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(!updated.contains("patches"));

        // Verify blank line before src is preserved
        assert!(updated.contains("\n  src = "));

        // Verify indentation of src is preserved
        assert!(updated.contains("  src = fetchurl"));
    }

    #[test]
    fn test_remove_patches_attribute_preserves_following_indentation() {
        let content = r#"{
  pname = "mypackage";
  patches = [ ];
  buildInputs = [ pkg1 pkg2 ];
}"#;

        let result = remove_patches_attribute(content);
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert!(!updated.contains("patches"));

        // Verify the buildInputs line maintains its indentation
        assert!(updated.contains("  buildInputs = "));
    }
}
