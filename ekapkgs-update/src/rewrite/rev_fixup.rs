use std::sync::OnceLock;

use regex::Regex;

use super::error::{Result, RewriteError};
use super::rev_update::{extract_rev_value, is_likely_commit_sha};
use crate::commands::migrate::{has_final_attrs_pattern, has_rec_pattern};

/// Split a tag name around the version: `"hdf5_1.14.6"` with `"1.14.6"` yields
/// `Some(("hdf5_", ""))`, `"v2.0-stable"` with `"2.0"` yields `Some(("v",
/// "-stable"))`. Returns `None` if the version does not appear in the tag name.
pub fn split_tag<'a>(tag_name: &'a str, version: &str) -> Option<(&'a str, &'a str)> {
    let idx = tag_name.find(version)?;
    Some((&tag_name[..idx], &tag_name[idx + version.len()..]))
}

/// Number of git ref bindings (`rev = …;` or `tag = …;`) in the file.
fn git_ref_count(content: &str) -> usize {
    static REF: OnceLock<Regex> = OnceLock::new();
    REF.get_or_init(|| Regex::new(r"(?m)^[ \t]*(?:rev|tag)\s*=").expect("valid regex"))
        .find_iter(content)
        .count()
}

/// Whether the file fetches exactly one git ref (`rev` or `tag`) and binds
/// `repo = "<repo>";` (case-insensitive).
///
/// Release tags only describe the upstream repository's own source. A file
/// with several refs (e.g. pinned third-party data repositories next to the
/// main source), or whose fetcher names another repository, is ambiguous, and
/// its refs are left alone.
pub fn is_single_ref_from(content: &str, repo: &str) -> bool {
    if git_ref_count(content) != 1 {
        return false;
    }
    let Ok(repo_re) = Regex::new(&format!(
        r#"(?mi)^[ \t]*repo\s*=\s*"{}"\s*;"#,
        regex::escape(repo)
    )) else {
        return false;
    };
    repo_re.is_match(content)
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

    // With several refs, the first `rev` may belong to an unrelated fetcher.
    if git_ref_count(content) != 1 {
        return Err(RewriteError::attr_not_found(
            "rev fixup (file has several rev/tag attributes)",
        ));
    }

    let current_rev = extract_rev_value(content)?;

    if !is_likely_commit_sha(&current_rev) {
        return Err(RewriteError::attr_not_found("rev fixup (not a commit SHA)"));
    }

    let (tag_prefix, tag_suffix) = split_tag(tag_name, new_version)
        .ok_or_else(|| RewriteError::attr_not_found("rev fixup (version not found in tag name)"))?;
    if !is_plain_tag_text(tag_prefix) || !is_plain_tag_text(tag_suffix) {
        return Err(RewriteError::attr_not_found(
            "rev fixup (tag needs escaping in a Nix string)",
        ));
    }

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
    let new_rev_nix = format!("{tag_prefix}${{{version_ref}}}{tag_suffix}");

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

/// Whether text can be placed in a Nix string literal as is.
fn is_plain_tag_text(text: &str) -> bool {
    !text.contains(['"', '\\', '$'])
}

/// Rewrite the file's only git ref so it names the release's actual tag.
///
/// Projects sometimes change their tag scheme between releases (HDF5:
/// `hdf5_1.14.6` → `2.2.0`), so a ref built from the old scheme points at a tag
/// that doesn't exist. The literal text around the version is replaced and the
/// version reference is kept:
///
/// - `rev = "hdf5_${version}";` → `rev = "${version}";`
/// - `tag = "v${finalAttrs.version}";` → `tag = "release-${finalAttrs.version}";`
/// - `tag = version;` → `tag = "v${version}";`
/// - `rev = "v2.2.0";` → `rev = "2.2.0";`
///
/// A ref that already evaluates to the tag (also as `refs/tags/<tag>`) is
/// returned unchanged. Call this after the version has been bumped, and only
/// for files where [`is_single_ref_from`] holds.
///
/// # Errors
/// [`RewriteError::NotFound`] when the file doesn't have exactly one ref, the
/// ref doesn't derive from the version (commit SHAs, other interpolations), or
/// the version isn't part of the tag name.
pub fn align_ref_with_tag(content: &str, tag_name: &str, new_version: &str) -> Result<String> {
    static BINDING: OnceLock<Regex> = OnceLock::new();
    let binding = BINDING.get_or_init(|| {
        Regex::new(r#"(?m)^([ \t]*(?:rev|tag)\s*=\s*)("[^"]*"|[A-Za-z_][\w.]*)(\s*;)"#)
            .expect("valid regex")
    });

    if git_ref_count(content) != 1 {
        return Err(RewriteError::attr_not_found(
            "ref alignment (file has several rev/tag attributes)",
        ));
    }
    let caps = binding
        .captures(content)
        .ok_or_else(|| RewriteError::attr_not_found("ref alignment (rev/tag value)"))?;
    let value = caps.get(2).expect("group").as_str();

    // (literal prefix, version reference if interpolated, literal suffix)
    let (prefix, version_ref, suffix) = match value.strip_prefix('"') {
        None if is_version_ref(value) => ("", Some(value), ""),
        None => {
            return Err(RewriteError::attr_not_found(
                "ref alignment (not derived from the version)",
            ));
        },
        Some(quoted) => {
            let inner = quoted.strip_suffix('"').unwrap_or(quoted);
            if let Some((before, rest)) = inner.split_once("${") {
                let (reference, after) = rest.split_once('}').ok_or_else(|| {
                    RewriteError::attr_not_found("ref alignment (unterminated interpolation)")
                })?;
                if !is_version_ref(reference.trim()) || after.contains("${") {
                    return Err(RewriteError::attr_not_found(
                        "ref alignment (not derived from the version)",
                    ));
                }
                (before, Some(reference.trim()), after)
            } else if is_likely_commit_sha(inner) {
                return Err(RewriteError::attr_not_found("ref alignment (commit SHA)"));
            } else {
                split_tag(inner, new_version).map_or_else(
                    || {
                        Err(RewriteError::attr_not_found(
                            "ref alignment (version not in rev)",
                        ))
                    },
                    |(before, after)| Ok((before, None, after)),
                )?
            }
        },
    };

    let current = format!("{prefix}{new_version}{suffix}");
    if current == tag_name || current.strip_prefix("refs/tags/") == Some(tag_name) {
        return Ok(content.to_owned());
    }

    let (tag_prefix, tag_suffix) = split_tag(tag_name, new_version)
        .ok_or_else(|| RewriteError::attr_not_found("ref alignment (version not in tag)"))?;
    if !is_plain_tag_text(tag_prefix) || !is_plain_tag_text(tag_suffix) {
        return Err(RewriteError::attr_not_found(
            "ref alignment (tag needs escaping in a Nix string)",
        ));
    }
    let refs = if prefix.starts_with("refs/tags/") {
        "refs/tags/"
    } else {
        ""
    };
    let new_value = match version_ref {
        Some(reference) => format!("\"{refs}{tag_prefix}${{{reference}}}{tag_suffix}\""),
        None => format!("\"{refs}{tag_name}\""),
    };

    let whole = caps.get(0).expect("match");
    let result = format!(
        "{}{}{new_value}{}{}",
        &content[..whole.start()],
        &caps[1],
        &caps[3],
        &content[whole.end()..]
    );
    if !rnix::Root::parse(&result).errors().is_empty() {
        return Err(RewriteError::InvalidResult {
            operation: "ref alignment",
        });
    }
    Ok(result)
}

fn is_version_ref(expr: &str) -> bool {
    matches!(expr, "version" | "finalAttrs.version")
}
