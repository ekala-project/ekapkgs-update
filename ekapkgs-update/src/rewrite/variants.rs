use regex::Regex;

use super::error::{Result, RewriteError};

/// Update an attribute within a specific variant in a mkManyVariants variants.nix file
///
/// This function locates a specific variant attribute set and updates an attribute
/// within that variant only, leaving other variants unchanged.
///
/// # Arguments
/// * `content` - The variants.nix file content as a string
/// * `variant_name` - The variant attribute name (e.g., "v0_20", "v1_2")
/// * `attr_name` - The attribute to update within the variant (e.g., "version", "src-hash")
/// * `new_value` - The new value to set (without quotes)
/// * `old_value` - Optional old value to match (for safety)
///
/// # Returns
/// The updated content if successful.
///
/// # Errors
/// Returns a [`RewriteError`] if:
/// - [`RewriteError::Parse`] - the file has invalid Nix syntax
/// - [`RewriteError::NotFound`] - the variant or the attribute within it is not found
/// - [`RewriteError::InvalidResult`] - the replacement would produce invalid syntax
/// - [`RewriteError::Regex`] - the internal regex failed to compile
///
/// # Example
/// ```
/// use ekapkgs_update::rewrite::update_variant_attr;
///
/// let content = r#"{
///   v0_20 = {
///     version = "0.20.1";
///     src-hash = "sha256-old";
///   };
///   v0_23 = {
///     version = "0.23.0";
///     src-hash = "sha256-other";
///   };
/// }"#;
///
/// let result = update_variant_attr(content, "v0_20", "version", "0.20.2", Some("0.20.1"));
/// assert!(result.is_ok());
/// // Only v0_20 is updated, v0_23 remains unchanged
/// ```
pub fn update_variant_attr(
    content: &str,
    variant_name: &str,
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

    // Find the variant attribute set boundaries using regex
    let variant_range = find_variant_range_regex(content, variant_name)?;

    // Extract the variant content
    let variant_content = &content[variant_range.clone()];

    // Build regex pattern to match: attr_name = "value";
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

    // Check if the attribute exists in this variant
    if !re.is_match(variant_content) {
        return Err(RewriteError::attr_not_found_in_variant(
            attr_name,
            variant_name,
        ));
    }

    // Replace the attribute value within the variant content
    let updated_variant = re.replace_all(variant_content, |caps: &regex::Captures<'_>| {
        format!("{}{}{}", &caps[1], new_value, &caps[caps.len() - 1])
    });

    // Reconstruct the full content with the updated variant
    let mut result = String::new();
    result.push_str(&content[..variant_range.start]);
    result.push_str(&updated_variant);
    result.push_str(&content[variant_range.end..]);

    // Validate the result parses correctly
    let result_parse = rnix::Root::parse(&result);
    if !result_parse.errors().is_empty() {
        return Err(RewriteError::InvalidResult {
            operation: "Replacement",
        });
    }

    Ok(result)
}

/// Find the character range of a variant attribute set in a variants.nix file using regex
///
/// This function looks for a pattern like:
/// ```nix
/// variant_name = {
///   ...
/// };
/// ```
fn find_variant_range_regex(content: &str, variant_name: &str) -> Result<std::ops::Range<usize>> {
    // Pattern to match: variant_name = { ... }; or variant_name = rec { ... };
    // We need to match balanced braces
    let start_pattern = format!(
        r"(?m)^\s*{}\s*=\s*(?:rec\s+)?\{{",
        regex::escape(variant_name)
    );

    let start_re = Regex::new(&start_pattern)?;

    // Find the start of the variant attribute set
    let start_match = start_re
        .find(content)
        .ok_or_else(|| RewriteError::variant_not_found(variant_name))?;

    // Find the opening brace position
    let brace_start = content[start_match.end() - 1..]
        .chars()
        .next()
        .and_then(|c| {
            if c == '{' {
                Some(start_match.end() - 1)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            RewriteError::Structural(format!(
                "Failed to find opening brace for variant '{variant_name}'"
            ))
        })?;

    // Find the matching closing brace
    let end_pos = find_matching_brace(content, brace_start)?;

    Ok(brace_start..end_pos + 1)
}

/// Add a new variant entry to a mkManyVariants `variants.nix` file
///
/// Inserts a new variant attribute set before the final closing brace of the
/// top-level attribute set. The new variant is formatted to match the
/// indentation style of existing variants.
///
/// # Arguments
/// * `content` - The variants.nix file content as a string
/// * `variant_name` - The variant attribute name (e.g., "v0_27")
/// * `attrs` - Key-value pairs for the variant (e.g., `[("version", "0.27.0"), ("src-hash", "sha256-...")]`)
///
/// # Returns
/// The updated file content with the new variant added.
///
/// # Errors
/// Returns a [`RewriteError`] if:
/// - The file has invalid Nix syntax
/// - The variant already exists
/// - The result would produce invalid syntax
///
/// # Example
/// ```
/// use ekapkgs_update::rewrite::add_variant_entry;
///
/// let content = r#"{
///   v0_20 = {
///     version = "0.20.1";
///     src-hash = "sha256-old";
///   };
///   v0_23 = {
///     version = "0.23.0";
///     src-hash = "sha256-other";
///   };
/// }"#;
///
/// let result = add_variant_entry(content, "v0_27", &[("version", "0.27.0"), ("src-hash", "sha256-new")]);
/// assert!(result.is_ok());
/// let updated = result.unwrap();
/// assert!(updated.contains("v0_27"));
/// assert!(updated.contains("0.27.0"));
/// ```
pub fn add_variant_entry(
    content: &str,
    variant_name: &str,
    attrs: &[(&str, &str)],
) -> Result<String> {
    // Validate the input parses correctly
    let parse = rnix::Root::parse(content);
    if !parse.errors().is_empty() {
        let errors: Vec<String> = parse
            .errors()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        return Err(RewriteError::Parse(errors.join(", ")));
    }

    // Check that the variant doesn't already exist
    let check_pattern = format!(r"(?m)^\s*{}\s*=", regex::escape(variant_name));
    let check_re = Regex::new(&check_pattern)?;
    if check_re.is_match(content) {
        return Err(RewriteError::Structural(format!(
            "Variant '{variant_name}' already exists"
        )));
    }

    // Detect indentation from an existing variant block
    let indent = detect_variant_indent(content);

    // Build the new variant block
    let mut block = format!("{indent}{variant_name} = {{\n");
    for (key, value) in attrs {
        block.push_str(&format!("{indent}  {key} = \"{value}\";\n"));
    }
    block.push_str(&format!("{indent}}};\n"));

    // Find the final closing brace of the outer attrset
    let insert_pos = content
        .rfind('}')
        .ok_or_else(|| RewriteError::Structural("No closing brace found in content".to_owned()))?;

    // Insert the new variant before the final closing brace
    let mut result = String::with_capacity(content.len() + block.len());
    result.push_str(&content[..insert_pos]);
    result.push_str(&block);
    result.push_str(&content[insert_pos..]);

    // Validate the result
    let result_parse = rnix::Root::parse(&result);
    if !result_parse.errors().is_empty() {
        return Err(RewriteError::InvalidResult {
            operation: "AddVariant",
        });
    }

    Ok(result)
}

/// Detect the indentation used for variant blocks in a variants.nix file
///
/// Looks for lines matching `<whitespace><identifier> = {` and returns the
/// leading whitespace. Falls back to two spaces if no variant is found.
fn detect_variant_indent(content: &str) -> String {
    let variant_pattern = Regex::new(r"(?m)^(\s+)\w+\s*=\s*\{").expect("indent regex");
    if let Some(caps) = variant_pattern.captures(content) {
        caps.get(1)
            .map_or("  ".to_owned(), |m| m.as_str().to_owned())
    } else {
        "  ".to_owned()
    }
}

/// Find the position of the closing brace matching an opening brace
fn find_matching_brace(content: &str, start_pos: usize) -> Result<usize> {
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, ch) in content[start_pos..].char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Ok(start_pos + i);
                }
            },
            _ => {},
        }
    }

    Err(RewriteError::Structural(
        "No matching closing brace found".to_owned(),
    ))
}
