use std::sync::OnceLock;

use regex::Regex;

use super::error::{Result, RewriteError};

/// `fetchPypi`'s default `extension`.
const DEFAULT_EXTENSION: &str = "tar.gz";

/// `fetchPypi`'s default wheel `python` tag and `dist` directory. They default
/// independently, so a wheel whose tag changes needs both set.
const DEFAULT_WHEEL_TAG: &str = "py2.py3";

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
    rewrite_fetch_pypi(
        content,
        pname,
        &[("extension", extension, DEFAULT_EXTENSION)],
    )
}

/// Set the `pname`, `python` and/or `dist` passed to the file's wheel
/// `fetchPypi` call (`format = "wheel";`)
///
/// Used when a release's wheel filename no longer matches the call, typically
/// because the python tag changed (`py2.py3` → `py3`). `pname` is handled as in
/// [`update_fetch_pypi_sdist`]. `python` and `dist` replace an existing string
/// attribute or add one; setting `fetchPypi`'s default (`py2.py3`) removes it.
///
/// # Errors
/// As [`update_fetch_pypi_sdist`], plus [`RewriteError::Structural`] when
/// `python` or `dist` is bound to something other than a string literal (e.g.
/// `dist = python;`) or inherited.
///
/// # Example
/// ```
/// use ekapkgs_update::rewrite::update_fetch_pypi_wheel;
///
/// let content = r#"{
///   src = fetchPypi {
///     inherit pname version;
///     format = "wheel";
///     hash = "sha256-abc";
///   };
/// }"#;
/// let updated = update_fetch_pypi_wheel(content, None, Some("py3"), Some("py3")).unwrap();
/// assert!(updated.contains(r#"python = "py3";"#));
/// assert!(updated.contains(r#"dist = "py3";"#));
/// ```
pub fn update_fetch_pypi_wheel(
    content: &str,
    pname: Option<&str>,
    python: Option<&str>,
    dist: Option<&str>,
) -> Result<String> {
    rewrite_fetch_pypi(
        content,
        pname,
        &[
            ("python", python, DEFAULT_WHEEL_TAG),
            ("dist", dist, DEFAULT_WHEEL_TAG),
        ],
    )
}

/// Rewrite the only `fetchPypi` call: set `pname`, then each
/// `(attribute, value, fetchPypi default)` whose value is `Some`.
fn rewrite_fetch_pypi(
    content: &str,
    pname: Option<&str>,
    attrs: &[(&str, Option<&str>, &str)],
) -> Result<String> {
    ensure_parses(content)?;

    let (body_start, body_end) = find_fetch_pypi_body(content)?;
    let mut body = content[body_start..body_end].to_owned();
    let indent = body_indent(&body);

    if let Some(pname) = pname {
        body = set_pname(&body, pname)?;
    }
    for &(name, value, default) in attrs {
        if let Some(value) = value {
            body = set_string_attr(&body, name, value, default, &indent)?;
        }
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

/// Set the string attribute `name` in a `fetchPypi` body: replace an existing
/// `name = "…";`, or add one after the last attribute. Setting `default`
/// removes the attribute instead.
fn set_string_attr(
    body: &str,
    name: &str,
    value: &str,
    default: &str,
    indent: &str,
) -> Result<String> {
    let name_re = regex::escape(name);
    let literal = Regex::new(&format!(
        r#"(?m)^([ \t]*){name_re}\s*=\s*"[^"]*"\s*;[ \t]*\n?"#
    ))?;

    if let Some(caps) = literal.captures(body) {
        let whole = caps.get(0).expect("match");
        let replacement = if value == default {
            String::new()
        } else {
            let newline = if whole.as_str().ends_with('\n') {
                "\n"
            } else {
                ""
            };
            format!("{}{name} = \"{value}\";{newline}", &caps[1])
        };
        return Ok(format!(
            "{}{replacement}{}",
            &body[..whole.start()],
            &body[whole.end()..]
        ));
    }

    // Any other binding (`dist = python;`, `inherit … dist;`) would be
    // duplicated by adding one, which Nix rejects at evaluation time.
    let other_binding = Regex::new(&format!(r"(?m)^[ \t]*{name_re}\s*="))?;
    let inherited = Regex::new(&format!(r"\binherit\b[^;]*\b{name_re}\b[^;]*;"))?;
    if other_binding.is_match(body) || inherited.is_match(body) {
        return Err(RewriteError::Structural(format!(
            "fetchPypi's {name} is not a string literal; refusing to rewrite it"
        )));
    }

    if value == default {
        return Ok(body.to_owned());
    }

    // Add it after the last attribute, before the closing brace's line.
    let trimmed = body.trim_end();
    Ok(format!(
        "{trimmed}\n{indent}{name} = \"{value}\";{}",
        &body[trimmed.len()..]
    ))
}
