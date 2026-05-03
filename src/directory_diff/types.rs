use std::collections::BTreeMap;
use std::path::PathBuf;

/// Complete directory diff result across all outputs
#[derive(Debug, Clone)]
pub struct DirectoryDiff {
    /// Per-output diffs (e.g., "out", "dev", "lib")
    pub outputs: Vec<OutputDiff>,
    /// Total files added across all outputs
    pub total_added: usize,
    /// Total files removed across all outputs
    pub total_removed: usize,
    /// Total size change in bytes (can be negative)
    pub total_size_change: i64,
    /// Whether the diff was truncated due to file count limits
    pub truncated: bool,
}

/// Directory diff for a single output
#[derive(Debug, Clone)]
pub struct OutputDiff {
    /// Name of the output (e.g., "out", "dev", "lib")
    pub output_name: String,
    /// Changes grouped by directory (sorted by path)
    pub directories: BTreeMap<PathBuf, DirectoryChange>,
    /// Directories that are summarized (share/, doc/, etc.)
    pub summary_dirs: Vec<SummaryDirectory>,
}

/// Changes within a specific directory
#[derive(Debug, Clone)]
pub struct DirectoryChange {
    /// Files added in this directory
    pub added_files: Vec<FileInfo>,
    /// Files removed from this directory
    pub removed_files: Vec<FileInfo>,
}

impl DirectoryChange {
    pub fn new() -> Self {
        Self {
            added_files: Vec::new(),
            removed_files: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.added_files.is_empty() && self.removed_files.is_empty()
    }

    pub fn total_added(&self) -> usize {
        self.added_files.len()
    }

    pub fn total_removed(&self) -> usize {
        self.removed_files.len()
    }

    pub fn size_change(&self) -> i64 {
        let added: u64 = self.added_files.iter().map(|f| f.size).sum();
        let removed: u64 = self.removed_files.iter().map(|f| f.size).sum();
        added as i64 - removed as i64
    }
}

/// Summary statistics for directories that aren't listed file-by-file
#[derive(Debug, Clone)]
pub struct SummaryDirectory {
    /// Path to the directory (relative to output root)
    pub path: PathBuf,
    /// Number of files added
    pub files_added: usize,
    /// Number of files removed
    pub files_removed: usize,
    /// Size change in bytes
    pub size_change: i64,
}

/// Information about a single file
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// Path relative to output root
    pub relative_path: PathBuf,
    /// File size in bytes
    pub size: u64,
    /// Whether the file is executable
    pub is_executable: bool,
}

impl FileInfo {
    /// Get just the filename component
    pub fn filename(&self) -> String {
        self.relative_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
    }
}

/// Configuration for directory diff comparison
#[derive(Debug, Clone)]
pub struct DiffConfig {
    /// Maximum total files to list individually before truncating
    pub max_files: usize,
    /// Path components that trigger summary-only treatment
    pub summary_path_components: Vec<String>,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            max_files: 10_000,
            summary_path_components: vec![
                "share".to_string(),
                "doc".to_string(),
                "man".to_string(),
                "locale".to_string(),
                "info".to_string(),
            ],
        }
    }
}

impl DiffConfig {
    /// Check if a path should be summarized rather than listed file-by-file
    pub fn should_summarize(&self, path: &std::path::Path) -> bool {
        path.components().any(|c| {
            if let Some(s) = c.as_os_str().to_str() {
                self.summary_path_components.contains(&s.to_string())
            } else {
                false
            }
        })
    }
}
