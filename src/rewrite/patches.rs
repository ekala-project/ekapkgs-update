use regex::Regex;

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
    // Matches: patches = [ ]; or patches = [ # comment ]; or patches = [ /* comment */ ];
    // Pattern explanation:
    // - (?ms)^ - start of line (multiline and dotall modes)
    // - \s*patches\s*=\s*\[ - matches "patches = ["
    // - (?:\s|#[^\n]*|/\*.*?\*/)* - matches any number of:
    //   - whitespace
    //   - single-line comments (# ...)
    //   - multiline comments (/* ... */)
    // - \]\s*; - matches "];"
    let empty_pattern =
        Regex::new(r"(?ms)^\s*patches\s*=\s*\[(?:\s|#[^\n]*|/\*.*?\*/)*\]\s*;").ok();

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
        let errors: Vec<String> = parse
            .errors()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        anyhow::bail!("Failed to parse Nix file: {}", errors.join(", "));
    }

    // Pattern to match the entire patches attribute (including comments)
    // Matches: patches = [ ]; or patches = [ # comment ]; or patches = [ /* comment */ ];
    // Only removes the line itself and its immediate newline, preserving following whitespace
    // Handles both # single-line and /* */ multiline comments
    let pattern = r"\n?(?ms)^\s*patches\s*=\s*\[(?:\s|#[^\n]*|/\*.*?\*/)*\]\s*;";
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
        let errors: Vec<String> = parse
            .errors()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
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
    anyhow::bail!("Patch '{patch_name}' not found in patches array")
}
