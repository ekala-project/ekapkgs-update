//! Lint a Nix package expression for whitespace formatting compliance.
//!
//! Checks three rules from the best-practices skill:
//! 1. Header attributes (`pname`, `version`, `extensions`, `pyproject`, `disabled`)
//!    must be grouped at the top of the attrset with no blank lines between them.
//! 2. All other attributes must be separated by a blank line.
//! 3. `meta` must be the last attribute.

use std::fmt;

use anyhow::{Context, Result, bail};
use regex::Regex;

/// Attributes that form the "header group" — allowed together without blank lines.
const HEADER_ATTRS: &[&str] = &["pname", "version", "format", "extensions", "pyproject", "disabled"];


// ── Types ────────────────────────────────────────────────────────────────────

/// A parsed attribute within the derivation attrset.
#[derive(Debug, Clone)]
pub(crate) struct AttrSpan {
    /// Attribute name (e.g. "pname", "src", "meta").
    pub(crate) name: String,
    /// 1-based line number where the attribute starts in the original file.
    pub(crate) start_line: usize,
    /// The full text of the attribute, from the first non-blank character of the
    /// attribute (including leading comments) through the terminating `;`.
    #[allow(dead_code)]
    pub(crate) text: String,
    /// Number of blank lines between the end of the previous attribute (or the
    /// opening brace) and the start of this attribute.
    pub(crate) blank_lines_before: usize,
}

/// A lint violation.
#[derive(Debug)]
pub(crate) struct LintViolation {
    pub(crate) line: usize,
    pub(crate) rule: LintRule,
    pub(crate) message: String,
}

#[derive(Debug)]
pub(crate) enum LintRule {
    HeaderGrouping,
    BlankLineSeparation,
    MetaAtEnd,
}

impl fmt::Display for LintViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = match self.rule {
            LintRule::HeaderGrouping => "header-group",
            LintRule::BlankLineSeparation => "blank-line",
            LintRule::MetaAtEnd => "meta-at-end",
        };
        write!(f, "line {}: [{}] {}", self.line, tag, self.message)
    }
}

// ── Entry point ──────────────────────────────────────────────────────────────

pub async fn lint(path: String, fix: bool, no_ignore: bool) -> Result<()> {
    let meta = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("reading {path}"))?;

    if meta.is_dir() {
        lint_directory(&path, fix, no_ignore).await
    } else {
        lint_file(&path, fix).await
    }
}

async fn lint_directory(dir: &str, fix: bool, no_ignore: bool) -> Result<()> {
    let mut entries = Vec::new();
    collect_nix_files(dir, &mut entries, no_ignore).await?;
    entries.sort();

    let mut set = tokio::task::JoinSet::new();
    for file in entries {
        set.spawn(async move {
            let result = lint_file_inner(&file, fix).await;
            (file, result)
        });
    }

    let mut has_violations = false;
    while let Some(join_result) = set.join_next().await {
        let (file, result) = join_result?;
        match result {
            Ok(had_violations) => {
                if had_violations {
                    has_violations = true;
                }
            },
            Err(e) => {
                tracing::warn!("{file}: {e:#}");
            },
        }
    }

    if has_violations {
        std::process::exit(1);
    }

    Ok(())
}

/// Recursively collect all `.nix` files under `dir`.
/// Skips directories whose name starts with `.` unless `no_ignore` is true.
fn collect_nix_files<'a>(
    dir: &'a str,
    out: &'a mut Vec<String>,
    no_ignore: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let mut read_dir = tokio::fs::read_dir(dir)
            .await
            .with_context(|| format!("reading directory {dir}"))?;

        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                if !no_ignore {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with('.') {
                            continue;
                        }
                    }
                }
                collect_nix_files(path.to_string_lossy().as_ref(), out, no_ignore).await?;
            } else if file_type.is_file() || file_type.is_symlink() {
                if let Some(ext) = path.extension() {
                    if ext == "nix" {
                        out.push(path.to_string_lossy().into_owned());
                    }
                }
            }
        }

        Ok(())
    })
}

async fn lint_file(file: &str, fix: bool) -> Result<()> {
    let had_violations = lint_file_inner(file, fix).await?;
    if had_violations {
        std::process::exit(1);
    }
    Ok(())
}

/// Lint a single file. Returns `Ok(true)` if violations were found (check mode only).
async fn lint_file_inner(file: &str, fix: bool) -> Result<bool> {
    let content = tokio::fs::read_to_string(file)
        .await
        .with_context(|| format!("reading {file}"))?;

    // Validate that the file parses as valid Nix.
    let parse = rnix::Root::parse(&content);
    if !parse.errors().is_empty() {
        let errors: Vec<String> = parse
            .errors()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        bail!("{file}: Nix parse error: {}", errors.join(", "));
    }

    // Find the derivation attrset.
    let Some(attrset_range) = find_derivation_attrset(&content) else {
        return Ok(false);
    };

    // Parse individual attributes within the attrset body.
    let body = &content[attrset_range.body_start..attrset_range.body_end];
    let body_start_line = content[..attrset_range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);

    if attrs.is_empty() {
        return Ok(false);
    }

    if fix {
        let fixed = apply_fix(&content, &attrset_range, &attrs);
        // Validate that the fixed content still parses.
        let fix_parse = rnix::Root::parse(&fixed);
        if !fix_parse.errors().is_empty() {
            bail!(
                "{file}: fix produced invalid Nix — refusing to write (this is a bug in the linter)"
            );
        }
        if fixed != content {
            tokio::fs::write(file, &fixed)
                .await
                .with_context(|| format!("writing {file}"))?;
            tracing::info!("{file}: fixed");
        }
        Ok(false)
    } else {
        let violations = check_rules(&attrs);
        if violations.is_empty() {
            return Ok(false);
        }
        for v in &violations {
            eprintln!("{file}:{v}");
        }
        Ok(true)
    }
}

// ── Public API for programmatic use ──────────────────────────────────────────

/// Check a Nix file's content for lint violations. Returns `None` if the file
/// is not a simple derivation expression, or `Some(violations)` otherwise.
pub fn lint_content(content: &str) -> Option<Vec<String>> {
    let range = find_derivation_attrset(content)?;
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    if attrs.is_empty() {
        return Some(Vec::new());
    }
    let violations = check_rules(&attrs);
    Some(violations.iter().map(ToString::to_string).collect())
}

/// Fix whitespace in a Nix file's content. Returns `None` if the file is not a
/// simple derivation expression, or `Some(fixed_content)` if fixes were applied
/// (returns the original content unchanged if already compliant).
pub fn fix_content(content: &str) -> anyhow::Result<Option<String>> {
    let Some(range) = find_derivation_attrset(content) else {
        return Ok(None);
    };
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    if attrs.is_empty() {
        return Ok(Some(content.to_owned()));
    }
    let fixed = apply_fix(content, &range, &attrs);

    // Validate the result.
    let parse = rnix::Root::parse(&fixed);
    if !parse.errors().is_empty() {
        bail!("lint fix produced invalid Nix (this is a bug in the linter)");
    }

    Ok(Some(fixed))
}

// ── Derivation detection ─────────────────────────────────────────────────────

/// Byte range of the derivation's outer attrset.
pub(crate) struct AttrsetRange {
    /// Byte offset of the `{` that opens the attrset body.
    pub(crate) open_brace: usize,
    /// Byte offset of the first character after `{\n` (the body content start).
    pub(crate) body_start: usize,
    /// Byte offset of the `}` that closes the attrset body.
    pub(crate) body_end: usize,
}

/// Find an attrset containing both `pname` and `version` (directly or via
/// `inherit`). Scans all `{…}` blocks in the file and returns the first match.
pub(crate) fn find_derivation_attrset(content: &str) -> Option<AttrsetRange> {
    let bytes = content.as_bytes();
    let attr_re = Regex::new(r"^[ \t]*([\w][\w.-]*)\s*=").unwrap();
    let mut search_from = 0;

    while search_from < bytes.len() {
        // Find the next `{` outside strings/comments.
        let Some(open_brace) = find_next_open_brace(content, search_from) else {
            break;
        };

        let Some(close_brace) = find_matching_brace(content, open_brace) else {
            break;
        };

        // Advance past this block for the next iteration.
        search_from = open_brace + 1;

        let mut body_start = open_brace + 1;
        if bytes.get(body_start) == Some(&b'\n') {
            body_start += 1;
        }

        let body = &content[body_start..close_brace];
        let stmts = split_top_level_statements(body);

        let mut has_pname = false;
        let mut has_version = false;

        for stmt in &stmts {
            let trimmed = stmt.text.trim_start();

            // Check inherit statements for pname/version.
            if trimmed.starts_with("inherit ") || trimmed.starts_with("inherit\n") {
                if trimmed.contains("pname") {
                    has_pname = true;
                }
                if trimmed.contains("version") {
                    has_version = true;
                }
            } else if let Some(caps) = attr_re.captures(trimmed) {
                match &caps[1] {
                    "pname" => has_pname = true,
                    "version" => has_version = true,
                    _ => {},
                }
            }

            if has_pname && has_version {
                return Some(AttrsetRange {
                    open_brace,
                    body_start,
                    body_end: close_brace,
                });
            }
        }
    }

    None
}

/// Find the next `{` at the top level (outside strings and comments) starting
/// from `start`.
fn find_next_open_brace(content: &str, start: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut in_string = false;
    let mut in_multiline_string = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut escape_next = false;
    let mut i = start;

    while i < len {
        let ch = bytes[i];

        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }
        if in_line_comment {
            if ch == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if ch == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if in_string {
            match ch {
                b'\\' => escape_next = true,
                b'"' => in_string = false,
                _ => {},
            }
            i += 1;
            continue;
        }
        if in_multiline_string {
            if ch == b'\'' && i + 1 < len && bytes[i + 1] == b'\'' {
                if i + 2 < len
                    && (bytes[i + 2] == b'$' || bytes[i + 2] == b'\\' || bytes[i + 2] == b'\'')
                {
                    i += 3;
                    continue;
                }
                in_multiline_string = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        match ch {
            b'"' => in_string = true,
            b'\'' if i + 1 < len && bytes[i + 1] == b'\'' => {
                in_multiline_string = true;
                i += 2;
                continue;
            },
            b'#' => in_line_comment = true,
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                in_block_comment = true;
                i += 2;
                continue;
            },
            b'{' => return Some(i),
            _ => {},
        }
        i += 1;
    }
    None
}

/// Find the matching `}` for a `{` at `start_pos`, handling strings, comments,
/// and nested braces.
pub(crate) fn find_matching_brace(content: &str, start_pos: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut in_multiline_string = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut escape_next = false;
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = start_pos;

    while i < len {
        let ch = bytes[i];

        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }

        // Line comment: # until newline
        if in_line_comment {
            if ch == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }

        // Block comment: /* until */
        if in_block_comment {
            if ch == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        // Double-quoted string
        if in_string {
            match ch {
                b'\\' => escape_next = true,
                b'"' => in_string = false,
                _ => {},
            }
            i += 1;
            continue;
        }

        // Multi-line string ('' ... '')
        if in_multiline_string {
            if ch == b'\'' && i + 1 < len && bytes[i + 1] == b'\'' {
                // Check for escape sequences: ''$ ''\ '''
                if i + 2 < len
                    && (bytes[i + 2] == b'$' || bytes[i + 2] == b'\\' || bytes[i + 2] == b'\'')
                {
                    i += 3; // skip the escape
                    continue;
                }
                // End of multi-line string
                in_multiline_string = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        // Outside strings/comments
        match ch {
            b'"' => in_string = true,
            b'\'' if i + 1 < len && bytes[i + 1] == b'\'' => {
                in_multiline_string = true;
                i += 2;
                continue;
            },
            b'#' => in_line_comment = true,
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                in_block_comment = true;
                i += 2;
                continue;
            },
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            },
            _ => {},
        }

        i += 1;
    }

    None
}

// ── Attribute parsing ────────────────────────────────────────────────────────

/// Parse individual attributes from the attrset body text.
///
/// Returns a list of `AttrSpan`s in order of appearance. `inherit` statements
/// and pure-comment blocks are skipped (they don't participate in lint rules).
pub(crate) fn parse_attributes(body: &str, body_start_line: usize) -> Vec<AttrSpan> {
    let mut attrs = Vec::new();
    let attr_start_re = Regex::new(r"^[ \t]*([\w][\w.-]*)\s*=").unwrap();

    let statements = split_top_level_statements(body);

    for stmt in &statements {
        let trimmed = stmt.text.trim_start();
        if trimmed.starts_with("inherit ") || trimmed.starts_with("inherit\n") {
            continue;
        }

        if let Some(caps) = attr_start_re.captures(trimmed) {
            let name = caps[1].to_string();
            let blank_lines = count_blank_lines_in(stmt.leading_whitespace);

            attrs.push(AttrSpan {
                name,
                start_line: body_start_line + stmt.line_offset,
                text: stmt.text.to_owned(),
                blank_lines_before: blank_lines,
            });
        }
    }

    attrs
}

/// A classified top-level statement: either a named attribute or an "other"
/// statement (inherit, comment block, etc.) that must be preserved as-is.
#[derive(Debug, Clone)]
enum BodyStatement {
    /// A named attribute (e.g. `pname = "x";`).
    Attr {
        name: String,
        text: String,
    },
    /// Anything else: `inherit`, standalone comments, etc.
    Other {
        text: String,
    },
}

/// Parse all top-level statements in the body, preserving non-attribute content.
fn parse_all_statements(body: &str) -> Vec<BodyStatement> {
    let attr_re = Regex::new(r"^[ \t]*([\w][\w.-]*)\s*=").unwrap();
    let statements = split_top_level_statements(body);
    let mut result = Vec::new();

    for stmt in &statements {
        let trimmed = stmt.text.trim_start();
        let is_inherit =
            trimmed.starts_with("inherit ") || trimmed.starts_with("inherit\n");

        if !is_inherit {
            if let Some(caps) = attr_re.captures(trimmed) {
                result.push(BodyStatement::Attr {
                    name: caps[1].to_string(),
                    text: stmt.text.to_owned(),
                });
                continue;
            }
        }

        result.push(BodyStatement::Other {
            text: stmt.text.to_owned(),
        });
    }

    result
}

/// A raw statement extracted from the attrset body.
struct RawStatement<'a> {
    /// The whitespace/comments between the previous statement's `;` and the
    /// first non-whitespace character of this statement.
    leading_whitespace: &'a str,
    /// The full text from the first non-whitespace char through the `;`.
    text: &'a str,
    /// 0-based line offset from the start of the body.
    line_offset: usize,
}

/// Check if position `i` in `bytes` starts with `keyword` followed by a
/// non-identifier character (or EOF), and is preceded by a non-identifier
/// character (or start of input). This prevents matching `letterbox` as `let`.
fn is_keyword_at(bytes: &[u8], i: usize, keyword: &[u8]) -> bool {
    let len = bytes.len();
    if i + keyword.len() > len {
        return false;
    }
    if &bytes[i..i + keyword.len()] != keyword {
        return false;
    }
    // Check word boundary after keyword
    let after = i + keyword.len();
    if after < len
        && (bytes[after].is_ascii_alphanumeric() || bytes[after] == b'_' || bytes[after] == b'-')
    {
        return false;
    }
    // Check word boundary before keyword
    if i > 0
        && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'-')
    {
        return false;
    }
    true
}

/// Split the attrset body into top-level statements (delimited by `;` at
/// depth 0, outside strings and comments).
///
/// Tracks `let`/`in` nesting so that `;` inside `let ... in` blocks is not
/// treated as an attribute terminator.
fn split_top_level_statements(body: &str) -> Vec<RawStatement<'_>> {
    let mut statements = Vec::new();
    let bytes = body.as_bytes();
    let len = bytes.len();

    let mut brace_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;
    let mut paren_depth: i32 = 0;
    let mut let_depth: i32 = 0;
    let mut with_depth: i32 = 0;
    let mut in_string = false;
    let mut in_multiline_string = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut escape_next = false;

    // Track where the current inter-statement gap started and where the
    // current statement's content started.
    let mut gap_start: usize = 0; // byte offset of the gap after last `;`
    let mut stmt_content_start: Option<usize> = None;

    let mut i: usize = 0;
    while i < len {
        let ch = bytes[i];

        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }

        if in_line_comment {
            if ch == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }

        if in_block_comment {
            if ch == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if in_string {
            match ch {
                b'\\' => escape_next = true,
                b'"' => in_string = false,
                _ => {},
            }
            i += 1;
            continue;
        }

        if in_multiline_string {
            if ch == b'\'' && i + 1 < len && bytes[i + 1] == b'\'' {
                if i + 2 < len
                    && (bytes[i + 2] == b'$' || bytes[i + 2] == b'\\' || bytes[i + 2] == b'\'')
                {
                    i += 3;
                    continue;
                }
                in_multiline_string = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        // Outside strings and comments — check for keywords first.
        // `let` increments nesting (`;` inside let blocks is not an attr terminator).
        if ch == b'l' && is_keyword_at(bytes, i, b"let") {
            let_depth += 1;
            if stmt_content_start.is_none() {
                stmt_content_start = Some(i);
            }
            i += 3;
            continue;
        }
        if ch == b'i' && let_depth > 0 && is_keyword_at(bytes, i, b"in") {
            let_depth -= 1;
            i += 2;
            continue;
        }
        // `with expr;` — the `;` ends the with clause, not the attribute.
        // Only track `with` at the top level; inside nested braces the `;`
        // is already ignored because brace_depth > 0.
        if ch == b'w'
            && brace_depth == 0
            && bracket_depth == 0
            && paren_depth == 0
            && is_keyword_at(bytes, i, b"with")
        {
            with_depth += 1;
            if stmt_content_start.is_none() {
                stmt_content_start = Some(i);
            }
            i += 4;
            continue;
        }

        match ch {
            b'"' => {
                in_string = true;
                if stmt_content_start.is_none() {
                    stmt_content_start = Some(i);
                }
            },
            b'\'' if i + 1 < len && bytes[i + 1] == b'\'' => {
                in_multiline_string = true;
                if stmt_content_start.is_none() {
                    stmt_content_start = Some(i);
                }
                i += 2;
                continue;
            },
            b'#' => {
                in_line_comment = true;
                // Comments before a statement are part of the leading gap
                // (they'll be captured as part of the statement text later
                // when we find the actual attr start).
                if stmt_content_start.is_none() {
                    stmt_content_start = Some(i);
                }
            },
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                in_block_comment = true;
                if stmt_content_start.is_none() {
                    stmt_content_start = Some(i);
                }
                i += 2;
                continue;
            },
            b'{' => {
                brace_depth += 1;
                if stmt_content_start.is_none() {
                    stmt_content_start = Some(i);
                }
            },
            b'}' => {
                brace_depth -= 1;
                if stmt_content_start.is_none() {
                    stmt_content_start = Some(i);
                }
            },
            b'[' => {
                bracket_depth += 1;
                if stmt_content_start.is_none() {
                    stmt_content_start = Some(i);
                }
            },
            b']' => {
                bracket_depth -= 1;
                if stmt_content_start.is_none() {
                    stmt_content_start = Some(i);
                }
            },
            b'(' => {
                paren_depth += 1;
                if stmt_content_start.is_none() {
                    stmt_content_start = Some(i);
                }
            },
            b')' => {
                paren_depth -= 1;
                if stmt_content_start.is_none() {
                    stmt_content_start = Some(i);
                }
            },
            // `with expr;` — the `;` terminates the with-clause, not the
            // attribute. Consume it and decrement with_depth.
            b';' if with_depth > 0
                && brace_depth == 0
                && bracket_depth == 0
                && paren_depth == 0
                && let_depth == 0 =>
            {
                with_depth -= 1;
                i += 1;
                continue;
            },
            b';' if brace_depth == 0
                && bracket_depth == 0
                && paren_depth == 0
                && let_depth == 0
                && with_depth == 0 =>
            {
                // End of a top-level statement. Include any trailing
                // comment on the same line (e.g. `foo = bar; # comment`).
                let mut end = i;
                let mut j = i + 1;
                while j < len && bytes[j] != b'\n' {
                    if bytes[j] == b'#' {
                        // Include from `#` through end of line.
                        while j < len && bytes[j] != b'\n' {
                            j += 1;
                        }
                        end = j.saturating_sub(1);
                        break;
                    }
                    j += 1;
                }

                if let Some(content_start) = stmt_content_start {
                    let leading_ws = &body[gap_start..content_start];
                    let text = &body[content_start..=end];
                    let line_offset = body[..content_start].matches('\n').count();

                    statements.push(RawStatement {
                        leading_whitespace: leading_ws,
                        text,
                        line_offset,
                    });
                }
                gap_start = end + 1;
                stmt_content_start = None;
                i = end + 1;
                continue;
            },
            _ if !ch.is_ascii_whitespace() && stmt_content_start.is_none() => {
                stmt_content_start = Some(i);
            },
            _ => {},
        }

        i += 1;
    }

    statements
}

/// Count the number of blank lines in a whitespace/comment region.
/// A "blank line" is a line that contains only whitespace.
fn count_blank_lines_in(text: &str) -> usize {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= 1 {
        return 0;
    }
    // Skip the first partial line (rest of previous line after `;`) and the
    // last partial line (indentation before the next attr). Count fully empty
    // lines in between.
    let mut blank_count = 0;
    for line in &lines[1..lines.len().saturating_sub(1)] {
        if line.trim().is_empty() {
            blank_count += 1;
        }
    }
    blank_count
}

// ── Lint rules ───────────────────────────────────────────────────────────────

pub(crate) fn check_rules(attrs: &[AttrSpan]) -> Vec<LintViolation> {
    let mut violations = Vec::new();

    check_header_grouping(attrs, &mut violations);
    check_blank_line_separation(attrs, &mut violations);
    check_meta_at_end(attrs, &mut violations);

    violations.sort_by_key(|v| v.line);
    violations
}

fn is_header_attr(name: &str) -> bool {
    HEADER_ATTRS.contains(&name)
}

/// Rule 1: Header attributes must be contiguous from the top with no blank lines.
fn check_header_grouping(attrs: &[AttrSpan], violations: &mut Vec<LintViolation>) {
    // Find the index where header attrs end (first non-header attr).
    let mut header_ended = false;

    for (i, attr) in attrs.iter().enumerate() {
        if is_header_attr(&attr.name) {
            if header_ended {
                violations.push(LintViolation {
                    line: attr.start_line,
                    rule: LintRule::HeaderGrouping,
                    message: format!(
                        "'{}' should be in the header group at the top of the attrset",
                        attr.name
                    ),
                });
            } else if i > 0 && attr.blank_lines_before > 0 && is_header_attr(&attrs[i - 1].name) {
                violations.push(LintViolation {
                    line: attr.start_line,
                    rule: LintRule::HeaderGrouping,
                    message: format!(
                        "unexpected blank line within header group (before '{}')",
                        attr.name
                    ),
                });
            }
        } else {
            header_ended = true;
        }
    }
}

/// Rule 2: Non-header, non-meta attributes must be separated by a blank line.
/// The first attribute after the header group must also have a blank line before it.
fn check_blank_line_separation(attrs: &[AttrSpan], violations: &mut Vec<LintViolation>) {
    // Find where the header group ends.
    let header_end = attrs
        .iter()
        .position(|a| !is_header_attr(&a.name))
        .unwrap_or(attrs.len());

    for (i, attr) in attrs.iter().enumerate().skip(header_end) {
        if attr.blank_lines_before == 0 && i > 0 {
            violations.push(LintViolation {
                line: attr.start_line,
                rule: LintRule::BlankLineSeparation,
                message: format!("missing blank line before '{}'", attr.name),
            });
        }
    }
}

/// Rule 3: `meta` must be the last attribute.
fn check_meta_at_end(attrs: &[AttrSpan], violations: &mut Vec<LintViolation>) {
    if let Some(meta_pos) = attrs.iter().position(|a| a.name == "meta") {
        if meta_pos != attrs.len() - 1 {
            violations.push(LintViolation {
                line: attrs[meta_pos].start_line,
                rule: LintRule::MetaAtEnd,
                message: "'meta' should be the last attribute in the attrset".to_owned(),
            });
        }
    }
}

// ── Fix logic ────────────────────────────────────────────────────────────────

/// Reconstruct the file with correct attribute ordering and whitespace.
///
/// Preserves all non-attribute statements (`inherit`, comments, etc.) in their
/// original positions. Only adjusts blank lines between statements and moves
/// header attrs to top / meta to end.
pub(crate) fn apply_fix(content: &str, range: &AttrsetRange, _attrs: &[AttrSpan]) -> String {
    let body = &content[range.body_start..range.body_end];
    let indent = detect_indent_from_body(body);
    let all_stmts = parse_all_statements(body);

    // Partition statements into groups, preserving order within each group.
    // "Other" statements (inherit, comments) stay attached to the group they
    // appear in: they precede the next named attribute and travel with it.
    let mut header_group: Vec<&BodyStatement> = Vec::new();
    let mut body_group: Vec<&BodyStatement> = Vec::new();
    let mut meta_group: Vec<&BodyStatement> = Vec::new();
    // Buffer for "Other" stmts that haven't been assigned to a group yet.
    let mut pending_others: Vec<&BodyStatement> = Vec::new();

    for stmt in &all_stmts {
        match stmt {
            BodyStatement::Other { text } => {
                // Inherit statements that contain header attr names belong
                // in the header group (e.g. `inherit version;`).
                let trimmed = text.trim_start();
                if (trimmed.starts_with("inherit ") || trimmed.starts_with("inherit\n"))
                    && HEADER_ATTRS.iter().any(|a| trimmed.contains(a))
                {
                    header_group.append(&mut pending_others);
                    header_group.push(stmt);
                } else {
                    pending_others.push(stmt);
                }
            },
            BodyStatement::Attr { name, .. } => {
                if name == "meta" {
                    meta_group.append(&mut pending_others);
                    meta_group.push(stmt);
                } else if is_header_attr(name) {
                    header_group.append(&mut pending_others);
                    header_group.push(stmt);
                } else {
                    body_group.append(&mut pending_others);
                    body_group.push(stmt);
                }
            },
        }
    }
    // Any trailing "Other" stmts (after the last attr) go into body.
    body_group.append(&mut pending_others);

    // Emit the ordered body: header → body → meta, with correct blank lines.
    let mut new_body = String::new();

    for stmt in &header_group {
        // Header group: no blank lines between statements.
        emit_statement(&mut new_body, stmt, &indent, false);
    }

    for stmt in &body_group {
        // Body group: blank line before each statement.
        let needs_blank = !new_body.is_empty();
        emit_statement(&mut new_body, stmt, &indent, needs_blank);
    }

    for stmt in &meta_group {
        let needs_blank = !new_body.is_empty();
        emit_statement(&mut new_body, stmt, &indent, needs_blank);
    }

    let prefix = &content[..=range.open_brace];
    let suffix = &content[range.body_end..];

    // Preserve the indentation of the closing `}` from the original body.
    let closing_indent = body
        .rfind('\n')
        .map_or("", |pos| {
            let after_nl = &body[pos + 1..];
            let ws_len = after_nl.len() - after_nl.trim_start().len();
            &after_nl[..ws_len]
        });

    format!("{prefix}\n{new_body}{closing_indent}{suffix}")
}

/// Emit a single statement into the body buffer.
///
/// - `needs_blank_before`: if true, ensure exactly one blank line before this
///   statement. If false (header group), no blank line.
fn emit_statement(buf: &mut String, stmt: &BodyStatement, indent: &str, needs_blank: bool) {
    let text = match stmt {
        BodyStatement::Attr { text, .. } | BodyStatement::Other { text, .. } => text,
    };

    if needs_blank && !buf.is_empty() {
        if !buf.ends_with("\n\n") {
            if !buf.ends_with('\n') {
                buf.push('\n');
            }
            buf.push('\n');
        }
    } else if !buf.is_empty() && !buf.ends_with('\n') {
        buf.push('\n');
    }
    buf.push_str(&normalize_attr_text(text, indent));
    buf.push('\n');
}

/// Normalize the indentation of an attribute text block.
/// Ensures the first line has the correct indent, and subsequent lines preserve
/// their relative indentation.
fn normalize_attr_text(text: &str, indent: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return String::new();
    }

    let mut result = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i == 0 {
            result.push_str(indent);
            result.push_str(line.trim_start());
        } else {
            result.push('\n');
            result.push_str(line);
        }
    }
    result
}

/// Detect the indentation string from a body region.
fn detect_indent_from_body(body: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim_start();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            let indent_len = line.len() - trimmed.len();
            return line[..indent_len].to_owned();
        }
    }
    "  ".to_owned()
}
