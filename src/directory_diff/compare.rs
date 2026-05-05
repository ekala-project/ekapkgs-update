use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use walkdir::WalkDir;

use super::types::*;

/// Compare two sets of build outputs
pub fn compare_build_outputs(
    old_outputs: &[(String, PathBuf)],
    new_outputs: &[(String, PathBuf)],
    config: &DiffConfig,
) -> Result<DirectoryDiff> {
    let mut output_diffs = Vec::new();
    let mut significant_size_changes = Vec::new();
    let mut total_added = 0;
    let mut total_removed = 0;
    let mut total_size_change = 0i64;
    let mut total_files_listed = 0;
    let mut truncated = false;

    // Match up outputs by name
    let old_map: HashMap<&str, &PathBuf> = old_outputs
        .iter()
        .map(|(name, path)| (name.as_str(), path))
        .collect();
    let new_map: HashMap<&str, &PathBuf> = new_outputs
        .iter()
        .map(|(name, path)| (name.as_str(), path))
        .collect();

    // Detect added and removed outputs
    let mut added_outputs = Vec::new();
    let mut removed_outputs = Vec::new();

    for name in new_map.keys() {
        if !old_map.contains_key(name) {
            added_outputs.push(name.to_string());
        }
    }

    for name in old_map.keys() {
        if !new_map.contains_key(name) {
            removed_outputs.push(name.to_string());
        }
    }

    added_outputs.sort();
    removed_outputs.sort();

    // Get all unique output names
    let mut all_output_names: Vec<&str> = old_map.keys().copied().collect();
    for name in new_map.keys() {
        if !all_output_names.contains(name) {
            all_output_names.push(name);
        }
    }
    all_output_names.sort();

    for output_name in all_output_names {
        let old_path = old_map.get(output_name).copied();
        let new_path = new_map.get(output_name).copied();

        let (output_diff, size_changes) = compare_single_output(
            output_name,
            old_path.map(PathBuf::as_path),
            new_path.map(PathBuf::as_path),
            config,
            &mut total_files_listed,
            &mut truncated,
        )
        .with_context(|| format!("Failed to compare output '{output_name}'"))?;

        // Collect significant size changes
        significant_size_changes.extend(size_changes);

        total_added += output_diff
            .directories
            .values()
            .map(super::types::DirectoryChange::total_added)
            .sum::<usize>();
        total_added += output_diff
            .summary_dirs
            .iter()
            .map(|s| s.files_added)
            .sum::<usize>();

        total_removed += output_diff
            .directories
            .values()
            .map(super::types::DirectoryChange::total_removed)
            .sum::<usize>();
        total_removed += output_diff
            .summary_dirs
            .iter()
            .map(|s| s.files_removed)
            .sum::<usize>();

        total_size_change += output_diff
            .directories
            .values()
            .map(super::types::DirectoryChange::size_change)
            .sum::<i64>();
        total_size_change += output_diff
            .summary_dirs
            .iter()
            .map(|s| s.size_change)
            .sum::<i64>();

        output_diffs.push(output_diff);
    }

    // Sort size changes by absolute percentage change (largest first)
    significant_size_changes.sort_by(|a, b| {
        b.percentage_change
            .abs()
            .partial_cmp(&a.percentage_change.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Calculate closure size changes
    let closure_size_changes = calculate_closure_size_changes(old_outputs, new_outputs);

    Ok(DirectoryDiff {
        outputs: output_diffs,
        added_outputs,
        removed_outputs,
        significant_size_changes,
        closure_size_changes,
        total_added,
        total_removed,
        total_size_change,
        truncated,
    })
}

/// Compare a single output between old and new versions
fn compare_single_output(
    output_name: &str,
    old_path: Option<&Path>,
    new_path: Option<&Path>,
    config: &DiffConfig,
    total_files_listed: &mut usize,
    truncated: &mut bool,
) -> Result<(OutputDiff, Vec<super::types::SignificantSizeChange>)> {
    use super::types::SignificantSizeChange;

    // Collect file information from both paths
    let old_files = if let Some(path) = old_path {
        collect_files(path)?
    } else {
        HashMap::new()
    };

    let new_files = if let Some(path) = new_path {
        collect_files(path)?
    } else {
        HashMap::new()
    };

    // Find added, removed, and size-changed files
    let mut added: Vec<FileInfo> = Vec::new();
    let mut removed: Vec<FileInfo> = Vec::new();
    let mut significant_size_changes: Vec<SignificantSizeChange> = Vec::new();

    for (path, info) in &new_files {
        if !old_files.contains_key(path) {
            added.push(info.clone());
        } else {
            // File exists in both - check for significant size change
            let old_info = &old_files[path];
            if old_info.size > 0 {
                let size_diff = info.size as i64 - old_info.size as i64;
                let percentage_change = (size_diff as f64 / old_info.size as f64) * 100.0;

                // Report if change is more than 10%
                if percentage_change.abs() > 10.0 {
                    significant_size_changes.push(SignificantSizeChange {
                        output_name: output_name.to_owned(),
                        file_path: info.relative_path.clone(),
                        old_size: old_info.size,
                        new_size: info.size,
                        percentage_change,
                    });
                }
            }
        }
    }

    for (path, info) in &old_files {
        if !new_files.contains_key(path) {
            removed.push(info.clone());
        }
    }

    // Separate regular changes from summary-only directories
    let mut regular_dirs: BTreeMap<Utf8PathBuf, DirectoryChange> = BTreeMap::new();
    let mut summary_stats: HashMap<Utf8PathBuf, (usize, usize, i64)> = HashMap::new();

    // Process added files
    for file in added {
        if config.should_summarize(&file.relative_path) {
            let dir = get_summary_dir(&file.relative_path);
            let entry = summary_stats.entry(dir).or_insert((0, 0, 0));
            entry.0 += 1;
            entry.2 += file.size as i64;
        } else {
            if *total_files_listed >= config.max_files {
                *truncated = true;
                continue;
            }

            let dir = file
                .relative_path
                .parent()
                .unwrap_or_else(|| Utf8Path::new(""))
                .to_path_buf();
            regular_dirs
                .entry(dir)
                .or_insert_with(DirectoryChange::new)
                .added_files
                .push(file);
            *total_files_listed += 1;
        }
    }

    // Process removed files
    for file in removed {
        if config.should_summarize(&file.relative_path) {
            let dir = get_summary_dir(&file.relative_path);
            let entry = summary_stats.entry(dir).or_insert((0, 0, 0));
            entry.1 += 1;
            entry.2 -= file.size as i64;
        } else {
            if *total_files_listed >= config.max_files {
                *truncated = true;
                continue;
            }

            let dir = file
                .relative_path
                .parent()
                .unwrap_or_else(|| Utf8Path::new(""))
                .to_path_buf();
            regular_dirs
                .entry(dir)
                .or_insert_with(DirectoryChange::new)
                .removed_files
                .push(file);
            *total_files_listed += 1;
        }
    }

    // Convert summary stats to SummaryDirectory objects
    let mut summary_dirs: Vec<SummaryDirectory> = summary_stats
        .into_iter()
        .map(|(path, (added, removed, size_change))| SummaryDirectory {
            path,
            files_added: added,
            files_removed: removed,
            size_change,
        })
        .collect();
    summary_dirs.sort_by(|a, b| a.path.cmp(&b.path));

    // Remove empty directories
    regular_dirs.retain(|_, change| !change.is_empty());

    Ok((
        OutputDiff {
            output_name: output_name.to_owned(),
            directories: regular_dirs,
            summary_dirs,
        },
        significant_size_changes,
    ))
}

/// Collect all files in a directory with their metadata
///
/// Files whose paths are not valid UTF-8 are skipped (with a debug log) since
/// our diff types are UTF-8 throughout.
fn collect_files(root: &Path) -> Result<HashMap<Utf8PathBuf, FileInfo>> {
    let mut files = HashMap::new();

    for entry in WalkDir::new(root).follow_links(true) {
        let entry =
            entry.with_context(|| format!("Failed to walk directory {}", root.display()))?;
        let path = entry.path();

        // Skip directories, only process files
        if !path.is_file() {
            continue;
        }

        let metadata = entry.metadata()?;
        let relative = path.strip_prefix(root).with_context(|| {
            format!(
                "Failed to strip prefix {} from {}",
                root.display(),
                path.display()
            )
        })?;

        let relative_path = match Utf8PathBuf::from_path_buf(relative.to_path_buf()) {
            Ok(p) => p,
            Err(non_utf8) => {
                tracing::debug!(
                    "Skipping non-UTF-8 path in {}: {}",
                    root.display(),
                    non_utf8.display()
                );
                continue;
            },
        };

        let is_executable = metadata.permissions().mode() & 0o111 != 0;

        files.insert(
            relative_path.clone(),
            FileInfo {
                relative_path,
                size: metadata.len(),
                is_executable,
            },
        );
    }

    Ok(files)
}

/// Get the top-level directory that should be summarized
fn get_summary_dir(path: &Utf8Path) -> Utf8PathBuf {
    // Find the first component that matches a summary pattern
    let components: Vec<_> = path.components().collect();

    for (i, component) in components.iter().enumerate() {
        if matches!(
            component.as_str(),
            "share" | "doc" | "man" | "locale" | "info"
        ) {
            // Return path up to and including this component
            return components.iter().take(i + 1).collect();
        }
    }

    // Fallback: return the top-level directory
    components
        .first()
        .map(|c| Utf8PathBuf::from(c.as_str()))
        .unwrap_or_else(|| Utf8PathBuf::from(""))
}

/// Calculate closure size for a store path using nix path-info
fn get_closure_size(store_path: &Path) -> Option<u64> {
    let output = Command::new("nix")
        .args(["path-info", "--json", "--closure-size"])
        .arg(store_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let json: serde_json::Value = serde_json::from_str(&stdout).ok()?;

    // The JSON output is an array with one object per path
    // We want the closure size which is the sum of all paths
    if let Some(arr) = json.as_array() {
        if let Some(first) = arr.first() {
            if let Some(closure_size) = first.get("closureSize") {
                return closure_size.as_u64();
            }
        }
    }

    None
}

/// Calculate closure size changes for outputs
fn calculate_closure_size_changes(
    old_outputs: &[(String, PathBuf)],
    new_outputs: &[(String, PathBuf)],
) -> Vec<ClosureSizeChange> {
    let mut changes = Vec::new();

    // Create map for easy lookup
    let old_map: HashMap<&str, &PathBuf> = old_outputs
        .iter()
        .map(|(name, path)| (name.as_str(), path))
        .collect();

    // Check outputs that exist in both versions
    for (output_name, new_path) in new_outputs {
        if let Some(old_path) = old_map.get(output_name.as_str()) {
            // Get closure sizes
            if let (Some(old_size), Some(new_size)) =
                (get_closure_size(old_path), get_closure_size(new_path))
            {
                if old_size > 0 {
                    let size_diff = new_size as i64 - old_size as i64;
                    let percentage_change = (size_diff as f64 / old_size as f64) * 100.0;

                    // Report if change is 20% or more (growth or shrinkage)
                    if percentage_change.abs() >= 20.0 {
                        changes.push(ClosureSizeChange {
                            output_name: output_name.clone(),
                            old_closure_size: old_size,
                            new_closure_size: new_size,
                            percentage_change,
                        });
                    }
                }
            }
        }
    }

    // Sort by absolute percentage change (largest first)
    changes.sort_by(|a, b| {
        b.percentage_change
            .abs()
            .partial_cmp(&a.percentage_change.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_summary_dir() {
        assert_eq!(
            get_summary_dir(Utf8Path::new("share/doc/foo/bar.txt")),
            Utf8PathBuf::from("share")
        );
        assert_eq!(
            get_summary_dir(Utf8Path::new("usr/share/locale/en/LC_MESSAGES/foo.mo")),
            Utf8PathBuf::from("usr/share")
        );
        assert_eq!(
            get_summary_dir(Utf8Path::new("share/man/man1/foo.1.gz")),
            Utf8PathBuf::from("share")
        );
    }

    #[test]
    fn test_should_summarize() {
        let config = DiffConfig::default();
        assert!(config.should_summarize(Utf8Path::new("share/doc/foo.txt")));
        assert!(config.should_summarize(Utf8Path::new("usr/share/man/man1/foo.1")));
        assert!(!config.should_summarize(Utf8Path::new("bin/foo")));
        assert!(!config.should_summarize(Utf8Path::new("lib/libfoo.so")));
    }
}
