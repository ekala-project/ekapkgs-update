use super::types::*;

/// Format a DirectoryDiff as markdown for inclusion in PR body
pub fn format_for_pr_body(diff: &DirectoryDiff) -> String {
    let mut output = String::new();

    // Header
    output.push_str("## Directory Diff\n\n");

    // Output changes (added/removed outputs)
    if !diff.added_outputs.is_empty() || !diff.removed_outputs.is_empty() {
        output.push_str("**Output Changes:**\n");

        if !diff.added_outputs.is_empty() {
            output.push_str("- **Added outputs:** ");
            output.push_str(&diff.added_outputs.iter().map(|o| format!("`{}`", o)).collect::<Vec<_>>().join(", "));
            output.push('\n');
        }

        if !diff.removed_outputs.is_empty() {
            output.push_str("- **Removed outputs:** ");
            output.push_str(&diff.removed_outputs.iter().map(|o| format!("`{}`", o)).collect::<Vec<_>>().join(", "));
            output.push('\n');
        }

        output.push('\n');
    }

    // Closure size changes (≥20%) - show this prominently as it affects runtime dependencies
    if !diff.closure_size_changes.is_empty() {
        // Separate critical from non-critical changes
        let mut critical_changes = Vec::new();
        let mut other_changes = Vec::new();

        const TEN_MB: u64 = 10 * 1024 * 1024;

        for change in &diff.closure_size_changes {
            // Critical if >50% change AND the closure size itself is >10MB
            let is_large = change.new_closure_size > TEN_MB || change.old_closure_size > TEN_MB;
            if change.percentage_change.abs() > 50.0 && is_large {
                critical_changes.push(change);
            } else {
                other_changes.push(change);
            }
        }

        // Show critical changes without collapsing
        if !critical_changes.is_empty() {
            output.push_str("⚠️ **Critical Closure Size Changes (>50% and >10MB):**\n\n");

            for change in &critical_changes {
                let direction = if change.percentage_change > 0.0 {
                    "increased"
                } else {
                    "decreased"
                };

                output.push_str(&format!(
                    "- ⚠️ Output `{}` closure {} by **{:.1}%** ({} → {})\n",
                    change.output_name,
                    direction,
                    change.percentage_change.abs(),
                    format_size(change.old_closure_size),
                    format_size(change.new_closure_size)
                ));
            }

            output.push('\n');
        }

        // Show other changes in collapsible section
        if !other_changes.is_empty() {
            let has_warnings = other_changes.iter().any(|c| c.percentage_change >= 20.0);
            let summary = if has_warnings {
                format!("Closure Size Warnings (≥20% increase, {} outputs)", other_changes.len())
            } else {
                format!("Closure Size Changes (≥20%, {} outputs)", other_changes.len())
            };

            output.push_str(&format!("<details>\n<summary>{}</summary>\n\n", summary));

            for change in &other_changes {
                let direction = if change.percentage_change > 0.0 {
                    "increased"
                } else {
                    "decreased"
                };

                let prefix = if change.percentage_change >= 20.0 { "⚠️ " } else { "" };

                output.push_str(&format!(
                    "- {}Output `{}` closure {} by **{:.1}%** ({} → {})\n",
                    prefix,
                    change.output_name,
                    direction,
                    change.percentage_change.abs(),
                    format_size(change.old_closure_size),
                    format_size(change.new_closure_size)
                ));
            }

            output.push_str("\n</details>\n\n");
        }
    }

    // Significant size changes (>10%)
    if !diff.significant_size_changes.is_empty() {
        // Separate critical from non-critical file changes
        let mut critical_file_changes = Vec::new();
        let mut other_file_changes = Vec::new();

        const TEN_MB: u64 = 10 * 1024 * 1024;

        for change in &diff.significant_size_changes {
            // Critical if >50% change AND the file size itself is >10MB
            let is_large = change.new_size > TEN_MB || change.old_size > TEN_MB;
            if change.percentage_change.abs() > 50.0 && is_large {
                critical_file_changes.push(change);
            } else {
                other_file_changes.push(change);
            }
        }

        // Show critical file changes without collapsing
        if !critical_file_changes.is_empty() {
            output.push_str("⚠️ **Critical File Size Changes (>50% and >10MB):**\n\n");

            for change in &critical_file_changes {
                let direction = if change.percentage_change > 0.0 {
                    "grew"
                } else {
                    "shrank"
                };

                output.push_str(&format!(
                    "- ⚠️ `{}` in output `{}` {} by **{:.1}%** ({} → {})\n",
                    change.file_path.display(),
                    change.output_name,
                    direction,
                    change.percentage_change.abs(),
                    format_size(change.old_size),
                    format_size(change.new_size)
                ));
            }

            output.push('\n');
        }

        // Show other file changes in collapsible section
        if !other_file_changes.is_empty() {
            let summary = format!("Significant File Size Changes (>10%, {} files)", other_file_changes.len());
            output.push_str(&format!("<details>\n<summary>{}</summary>\n\n", summary));

            for change in &other_file_changes {
                let direction = if change.percentage_change > 0.0 {
                    "grew"
                } else {
                    "shrank"
                };

                output.push_str(&format!(
                    "- `{}` in output `{}` {} by **{:.1}%** ({} → {})\n",
                    change.file_path.display(),
                    change.output_name,
                    direction,
                    change.percentage_change.abs(),
                    format_size(change.old_size),
                    format_size(change.new_size)
                ));
            }

            output.push_str("\n</details>\n\n");
        }
    }

    // Overall summary
    output.push_str("**Summary:**\n");
    output.push_str(&format!("- Files added: {}\n", diff.total_added));
    output.push_str(&format!("- Files removed: {}\n", diff.total_removed));
    output.push_str(&format!(
        "- Total size change: {}\n",
        format_size_change(diff.total_size_change)
    ));

    if diff.truncated {
        output.push_str("\n⚠️ **Note:** Diff truncated due to large number of files (>10,000).\n");
    }

    output.push('\n');

    // Per-output diffs
    for output_diff in &diff.outputs {
        // Skip outputs with no changes
        if output_diff.directories.is_empty() && output_diff.summary_dirs.is_empty() {
            continue;
        }

        output.push_str(&format!("### Output: {}\n\n", output_diff.output_name));

        // Regular directories with file-by-file listings
        for (dir_path, changes) in &output_diff.directories {
            let dir_str = if dir_path.as_os_str().is_empty() {
                "(root)".to_string()
            } else {
                format!("{}/", dir_path.display())
            };

            let summary = format!(
                "{} (+{}, -{}, {})",
                dir_str,
                changes.total_added(),
                changes.total_removed(),
                format_size_change(changes.size_change())
            );

            output.push_str(&format!("<details>\n<summary>{}</summary>\n\n", summary));

            // Added files
            if !changes.added_files.is_empty() {
                output.push_str("**Added:**\n");
                for file in &changes.added_files {
                    let executable = if file.is_executable { ", executable" } else { "" };
                    output.push_str(&format!(
                        "- `{}` ({}{})\n",
                        file.filename(),
                        format_size(file.size),
                        executable
                    ));
                }
                output.push('\n');
            }

            // Removed files
            if !changes.removed_files.is_empty() {
                output.push_str("**Removed:**\n");
                for file in &changes.removed_files {
                    let executable = if file.is_executable { ", executable" } else { "" };
                    output.push_str(&format!(
                        "- `{}` ({}{})\n",
                        file.filename(),
                        format_size(file.size),
                        executable
                    ));
                }
                output.push('\n');
            }

            output.push_str("</details>\n\n");
        }

        // Summary directories
        for summary_dir in &output_diff.summary_dirs {
            let dir_str = format!("{}/", summary_dir.path.display());
            let summary = format!(
                "{} (+{}, -{}, {})",
                dir_str,
                summary_dir.files_added,
                summary_dir.files_removed,
                format_size_change(summary_dir.size_change)
            );

            output.push_str(&format!("<details>\n<summary>{}</summary>\n\n", summary));
            output.push_str("Summary only (documentation, locales, and related files).\n\n");
            output.push_str("</details>\n\n");
        }
    }

    output
}

/// Format a file size in human-readable form (KB, MB, GB)
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format a size change (can be negative) in human-readable form
fn format_size_change(bytes: i64) -> String {
    let abs_bytes = bytes.unsigned_abs();
    let formatted = format_size(abs_bytes);

    if bytes > 0 {
        format!("+{}", formatted)
    } else if bytes < 0 {
        format!("-{}", formatted)
    } else {
        formatted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_format_size_change() {
        assert_eq!(format_size_change(1024), "+1.0 KB");
        assert_eq!(format_size_change(-1024), "-1.0 KB");
        assert_eq!(format_size_change(0), "0 B");
    }
}
