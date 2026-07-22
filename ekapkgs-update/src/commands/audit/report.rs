use std::collections::BTreeMap;

use super::types::{AuditFinding, AuditReport, CheckCategory, Severity};

/// Format the audit report for terminal display with ANSI colors
pub fn format_terminal(report: &AuditReport) -> String {
    let mut out = String::new();

    // Header
    out.push_str(&format!("Package Audit: {}\n", report.attr_path));
    out.push_str(&"=".repeat(50));
    out.push('\n');

    let errors = report.error_count();
    let warnings = report.warning_count();
    let infos = report.info_count();

    if report.findings.is_empty() {
        out.push_str("\x1b[32mNo issues found.\x1b[0m\n");
        return out;
    }

    // Summary line
    let mut parts = Vec::new();
    if errors > 0 {
        parts.push(format!("\x1b[31m{errors} error(s)\x1b[0m"));
    }
    if warnings > 0 {
        parts.push(format!("\x1b[33m{warnings} warning(s)\x1b[0m"));
    }
    if infos > 0 {
        parts.push(format!("\x1b[2m{infos} info\x1b[0m"));
    }
    out.push_str(&parts.join(", "));
    out.push('\n');
    out.push('\n');

    // Group by category
    let grouped = group_by_category(&report.findings);

    for (category, findings) in &grouped {
        out.push_str(&format!("{}:\n", category.label()));

        for finding in findings {
            let icon = match finding.severity {
                Severity::Error => "\x1b[31m[E]\x1b[0m",
                Severity::Warning => "\x1b[33m[W]\x1b[0m",
                Severity::Info => "\x1b[2m[I]\x1b[0m",
            };

            let location = if let Some(ref path) = finding.file_path {
                format!(" {}/{}", finding.output_name, path)
            } else {
                String::new()
            };

            out.push_str(&format!(
                "  {icon} {}{location}: {}\n",
                finding.check_name, finding.message
            ));

            if let Some(ref details) = finding.details {
                out.push_str(&format!("       {details}\n"));
            }
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "({} checks run in {}ms)\n",
        report.checks_run, report.duration_ms
    ));

    out
}

/// Format the audit report as markdown for PR bodies
pub fn format_markdown(report: &AuditReport) -> String {
    let mut out = String::new();

    let errors = report.error_count();
    let warnings = report.warning_count();
    let infos = report.info_count();

    out.push_str("## Package Audit\n\n");

    if report.findings.is_empty() {
        out.push_str("No issues found.\n");
        return out;
    }

    // Summary
    let mut summary_parts = Vec::new();
    if errors > 0 {
        summary_parts.push(format!("**{errors} error(s)**"));
    }
    if warnings > 0 {
        summary_parts.push(format!("{warnings} warning(s)"));
    }
    if infos > 0 {
        summary_parts.push(format!("{infos} info"));
    }
    out.push_str(&summary_parts.join(", "));
    out.push_str("\n\n");

    let grouped = group_by_category(&report.findings);

    // Show errors prominently (not collapsed)
    let error_findings: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .collect();

    if !error_findings.is_empty() {
        out.push_str("### Errors\n\n");
        for finding in &error_findings {
            let location = if let Some(ref path) = finding.file_path {
                format!("`{}/{}`", finding.output_name, path)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "- {} {}: {}",
                finding.check_name, location, finding.message
            ));
            if let Some(ref details) = finding.details {
                out.push_str(&format!(" (`{details}`)"));
            }
            out.push('\n');
        }
        out.push('\n');
    }

    // Warnings and info in collapsible sections
    let non_error_findings: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.severity != Severity::Error)
        .collect();

    if !non_error_findings.is_empty() {
        out.push_str(&format!(
            "<details>\n<summary>Warnings and info ({} items)</summary>\n\n",
            non_error_findings.len()
        ));

        for (category, findings) in &grouped {
            let non_errors: Vec<_> = findings
                .iter()
                .filter(|f| f.severity != Severity::Error)
                .collect();

            if non_errors.is_empty() {
                continue;
            }

            out.push_str(&format!("**{}**\n\n", category.label()));
            for finding in non_errors {
                let icon = match finding.severity {
                    Severity::Warning => ":warning:",
                    Severity::Info => ":information_source:",
                    _ => "",
                };
                let location = if let Some(ref path) = finding.file_path {
                    format!("`{}/{}`", finding.output_name, path)
                } else {
                    String::new()
                };
                out.push_str(&format!("- {icon} {location} {}\n", finding.message));
            }
            out.push('\n');
        }

        out.push_str("</details>\n");
    }

    out
}

/// Format the audit report as JSON
pub fn format_json(report: &AuditReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
}

/// Group findings by category, preserving category order
fn group_by_category(findings: &[AuditFinding]) -> BTreeMap<CheckCategory, Vec<&AuditFinding>> {
    let mut grouped: BTreeMap<CheckCategory, Vec<&AuditFinding>> = BTreeMap::new();
    for finding in findings {
        grouped.entry(finding.category).or_default().push(finding);
    }
    grouped
}
