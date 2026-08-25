use regex::Regex;

use super::error::{Result, RewriteError};

/// Replace meta.teams with an empty array
///
/// # Arguments
/// * `content` - The Nix file content as a string
///
/// # Returns
/// A tuple of (updated_content, changed) where changed indicates if any replacement was made
///
/// # Errors
/// Returns a [`RewriteError`] if:
/// - [`RewriteError::Parse`] - the file has invalid Nix syntax before replacement
/// - [`RewriteError::InvalidResult`] - the replacement would create invalid syntax
/// - [`RewriteError::Regex`] - the internal regex failed to compile
pub fn replace_teams_with_empty(content: &str) -> Result<(String, bool)> {
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

    // Check if teams is already exactly empty (teams = [ ];)
    let empty_pattern = Regex::new(r"(?m)^\s*teams\s*=\s*\[\s*\]\s*;")?;
    if empty_pattern.is_match(content) {
        return Ok((content.to_owned(), false));
    }

    // Pattern to match teams attribute with any value
    // Handles: teams = [ ... ]; or teams = with lib; [ ... ];
    let pattern = r"(?m)(\s*teams\s*=\s*)(?:with\s+[^;]*;\s*)?\[[^\]]*\]\s*;";
    let regex = Regex::new(pattern)?;

    if !regex.is_match(content) {
        return Ok((content.to_owned(), false));
    }

    // Replace with empty array, preserving the leading whitespace and attribute name
    let result = regex.replace_all(content, "${1}[ ];");

    // Validate the result parses correctly
    let result_parse = rnix::Root::parse(&result);
    if !result_parse.errors().is_empty() {
        return Err(RewriteError::InvalidResult {
            operation: "Replacement",
        });
    }

    Ok((result.into_owned(), true))
}
