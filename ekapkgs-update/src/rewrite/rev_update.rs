use super::attributes::find_and_update_attr;
use super::error::{Result, RewriteError};

/// Attempt to update the `rev` attribute if it contains a version-like pattern.
///
/// This function is conservative about what it updates to avoid breaking packages:
/// - It only updates `rev` if it clearly references the version
/// - It skips commit SHAs (40 hex characters)
/// - It skips string interpolations (contain `${...}`)
/// - It skips unquoted references (e.g., `rev = version;`)
///
/// Supported patterns:
/// 1. **Prefix pattern**: `rev = "v1.2.3"` → `rev = "v2.0.0"`
/// 2. **Smart substring**: `rev = "jq-1.6"` → `rev = "jq-1.7"` (if old_version appears in rev)
///
/// # Examples
///
/// ```rust,ignore
/// // Updates v-prefixed tags
/// let result = try_update_rev_attr(
///     r#"rev = "v1.0.0";"#,
///     "1.0.0",
///     "2.0.0"
/// );
/// assert!(result.unwrap().contains(r#"rev = "v2.0.0";"#));
///
/// // Skips commit SHAs
/// let result = try_update_rev_attr(
///     r#"rev = "abc123def456789012345678901234567890abcd";"#,
///     "1.0.0",
///     "2.0.0"
/// );
/// assert!(result.is_err()); // Not updated
///
/// // Skips string interpolations
/// let result = try_update_rev_attr(
///     r#"rev = "v${version}";"#,
///     "1.0.0",
///     "2.0.0"
/// );
/// assert!(result.is_err()); // Not updated
/// ```
pub fn try_update_rev_attr(content: &str, old_version: &str, new_version: &str) -> Result<String> {
    // Extract the current rev value to inspect it
    let current_rev = extract_rev_value(content)?;

    // Skip commit SHAs (40 hex characters)
    if is_likely_commit_sha(&current_rev) {
        return Err(RewriteError::attr_not_found("rev (commit SHA, skipped)"));
    }

    // Skip string interpolations (they automatically reference ${version})
    if current_rev.contains("${") {
        return Err(RewriteError::attr_not_found(
            "rev (string interpolation, skipped)",
        ));
    }

    // Try different update patterns in order of specificity

    // Pattern 1: "v{version}" - most common
    let old_rev_v = format!("v{}", old_version);
    let new_rev_v = format!("v{}", new_version);
    if current_rev == old_rev_v {
        return find_and_update_attr(content, "rev", &new_rev_v, Some(&old_rev_v));
    }

    // Pattern 2: Smart substring detection
    // If the old version appears anywhere in the rev string, replace it
    // This handles cases like "jq-1.6", "release-2.0.0", etc.
    if current_rev.contains(old_version) {
        let new_rev = current_rev.replace(old_version, new_version);
        return find_and_update_attr(content, "rev", &new_rev, Some(&current_rev));
    }

    // No recognized pattern found
    Err(RewriteError::attr_not_found(
        "rev (does not match version pattern)",
    ))
}

/// Extract the value of the rev attribute from Nix content.
///
/// Returns the string value without quotes.
/// Returns error if rev attribute is not found or has unexpected format.
fn extract_rev_value(content: &str) -> Result<String> {
    use regex::Regex;

    // Match: rev = "...";
    let pattern = r#"(?m)\s*rev\s*=\s*"([^"]*)"\s*;"#;
    let re = Regex::new(pattern)?;

    let caps = re
        .captures(content)
        .ok_or_else(|| RewriteError::attr_not_found("rev"))?;

    Ok(caps[1].to_string())
}

/// Check if a value is likely a Git commit SHA.
///
/// Git commit SHAs are 40 hexadecimal characters.
/// We don't want to update these because they point to specific commits,
/// not version tags.
///
/// # Examples
///
/// ```rust,ignore
/// assert!(is_likely_commit_sha("abc123def456789012345678901234567890abcd"));
/// assert!(!is_likely_commit_sha("v1.2.3"));
/// assert!(!is_likely_commit_sha("abc123")); // Too short
/// ```
fn is_likely_commit_sha(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_likely_commit_sha() {
        // Valid commit SHA
        assert!(is_likely_commit_sha(
            "abc123def456789012345678901234567890abcd"
        ));
        assert!(is_likely_commit_sha(
            "1234567890123456789012345678901234567890"
        ));

        // Not commit SHAs
        assert!(!is_likely_commit_sha("v1.2.3"));
        assert!(!is_likely_commit_sha("abc123")); // Too short
        assert!(!is_likely_commit_sha(
            "abc123def456789012345678901234567890abcg"
        )); // Contains 'g'
        assert!(!is_likely_commit_sha(
            "abc123def456789012345678901234567890abcd1"
        )); // Too long
    }

    #[test]
    fn test_extract_rev_value() {
        let content = r#"{
  version = "1.0.0";
  rev = "v1.0.0";
}"#;
        let result = extract_rev_value(content);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "v1.0.0");
    }

    #[test]
    fn test_extract_rev_value_not_found() {
        let content = r#"{
  version = "1.0.0";
}"#;
        let result = extract_rev_value(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_rev_value_with_interpolation() {
        let content = r#"{
  version = "1.0.0";
  rev = "v${version}";
}"#;
        let result = extract_rev_value(content);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "v${version}");
    }
}
