use regex::Regex;

use super::error::{Result, RewriteError};

/// Find and update an attribute value in a Nix file using regex with rnix validation
///
/// # Arguments
/// * `content` - The Nix file content as a string
/// * `attr_name` - The attribute name to find (e.g., "version", "hash")
/// * `new_value` - The new value to set (without quotes)
/// * `old_value` - Optional old value to match (for safety)
///
/// # Returns
/// The updated content if successful.
///
/// # Errors
/// Returns a [`RewriteError`] if:
/// - [`RewriteError::Parse`] - the file has invalid Nix syntax
/// - [`RewriteError::NotFound`] - the attribute is not found (or the old value doesn't match the
///   matched site)
/// - [`RewriteError::InvalidResult`] - the replacement would produce invalid Nix syntax
/// - [`RewriteError::Regex`] - the internal regex failed to compile
pub fn find_and_update_attr(
    content: &str,
    attr_name: &str,
    new_value: &str,
    old_value: Option<&str>,
) -> Result<String> {
    // First, validate that the file parses correctly
    let parse = rnix::Root::parse(content);
    if !parse.errors().is_empty() {
        let errors: Vec<String> = parse
            .errors()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        return Err(RewriteError::Parse(errors.join(", ")));
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
        return Err(RewriteError::attr_not_found(attr_name));
    }

    // Replace the attribute value
    let result = re.replace_all(content, |caps: &regex::Captures<'_>| {
        format!("{}{}{}", &caps[1], new_value, &caps[caps.len() - 1])
    });

    // Validate the result parses correctly
    let result_parse = rnix::Root::parse(&result);
    if !result_parse.errors().is_empty() {
        return Err(RewriteError::InvalidResult {
            operation: "Replacement",
        });
    }

    Ok(result.into_owned())
}
