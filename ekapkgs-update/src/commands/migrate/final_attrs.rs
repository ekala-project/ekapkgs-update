use regex::Regex;

/// Convert stdenv.mkDerivation rec { to stdenv.mkDerivation (finalAttrs: rec {
pub fn convert_to_final_attrs_pattern(content: &str) -> anyhow::Result<String> {
    // Pattern to match: stdenv.mkDerivation rec {
    // or: stdenv.mkDerivation {
    let pattern = Regex::new(r"(stdenv\.mkDerivation\s+)(rec\s+)?(\{)")?;

    let result = pattern.replace(content, |caps: &regex::Captures<'_>| {
        let prefix = &caps[1];
        let has_rec = caps.get(2).is_some();

        if has_rec {
            format!("{prefix}(finalAttrs: rec {{")
        } else {
            format!("{prefix}(finalAttrs: {{")
        }
    });

    Ok(result.into_owned())
}

/// Fix the closing brace to add the closing parenthesis for finalAttrs
pub fn fix_closing_brace(content: &str) -> anyhow::Result<String> {
    // Find the last closing brace that closes stdenv.mkDerivation
    // This should be the very last closing brace in the file (after meta)
    let pattern = Regex::new(r"(?ms)(.*)(^\})\s*$")?;

    if let Some(caps) = pattern.captures(content) {
        let before = &caps[1];
        let closing = &caps[2];

        // Check if it already ends with })
        if before.trim_end().ends_with(')') {
            return Ok(content.to_owned());
        }

        Ok(format!("{before}{closing})\n"))
    } else {
        Ok(content.to_owned())
    }
}
