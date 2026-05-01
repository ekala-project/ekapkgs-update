use regex::Regex;

/// Add runUnitTests to the function arguments
pub fn add_run_unit_tests_argument(content: &str) -> anyhow::Result<String> {
    // Pattern to match function arguments ending with }:
    // We want to add runUnitTests before the closing }:
    let pattern = Regex::new(r"(?s)(.*?)(\n\s*)(\}:)(.*)")?;

    if let Some(caps) = pattern.captures(content) {
        let before = &caps[1];
        let whitespace = &caps[2];
        let closing = &caps[3];
        let after = &caps[4];

        // Find the last argument to determine indentation
        let lines: Vec<&str> = before.lines().collect();
        let last_arg_line = lines
            .iter()
            .rev()
            .find(|line| line.trim().ends_with(','))
            .or_else(|| lines.iter().rev().find(|line| line.contains(',')))
            .unwrap_or(&lines[lines.len() - 1]);

        // Get indentation from last argument
        let indent = last_arg_line.len() - last_arg_line.trim_start().len();
        let indent_str = " ".repeat(indent);

        Ok(format!(
            "{}{}{}runUnitTests,{}{}{}",
            before, whitespace, indent_str, whitespace, closing, after
        ))
    } else {
        anyhow::bail!("Could not find function argument pattern in file")
    }
}
