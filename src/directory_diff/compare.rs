use super::types::*;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Compare two sets of build outputs
pub fn compare_build_outputs(
    old_outputs: &[(String, PathBuf)],
    new_outputs: &[(String, PathBuf)],
    config: &DiffConfig,
) -> Result<DirectoryDiff> {
    let mut output_diffs = Vec::new();
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

        let output_diff = compare_single_output(
            output_name,
            old_path,
            new_path,
            config,
            &mut total_files_listed,
            &mut truncated,
        )
        .with_context(|| format!("Failed to compare output '{}'", output_name))?;

        total_added += output_diff
            .directories
            .values()
            .map(|d| d.total_added())
            .sum::<usize>();
        total_added += output_diff.summary_dirs.iter().map(|s| s.files_added).sum::<usize>();

        total_removed += output_diff
            .directories
            .values()
            .map(|d| d.total_removed())
            .sum::<usize>();
        total_removed += output_diff
            .summary_dirs
            .iter()
            .map(|s| s.files_removed)
            .sum::<usize>();

        total_size_change += output_diff
            .directories
            .values()
            .map(|d| d.size_change())
            .sum::<i64>();
        total_size_change += output_diff.summary_dirs.iter().map(|s| s.size_change).sum::<i64>();

        output_diffs.push(output_diff);
    }

    Ok(DirectoryDiff {
        outputs: output_diffs,
        total_added,
        total_removed,
        total_size_change,
        truncated,
    })
}

/// Compare a single output between old and new versions
fn compare_single_output(
    output_name: &str,
    old_path: Option<&PathBuf>,
    new_path: Option<&PathBuf>,
    config: &DiffConfig,
    total_files_listed: &mut usize,
    truncated: &mut bool,
) -> Result<OutputDiff> {
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

    // Find added and removed files
    let mut added: Vec<FileInfo> = Vec::new();
    let mut removed: Vec<FileInfo> = Vec::new();

    for (path, info) in &new_files {
        if !old_files.contains_key(path) {
            added.push(info.clone());
        }
    }

    for (path, info) in &old_files {
        if !new_files.contains_key(path) {
            removed.push(info.clone());
        }
    }

    // Separate regular changes from summary-only directories
    let mut regular_dirs: BTreeMap<PathBuf, DirectoryChange> = BTreeMap::new();
    let mut summary_stats: HashMap<PathBuf, (usize, usize, i64)> = HashMap::new();

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
                .unwrap_or_else(|| Path::new(""))
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
                .unwrap_or_else(|| Path::new(""))
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

    Ok(OutputDiff {
        output_name: output_name.to_string(),
        directories: regular_dirs,
        summary_dirs,
    })
}

/// Collect all files in a directory with their metadata
fn collect_files(root: &Path) -> Result<HashMap<PathBuf, FileInfo>> {
    let mut files = HashMap::new();

    for entry in WalkDir::new(root).follow_links(true) {
        let entry = entry.with_context(|| format!("Failed to walk directory {}", root.display()))?;
        let path = entry.path();

        // Skip directories, only process files
        if !path.is_file() {
            continue;
        }

        let metadata = entry.metadata()?;
        let relative_path = path
            .strip_prefix(root)
            .with_context(|| format!("Failed to strip prefix {} from {}", root.display(), path.display()))?
            .to_path_buf();

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
fn get_summary_dir(path: &Path) -> PathBuf {
    // Find the first component that matches a summary pattern
    let components: Vec<_> = path.components().collect();

    for (i, component) in components.iter().enumerate() {
        if let Some(s) = component.as_os_str().to_str() {
            if matches!(s, "share" | "doc" | "man" | "locale" | "info") {
                // Return path up to and including this component
                return components.iter().take(i + 1).collect();
            }
        }
    }

    // Fallback: return the top-level directory
    components.first().map(|c| PathBuf::from(c.as_os_str())).unwrap_or_else(|| PathBuf::from(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_summary_dir() {
        assert_eq!(
            get_summary_dir(Path::new("share/doc/foo/bar.txt")),
            PathBuf::from("share")
        );
        assert_eq!(
            get_summary_dir(Path::new("usr/share/locale/en/LC_MESSAGES/foo.mo")),
            PathBuf::from("usr/share")
        );
        assert_eq!(
            get_summary_dir(Path::new("share/man/man1/foo.1.gz")),
            PathBuf::from("share")
        );
    }

    #[test]
    fn test_should_summarize() {
        let config = DiffConfig::default();
        assert!(config.should_summarize(Path::new("share/doc/foo.txt")));
        assert!(config.should_summarize(Path::new("usr/share/man/man1/foo.1")));
        assert!(!config.should_summarize(Path::new("bin/foo")));
        assert!(!config.should_summarize(Path::new("lib/libfoo.so")));
    }
}
