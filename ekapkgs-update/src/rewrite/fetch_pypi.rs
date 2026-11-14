use std::sync::OnceLock;

use regex::Regex;

use super::error::{Result, RewriteError};

/// `fetchPypi`'s default `extension`.
const DEFAULT_EXTENSION: &str = "tar.gz";

/// Set the `pname` and/or `extension` passed to the file's `fetchPypi` call
///
/// Used when a release's sdist filename no longer matches what `fetchPypi`
/// derives from the package's `pname` (e.g. `sphinxcontrib-openapi` →
/// `sphinxcontrib_openapi`).
///
/// - `pname`: an existing `pname = "…";` in the call is replaced. Otherwise
///   `pname` is taken out of the call's `inherit` and set explicitly, so
///   `inherit (finalAttrs) pname version;` becomes
///   `pname = "…"; inherit (finalAttrs) version;`.
/// - `extension`: replaces an existing `extension = "…";`, or adds one. Setting
///   the default (`tar.gz`) removes the attribute instead.
///
/// `fetchPyPi` (capital P) is matched too.
///
/// # Errors
/// Returns a [`RewriteError`] if:
/// - [`RewriteError::Parse`] - the file has invalid Nix syntax
/// - [`RewriteError::NotFound`] - there is no `fetchPypi` call, or `pname` is
///   neither set nor inherited in it
/// - [`RewriteError::Structural`] - there is more than one `fetchPypi` call, or
///   its braces are unbalanced
/// - [`RewriteError::InvalidResult`] - the rewrite would produce invalid syntax
///
/// # Example
/// ```
/// use ekapkgs_update::rewrite::update_fetch_pypi_sdist;
///
/// let content = r#"{
///   src = fetchPypi {
///     inherit (finalAttrs) pname version;
///     hash = "sha256-abc";
///   };
/// }"#;
/// let updated = update_fetch_pypi_sdist(content, Some("foo_bar"), None).unwrap();
/// assert!(updated.contains(r#"pname = "foo_bar";"#));
/// assert!(updated.contains("inherit (finalAttrs) version;"));
/// ```
pub fn update_fetch_pypi_sdist(
    content: &str,
    pname: Option<&str>,
    extension: Option<&str>,
) -> Result<String> {
    ensure_parses(content)?;

    let (body_start, body_end) = find_fetch_pypi_body(content)?;
    let mut body = content[body_start..body_end].to_owned();
    let indent = body_indent(&body);

    if let Some(pname) = pname {
        body = set_pname(&body, pname)?;
    }
    if let Some(extension) = extension {
        body = set_extension(&body, extension, &indent);
    }

    let result = format!("{}{body}{}", &content[..body_start], &content[body_end..]);
    if !rnix::Root::parse(&result).errors().is_empty() {
        return Err(RewriteError::InvalidResult {
            operation: "fetchPypi rewrite",
        });
    }
    Ok(result)
}

fn ensure_parses(content: &str) -> Result<()> {
    let parse = rnix::Root::parse(content);
    if parse.errors().is_empty() {
        return Ok(());
    }
    let errors: Vec<String> = parse
        .errors()
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    Err(RewriteError::Parse(errors.join(", ")))
}

/// Byte range of the text between the braces of the only `fetchPypi { … }`.
fn find_fetch_pypi_body(content: &str) -> Result<(usize, usize)> {
    static CALL: OnceLock<Regex> = OnceLock::new();
    let call = CALL.get_or_init(|| Regex::new(r"\bfetchPy[Pp]i\s*\{").expect("valid regex"));

    let mut calls = call.find_iter(content);
    let first = calls.next().ok_or_else(|| RewriteError::NotFound {
        what: "Call",
        name: "fetchPypi".to_owned(),
        context: None,
    })?;
    if calls.next().is_some() {
        return Err(RewriteError::Structural(
            "more than one fetchPypi call; refusing to guess which to rewrite".to_owned(),
        ));
    }

    // `${…}` interpolations are balanced, so counting every brace is enough.
    let body_start = first.end();
    let mut depth = 1usize;
    for (offset, c) in content[body_start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((body_start, body_start + offset));
                }
            },
            _ => {},
        }
    }
    Err(RewriteError::Structural(
        "unbalanced braces in fetchPypi call".to_owned(),
    ))
}

/// Indentation of the first non-blank line of the call's body.
fn body_indent(body: &str) -> String {
    body.lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line[..line.len() - line.trim_start().len()].to_owned())
        .unwrap_or_else(|| "    ".to_owned())
}

fn set_pname(body: &str, pname: &str) -> Result<String> {
    static PNAME: OnceLock<Regex> = OnceLock::new();
    static INHERIT: OnceLock<Regex> = OnceLock::new();
    let pname_re = PNAME.get_or_init(|| {
        Regex::new(r#"(?m)^([ \t]*)pname\s*=\s*"[^"]*"\s*;"#).expect("valid regex")
    });
    let inherit_re = INHERIT.get_or_init(|| {
        Regex::new(r"(?m)^([ \t]*)inherit(\s*\([^)]*\))?([^;]*);").expect("valid regex")
    });

    if let Some(caps) = pname_re.captures(body) {
        let whole = caps.get(0).expect("match");
        return Ok(format!(
            "{}{}pname = \"{pname}\";{}",
            &body[..whole.start()],
            &caps[1],
            &body[whole.end()..]
        ));
    }

    for caps in inherit_re.captures_iter(body) {
        let names: Vec<&str> = caps[3].split_whitespace().collect();
        if !names.contains(&"pname") {
            continue;
        }
        let whole = caps.get(0).expect("match");
        let indent = &caps[1];
        let rest: Vec<&str> = names.into_iter().filter(|n| *n != "pname").collect();
        let mut replacement = format!("{indent}pname = \"{pname}\";");
        if !rest.is_empty() {
            let from = caps.get(2).map_or("", |m| m.as_str());
            replacement.push_str(&format!("\n{indent}inherit{from} {};", rest.join(" ")));
        }
        return Ok(format!(
            "{}{replacement}{}",
            &body[..whole.start()],
            &body[whole.end()..]
        ));
    }

    Err(RewriteError::NotFound {
        what: "Attribute",
        name: "pname".to_owned(),
        context: Some("in fetchPypi call".to_owned()),
    })
}

fn set_extension(body: &str, extension: &str, indent: &str) -> String {
    static EXTENSION: OnceLock<Regex> = OnceLock::new();
    let extension_re = EXTENSION.get_or_init(|| {
        Regex::new(r#"(?m)^([ \t]*)extension\s*=\s*"[^"]*"\s*;[ \t]*\n?"#).expect("valid regex")
    });

    if let Some(caps) = extension_re.captures(body) {
        let whole = caps.get(0).expect("match");
        let replacement = if extension == DEFAULT_EXTENSION {
            String::new()
        } else {
            let newline = if whole.as_str().ends_with('\n') {
                "\n"
            } else {
                ""
            };
            format!("{}extension = \"{extension}\";{newline}", &caps[1])
        };
        return format!(
            "{}{replacement}{}",
            &body[..whole.start()],
            &body[whole.end()..]
        );
    }

    if extension == DEFAULT_EXTENSION {
        return body.to_owned();
    }

    // Add it after the last attribute, before the closing brace's line.
    let trimmed = body.trim_end();
    format!(
        "{trimmed}\n{indent}extension = \"{extension}\";{}",
        &body[trimmed.len()..]
    )
}
