//! Report command for generating categorized failure summaries
//!
//! Queries the `update_logs` table for failed updates, deduplicates by package,
//! categorizes failures by error type, and renders a markdown report.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Context;
use chrono::Utc;

use crate::database::{Database, UpdateLog};

/// Fixed display order for failure categories
const CATEGORY_ORDER: &[&str] = &[
    "Build failures",
    "Cargo hash discovery failures",
    "Vendor hash discovery failures",
    "Hash discovery failures",
    "Source build failures",
    "Patch removal failures",
    "Attribute not found (eval issues)",
    "No compatible release found",
    "Other failures",
];

/// Categorize a failure based on its error log text.
/// Patterns are checked in order; first match wins.
fn categorize_error(error_log: &str) -> &'static str {
    if error_log.contains("Attribute") && error_log.contains("not found") {
        "Attribute not found (eval issues)"
    } else if error_log.contains("No compatible releases") {
        "No compatible release found"
    } else if error_log.contains("Could not extract correct cargoHash") {
        "Cargo hash discovery failures"
    } else if error_log.contains("Could not extract correct vendorHash") {
        "Vendor hash discovery failures"
    } else if error_log.contains("Could not extract correct") && error_log.contains("hash") {
        "Hash discovery failures"
    } else if error_log.contains("Source build failed") {
        "Source build failures"
    } else if error_log.contains("couldn't remove it from the patches") {
        "Patch removal failures"
    } else if error_log.contains("Package build failed") {
        "Build failures"
    } else {
        "Other failures"
    }
}

/// Generate a categorized failure report
pub async fn report(
    database_path: String,
    package_pattern: Option<String>,
    since_days: Option<u32>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&database_path).to_string();
    let db = Database::new(&expanded_path).await?;

    let since = since_days.map(|days| Utc::now() - chrono::Duration::days(days as i64));

    let logs = db
        .query_failed_logs(package_pattern.as_deref(), since, None)
        .await
        .context("Failed to query failed update logs")?;

    // Deduplicate by attr_path, keeping the most recent (results are ORDER BY timestamp DESC)
    let mut seen = HashSet::new();
    let deduped: Vec<_> = logs
        .into_iter()
        .filter(|log| seen.insert(log.attr_path.clone()))
        .collect();

    if deduped.is_empty() {
        println!("No failed updates found.");
        return Ok(());
    }

    let markdown = render_markdown(&deduped);

    if let Some(path) = output {
        tokio::fs::write(&path, &markdown)
            .await
            .with_context(|| format!("write report to {}", path.display()))?;
        println!("Report written to {}", path.display());
    } else {
        print!("{markdown}");
    }

    Ok(())
}

/// Render a categorized markdown report from deduplicated failure logs
fn render_markdown(logs: &[UpdateLog]) -> String {
    // Categorize each log
    let mut categories: Vec<(&str, Vec<&UpdateLog>)> = CATEGORY_ORDER
        .iter()
        .map(|&cat| (cat, Vec::new()))
        .collect();

    for log in logs {
        let cat = categorize_error(&log.error_log);
        if let Some(entry) = categories.iter_mut().find(|(name, _)| *name == cat) {
            entry.1.push(log);
        }
    }

    // Sort entries within each category by attr_path
    for (_, entries) in &mut categories {
        entries.sort_by(|a, b| a.attr_path.cmp(&b.attr_path));
    }

    let mut out = String::new();
    out.push_str("# Failed Updates\n\n");
    out.push_str(&format!("Total: {} packages failed\n", logs.len()));

    for (name, entries) in &categories {
        if entries.is_empty() {
            continue;
        }
        out.push_str(&format!("\n## {} ({})\n\n", name, entries.len()));
        out.push_str("| Package | Old | New |\n");
        out.push_str("|---------|-----|-----|\n");
        for log in entries {
            let old = log.old_version.as_deref().unwrap_or("?");
            let new = log.new_version.as_deref().unwrap_or("?");
            out.push_str(&format!("| `{}` | {} | {} |\n", log.attr_path, old, new));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_categorize_build_failure() {
        assert_eq!(
            categorize_error("Package build failed after update with no reversed patches"),
            "Build failures"
        );
    }

    #[test]
    fn test_categorize_cargo_hash() {
        assert_eq!(
            categorize_error("Could not extract correct cargoHash from build error"),
            "Cargo hash discovery failures"
        );
    }

    #[test]
    fn test_categorize_vendor_hash() {
        assert_eq!(
            categorize_error("Could not extract correct vendorHash from build error"),
            "Vendor hash discovery failures"
        );
    }

    #[test]
    fn test_categorize_generic_hash() {
        assert_eq!(
            categorize_error("Could not extract correct hash from build error"),
            "Hash discovery failures"
        );
    }

    #[test]
    fn test_categorize_source_build() {
        assert_eq!(
            categorize_error("Source build failed after hash update"),
            "Source build failures"
        );
    }

    #[test]
    fn test_categorize_attribute_not_found() {
        assert_eq!(
            categorize_error("Attribute 'version' not found"),
            "Attribute not found (eval issues)"
        );
    }

    #[test]
    fn test_categorize_no_compatible() {
        assert_eq!(
            categorize_error("No compatible releases found for version 1.0"),
            "No compatible release found"
        );
    }

    #[test]
    fn test_categorize_patch_removal() {
        assert_eq!(
            categorize_error("patch 'foo.patch' but couldn't remove it from the patches array"),
            "Patch removal failures"
        );
    }

    #[test]
    fn test_categorize_other() {
        assert_eq!(categorize_error("something unexpected"), "Other failures");
    }

    #[test]
    fn test_render_markdown() {
        let logs = vec![
            UpdateLog {
                drv_path: "/nix/store/abc.drv".to_owned(),
                attr_path: "hello".to_owned(),
                timestamp: "2026-07-08T00:00:00Z".to_owned(),
                status: "failed".to_owned(),
                error_log: "Package build failed after update".to_owned(),
                old_version: Some("1.0".to_owned()),
                new_version: Some("2.0".to_owned()),
            },
            UpdateLog {
                drv_path: "/nix/store/def.drv".to_owned(),
                attr_path: "world".to_owned(),
                timestamp: "2026-07-08T00:00:00Z".to_owned(),
                status: "failed".to_owned(),
                error_log: "No compatible releases found".to_owned(),
                old_version: Some("3.0".to_owned()),
                new_version: Some("4.0".to_owned()),
            },
        ];

        let md = render_markdown(&logs);
        assert!(md.contains("Total: 2 packages failed"));
        assert!(md.contains("## Build failures (1)"));
        assert!(md.contains("| `hello` | 1.0 | 2.0 |"));
        assert!(md.contains("## No compatible release found (1)"));
        assert!(md.contains("| `world` | 3.0 | 4.0 |"));
        // Empty categories should not appear
        assert!(!md.contains("Cargo hash"));
    }
}
