use regex::Regex;

/// Replace meta.maintainers with an empty array
///
/// # Arguments
/// * `content` - The Nix file content as a string
///
/// # Returns
/// A tuple of (updated_content, changed) where changed indicates if any replacement was made
///
/// # Errors
/// Returns an error if:
/// - The file has invalid Nix syntax before replacement
/// - The replacement would create invalid syntax
pub fn replace_maintainers_with_empty(content: &str) -> anyhow::Result<(String, bool)> {
    // First, validate that the file parses correctly
    let parse = rnix::Root::parse(content);
    if !parse.errors().is_empty() {
        let errors: Vec<String> = parse.errors().iter().map(|e| e.to_string()).collect();
        anyhow::bail!("Failed to parse Nix file: {}", errors.join(", "));
    }

    // Check if maintainers is already exactly empty (maintainers = [ ];)
    // Pattern matches: maintainers = [ ]; with any whitespace inside the brackets
    let empty_pattern = Regex::new(r"(?m)^\s*maintainers\s*=\s*\[\s*\]\s*;")?;
    if empty_pattern.is_match(content) {
        // Already empty, no need to change
        return Ok((content.to_string(), false));
    }

    // Pattern to match maintainers attribute with any value
    // Handles: maintainers = [ ... ]; or maintainers = with lib; [ ... ];
    // This matches the attribute name, =, and everything until the closing ];
    let pattern = r"(?m)(\s*maintainers\s*=\s*)(?:with\s+[^;]*;\s*)?\[[^\]]*\]\s*;";
    let regex = Regex::new(pattern)?;

    if !regex.is_match(content) {
        // No maintainers found, return unchanged
        return Ok((content.to_string(), false));
    }

    // Replace with empty array, preserving the leading whitespace and attribute name
    let result = regex.replace_all(content, "${1}[ ];");

    // Validate the result parses correctly
    let result_parse = rnix::Root::parse(&result);
    if !result_parse.errors().is_empty() {
        anyhow::bail!("Replacement would create invalid Nix syntax");
    }

    Ok((result.into_owned(), true))
}
