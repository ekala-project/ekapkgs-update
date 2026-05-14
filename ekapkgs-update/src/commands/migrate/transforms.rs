use regex::Regex;
use tracing::debug;

/// Ensure doCheck = false; exists and is set to false
pub fn ensure_do_check_false(content: &str) -> anyhow::Result<String> {
    // Check if doCheck exists
    let do_check_pattern = Regex::new(r"(?m)^[ \t]*doCheck\s*=\s*([^;]+);")?;

    if let Some(caps) = do_check_pattern.captures(content) {
        let value = caps[1].trim();
        if value == "false" {
            // Already false, no change needed
            return Ok(content.to_owned());
        }

        // Change to false
        let result = do_check_pattern.replace(content, |caps: &regex::Captures<'_>| {
            let line = &caps[0];
            let indent = line.len() - line.trim_start().len();
            let indent_str = " ".repeat(indent);
            format!("{indent_str}doCheck = false;")
        });

        Ok(result.into_owned())
    } else {
        // doCheck doesn't exist, we'll add it when we find a good place
        // For now, we'll add it before passthru or meta, or after buildInputs
        add_do_check_false(content)
    }
}

/// Add doCheck = false; to the file
pub fn add_do_check_false(content: &str) -> anyhow::Result<String> {
    // Try to add before passthru
    let passthru_pattern = Regex::new(r"(?m)(^\s*)(passthru\s*=)")?;
    if let Some(caps) = passthru_pattern.captures(content) {
        let indent = &caps[1];
        let result = passthru_pattern.replace(
            content,
            format!(
                "{indent}# Test suite is quite long, run as passthru\n{indent}doCheck = \
                 false;\n\n{indent}passthru ="
            ),
        );
        return Ok(result.into_owned());
    }

    // Try to add before meta
    let meta_pattern = Regex::new(r"(?m)(^\s*)(meta\s*=)")?;
    if let Some(caps) = meta_pattern.captures(content) {
        let indent = &caps[1];
        let result = meta_pattern.replace(
            content,
            format!(
                "{indent}# Test suite is quite long, run as passthru\n{indent}doCheck = \
                 false;\n\n{indent}meta ="
            ),
        );
        return Ok(result.into_owned());
    }

    // Otherwise, return as is (we'll let the user handle this case)
    debug!("Could not find appropriate place to add doCheck = false;");
    Ok(content.to_owned())
}

/// Add unittests to passthru.tests
pub fn add_unittests_to_passthru(content: &str) -> anyhow::Result<String> {
    // Check if tests = { exists (simpler check)
    let tests_exists = Regex::new(r"(?m)^\s*tests\s*=\s*\{")?;

    if tests_exists.is_match(content) {
        // passthru.tests exists, add unittests to it
        let result = add_unittests_to_existing_tests(content)?;
        Ok(result)
    } else {
        // Check if passthru exists without tests
        let passthru_pattern = Regex::new(r"(?ms)(^\s*)(passthru\s*=\s*\{)")?;

        if passthru_pattern.is_match(content) {
            // passthru exists, add tests attribute
            add_tests_to_passthru(content)
        } else {
            // No passthru, create one
            add_passthru_with_tests(content)
        }
    }
}

/// Add unittests to existing passthru.tests
pub fn add_unittests_to_existing_tests(content: &str) -> anyhow::Result<String> {
    // Find the tests attribute and add unittests
    let tests_pattern = Regex::new(r"(?ms)(^\s*tests\s*=\s*\{)(.*?)(^\s*\};)")?;

    if let Some(_caps) = tests_pattern.captures(content) {
        let tests_header = &_caps[1];
        let tests_body = &_caps[2];
        let tests_closing = &_caps[3];

        // Get indentation from tests_closing
        let indent_len = tests_closing.len() - tests_closing.trim_start().len();
        let indent = " ".repeat(indent_len + 2);

        // Check if unittests already exists
        if tests_body.contains("unittests") {
            debug!("unittests already exists in passthru.tests");
            return Ok(content.to_owned());
        }

        let result = format!(
            "{tests_header}{tests_body}\n{indent}unittests = runUnitTests \
             finalAttrs.finalPackage;\n{tests_closing}"
        );

        Ok(tests_pattern.replace(content, result).into_owned())
    } else {
        anyhow::bail!("Could not find tests pattern in passthru")
    }
}

/// Add tests attribute to existing passthru
pub fn add_tests_to_passthru(content: &str) -> anyhow::Result<String> {
    // Find passthru and add tests attribute before closing brace
    let passthru_pattern = Regex::new(r"(?ms)(^\s*passthru\s*=\s*\{)(.*?)(^\s*)(\};)")?;

    if let Some(caps) = passthru_pattern.captures(content) {
        let passthru_header = &caps[1];
        let passthru_body = &caps[2];
        let closing_indent = &caps[3];
        let closing_brace = &caps[4];

        // Get indentation
        let indent_len = closing_indent.len() - closing_indent.trim_start().len();
        let indent = " ".repeat(indent_len + 2);

        let result = format!(
            "{passthru_header}{passthru_body}{indent}tests = {{\n{indent}unittests = runUnitTests \
             finalAttrs.finalPackage;\n{indent}}};"
        );

        // Add newline before closing if there was content
        let final_result = if !passthru_body.trim().is_empty() {
            format!("{result}\n{closing_indent}{closing_brace}")
        } else {
            format!("{result}{closing_indent}{closing_brace}")
        };

        Ok(passthru_pattern.replace(content, final_result).into_owned())
    } else {
        anyhow::bail!("Could not find passthru pattern")
    }
}

/// Add passthru with tests
pub fn add_passthru_with_tests(content: &str) -> anyhow::Result<String> {
    // Add passthru before meta
    let meta_pattern = Regex::new(r"(?m)(^\s*)(meta\s*=)")?;

    if let Some(caps) = meta_pattern.captures(content) {
        let indent = &caps[1];
        let result = meta_pattern.replace(
            content,
            format!(
                "{indent}passthru = {{\n{indent}  tests = {{\n{indent}    unittests = \
                 runUnitTests finalAttrs.finalPackage;\n{indent}  \
                 }};\n{indent}}};\n\n{indent}meta ="
            ),
        );
        return Ok(result.into_owned());
    }

    // If no meta, add before the closing brace using simplified syntax
    // Find the closing brace
    let closing_pattern = Regex::new(r"(?ms)(.*)(^\})\s*$")?;

    if let Some(caps) = closing_pattern.captures(content) {
        let before = &caps[1];
        let closing = &caps[2];

        // Find the indentation from the last non-empty line before the closing brace
        let last_line_pattern = Regex::new(r"(?m)^([ \t]+)[^\s].*$")?;
        let indent = if let Some(indent_caps) = last_line_pattern.captures(before) {
            indent_caps[1].to_string()
        } else {
            "  ".to_owned() // Default to 2 spaces
        };

        let result = format!(
            "{before}\n{indent}passthru.tests.unittests = runUnitTests \
             finalAttrs.finalPackage;\n{closing}"
        );
        return Ok(result);
    }

    // If we still can't find it, just return as is
    debug!("Could not find appropriate place to add passthru");
    Ok(content.to_owned())
}

/// Update test-related comments
pub fn update_test_comments(content: &str) -> String {
    // Replace TODO(corepkgs) comments about moving tests
    let todo_pattern =
        Regex::new(r"(?m)^\s*#.*TODO\(corepkgs\):.*move.*unittests.*passthru.*\n").unwrap();
    let result = todo_pattern.replace_all(content, "");

    // Update "Test suite is quite long" comment to include ", run as passthru"
    let check_comment_pattern =
        Regex::new(r"(?m)(^\s*#\s*Test suite is quite long)(\s*)$").unwrap();
    check_comment_pattern
        .replace_all(&result, "$1, run as passthru$2")
        .into_owned()
}
