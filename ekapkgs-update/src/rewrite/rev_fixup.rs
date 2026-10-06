use regex::Regex;

use super::error::{Result, RewriteError};
use super::rev_update::{extract_rev_value, is_likely_commit_sha};
use crate::commands::migrate::{has_final_attrs_pattern, has_rec_pattern};

/// Infer the tag prefix from a tag name and version.
///
/// For example, `tag_name = "v5.1.0"` with `version = "5.1.0"` yields `Some("v")`.
/// Returns `None` if the version does not appear in the tag name.
pub fn infer_tag_prefix<'a>(tag_name: &'a str, version: &str) -> Option<&'a str> {
    let idx = tag_name.find(version)?;
    Some(&tag_name[..idx])
}

/// Check whether the file content needs `finalAttrs` conversion to allow
/// `rev` to reference `version`.
///
/// Returns `true` when the builder call uses neither `rec {` nor `(finalAttrs:`.
pub fn needs_final_attrs_conversion(content: &str) -> bool {
    !has_rec_pattern(content) && !has_final_attrs_pattern(content)
}

/// Attempt to rewrite a stale commit-SHA `rev` to reference the version
/// via the tag format discovered upstream.
///
/// This function:
/// 1. Verifies `rev` is currently a commit SHA
/// 2. Infers the tag prefix from `tag_name` (e.g. `"v"` from `"v5.1.0"`)
/// 3. Constructs the new `rev` using `${version}` or `${finalAttrs.version}`
///    depending on whether `rec` or `finalAttrs` is present
///
/// Callers should convert to `finalAttrs` *before* calling this function
/// if [`needs_final_attrs_conversion`] returns `true`.
pub fn try_fixup_stale_rev(content: &str, tag_name: &str, new_version: &str) -> Result<String> {
    // Skip unstable packages — they intentionally use commit SHAs
    if new_version.contains("unstable") {
        return Err(RewriteError::attr_not_found(
            "rev fixup (unstable version, skipped)",
        ));
    }

    let current_rev = extract_rev_value(content)?;

    if !is_likely_commit_sha(&current_rev) {
        return Err(RewriteError::attr_not_found("rev fixup (not a commit SHA)"));
    }

    let prefix = infer_tag_prefix(tag_name, new_version)
        .ok_or_else(|| RewriteError::attr_not_found("rev fixup (version not found in tag name)"))?;

    // Determine which version reference to use
    let version_ref = if has_final_attrs_pattern(content) {
        "finalAttrs.version"
    } else if has_rec_pattern(content) {
        "version"
    } else {
        return Err(RewriteError::attr_not_found(
            "rev fixup (no rec or finalAttrs pattern — convert first)",
        ));
    };

    // Build the new rev value as a Nix string interpolation
    let new_rev_nix = if prefix.is_empty() {
        format!("${{{version_ref}}}")
    } else {
        format!("{prefix}${{{version_ref}}}")
    };

    // Replace rev = "<40-hex-sha>" with rev = "<new_rev_nix>"
    // We use replacen on the raw string to avoid regex capture group conflicts
    // with Nix's ${...} interpolation syntax.
    let pattern = Regex::new(r#"(?m)(\s*rev\s*=\s*)"[0-9a-fA-F]{40}"(\s*;)"#)?;
    let caps = pattern.captures(content).ok_or_else(|| {
        RewriteError::attr_not_found("rev fixup (could not locate commit SHA rev in file)")
    })?;
    let full_match = caps.get(0).unwrap();
    let prefix = &caps[1];
    let suffix = &caps[2];
    let replacement = format!(r#"{prefix}"{new_rev_nix}"{suffix}"#);
    let result = format!(
        "{}{}{}",
        &content[..full_match.start()],
        replacement,
        &content[full_match.end()..]
    );

    // Validate the result parses as valid Nix
    let parse = rnix::Root::parse(&result);
    if !parse.errors().is_empty() {
        return Err(RewriteError::attr_not_found(
            "rev fixup (produced invalid Nix syntax)",
        ));
    }

    Ok(result)
}
