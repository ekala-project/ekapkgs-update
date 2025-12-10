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
        return Err(anyhow::anyhow!(
            "Attribute '{}' not found in Nix file",
            attr_name
        ));
    }

    // Replace the attribute value
    let result = re.replace_all(content, |caps: &regex::Captures| {
        format!("{}{}{}", &caps[1], new_value, &caps[caps.len() - 1])
    });

    // Validate the result parses correctly
    let result_parse = rnix::Root::parse(&result);
    if !result_parse.errors().is_empty() {
        return Err(anyhow::anyhow!(
            "Replacement would create invalid Nix syntax"
        ));
    }

    Ok(result.into_owned())
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
}
