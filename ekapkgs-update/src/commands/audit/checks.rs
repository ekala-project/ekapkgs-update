use std::collections::HashSet;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use camino::Utf8PathBuf;
use regex::Regex;
use tracing::{debug, warn};
use walkdir::WalkDir;

use super::types::{AuditConfig, AuditFinding, AuditReport, CheckCategory, Severity};

/// Check whether a file starts with the ELF magic bytes
fn is_elf(path: &Path) -> bool {
    std::fs::File::open(path)
        .and_then(|mut f| {
            let mut magic = [0u8; 4];
            f.read_exact(&mut magic)?;
            Ok(magic == [0x7f, b'E', b'L', b'F'])
        })
        .unwrap_or(false)
}

/// Try to convert a std::path::Path to a Utf8PathBuf, returning None on failure
fn to_utf8(path: &Path) -> Option<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(path.to_path_buf()).ok()
}

// ---------------------------------------------------------------------------
// Category 1: Broken References
// ---------------------------------------------------------------------------

/// Check for broken symlinks in the output tree
fn check_broken_symlinks(output_name: &str, output_path: &Path) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    // Walk WITHOUT following links so we can inspect each symlink
    for entry in WalkDir::new(output_path).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                debug!("walkdir error in {}: {}", output_path.display(), e);
                continue;
            },
        };

        if !entry.path_is_symlink() {
            continue;
        }

        let link_path = entry.path();
        if std::fs::canonicalize(link_path).is_ok() {
            continue;
        }

        let target = std::fs::read_link(link_path)
            .map(|t| t.to_string_lossy().to_string())
            .unwrap_or_else(|_| "<unreadable>".to_owned());

        let relative = link_path.strip_prefix(output_path).unwrap_or(link_path);

        findings.push(AuditFinding {
            severity: Severity::Error,
            category: CheckCategory::BrokenReferences,
            check_name: "broken_symlink".to_owned(),
            message: "Broken symlink: target does not exist".to_owned(),
            output_name: output_name.to_owned(),
            file_path: to_utf8(relative),
            details: Some(format!("target: {target}")),
        });
    }

    findings
}

/// Scan file contents for references to non-existent /nix/store paths
fn check_store_references(output_name: &str, output_path: &Path) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    let store_re = match Regex::new(r"/nix/store/[a-z0-9]{32}-[a-zA-Z0-9._+-]+") {
        Ok(re) => re,
        Err(e) => {
            warn!("Failed to compile store path regex: {}", e);
            return findings;
        },
    };

    // Extensions to skip (binary/compressed formats)
    let skip_extensions: HashSet<&str> = [
        "png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "webp", "gz", "xz", "bz2", "zst",
        "zip", "tar", "7z", "rar", "woff", "woff2", "ttf", "otf", "eot", "pyc", "pyo", "class",
        "o", "a",
    ]
    .into_iter()
    .collect();

    for entry in WalkDir::new(output_path).follow_links(true) {
        let Ok(entry) = entry else { continue };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        // Skip by extension
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if skip_extensions.contains(ext.to_lowercase().as_str()) {
                continue;
            }
        }

        // Skip files larger than 100MB
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > 100 * 1024 * 1024 {
            continue;
        }

        // Read first 8KB
        let mut buf = vec![0u8; 8192];
        let Ok(bytes_read) = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)) else {
            continue;
        };
        buf.truncate(bytes_read);

        // Convert to string (lossy is fine for scanning)
        let content = String::from_utf8_lossy(&buf);

        let mut seen = HashSet::new();
        for cap in store_re.find_iter(&content) {
            let store_path = cap.as_str();
            if seen.contains(store_path) {
                continue;
            }
            seen.insert(store_path.to_owned());

            if !Path::new(store_path).exists() {
                let relative = path.strip_prefix(output_path).unwrap_or(path);
                findings.push(AuditFinding {
                    severity: Severity::Warning,
                    category: CheckCategory::BrokenReferences,
                    check_name: "missing_store_reference".to_owned(),
                    message: "References a store path that does not exist".to_owned(),
                    output_name: output_name.to_owned(),
                    file_path: to_utf8(relative),
                    details: Some(store_path.to_owned()),
                });
            }
        }
    }

    findings
}

/// Check for shared libraries that cannot be found by ldd
async fn check_shared_libs(output_name: &str, output_path: &Path) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    // Check if ldd is available
    let ldd_available = tokio::process::Command::new("ldd")
        .arg("--version")
        .output()
        .await
        .is_ok();

    if !ldd_available {
        debug!("ldd not available, skipping shared library check");
        return findings;
    }

    for entry in WalkDir::new(output_path).follow_links(true) {
        let Ok(entry) = entry else { continue };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        if !is_elf(path) {
            continue;
        }

        let Ok(output) = tokio::process::Command::new("ldd")
            .arg(path)
            .output()
            .await
        else {
            continue;
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains("not found") {
                let lib_name = line.split_whitespace().next().unwrap_or("<unknown>");
                let relative = path.strip_prefix(output_path).unwrap_or(path);
                findings.push(AuditFinding {
                    severity: Severity::Error,
                    category: CheckCategory::BrokenReferences,
                    check_name: "missing_shared_lib".to_owned(),
                    message: format!("Shared library not found: {lib_name}"),
                    output_name: output_name.to_owned(),
                    file_path: to_utf8(relative),
                    details: Some(line.trim().to_owned()),
                });
            }
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Category 2: Layout Issues
// ---------------------------------------------------------------------------

/// Check that the output follows expected Nix package layout conventions
fn check_layout(output_name: &str, output_path: &Path) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    let expected_top_level: HashSet<&str> = [
        "bin",
        "lib",
        "lib64",
        "share",
        "etc",
        "include",
        "libexec",
        "sbin",
        "man",
        "nix-support",
    ]
    .into_iter()
    .collect();

    // Check if the output is empty
    let entries: Vec<_> = match std::fs::read_dir(output_path) {
        Ok(iter) => iter.filter_map(Result::ok).collect(),
        Err(_) => {
            findings.push(AuditFinding {
                severity: Severity::Error,
                category: CheckCategory::LayoutIssues,
                check_name: "unreadable_output".to_owned(),
                message: "Cannot read output directory".to_owned(),
                output_name: output_name.to_owned(),
                file_path: None,
                details: None,
            });
            return findings;
        },
    };

    if entries.is_empty() {
        findings.push(AuditFinding {
            severity: Severity::Error,
            category: CheckCategory::LayoutIssues,
            check_name: "empty_output".to_owned(),
            message: "Output directory is empty".to_owned(),
            output_name: output_name.to_owned(),
            file_path: None,
            details: None,
        });
        return findings;
    }

    // Check for files-only output (no regular files anywhere)
    let has_files = WalkDir::new(output_path)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .any(|e| e.file_type().is_file());

    if !has_files {
        findings.push(AuditFinding {
            severity: Severity::Warning,
            category: CheckCategory::LayoutIssues,
            check_name: "no_regular_files".to_owned(),
            message: "Output contains directories but no regular files".to_owned(),
            output_name: output_name.to_owned(),
            file_path: None,
            details: None,
        });
    }

    // Check top-level entries
    for entry in &entries {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !expected_top_level.contains(name_str.as_ref()) {
            findings.push(AuditFinding {
                severity: Severity::Warning,
                category: CheckCategory::LayoutIssues,
                check_name: "unexpected_top_level".to_owned(),
                message: format!("Unexpected top-level entry: {name_str}"),
                output_name: output_name.to_owned(),
                file_path: Some(Utf8PathBuf::from(name_str.as_ref())),
                details: None,
            });
        }
    }

    findings
}

/// Check for executable files in unexpected locations
fn check_misplaced_executables(output_name: &str, output_path: &Path) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    let exec_dirs: HashSet<&str> = ["bin", "sbin", "libexec"].into_iter().collect();

    for entry in WalkDir::new(output_path).follow_links(true) {
        let Ok(entry) = entry else { continue };

        if !entry.file_type().is_file() {
            continue;
        }

        let Ok(metadata) = entry.metadata() else {
            continue;
        };

        // Check if executable
        if metadata.permissions().mode() & 0o111 == 0 {
            continue;
        }

        let path = entry.path();

        // Skip shared libraries
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.contains(".so") {
                continue;
            }
        }

        let Ok(relative) = path.strip_prefix(output_path) else {
            continue;
        };

        // Check if under an expected executable directory
        let in_exec_dir = relative
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .is_some_and(|top| exec_dirs.contains(top));

        if !in_exec_dir {
            findings.push(AuditFinding {
                severity: Severity::Info,
                category: CheckCategory::LayoutIssues,
                check_name: "misplaced_executable".to_owned(),
                message: "Executable file outside bin/, sbin/, or libexec/".to_owned(),
                output_name: output_name.to_owned(),
                file_path: to_utf8(relative),
                details: None,
            });
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Category 3: Binary Issues
// ---------------------------------------------------------------------------

/// Check RPATH/RUNPATH entries in ELF binaries for problematic paths
async fn check_rpaths(output_name: &str, output_path: &Path) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    // Check if patchelf is available
    let patchelf_available = tokio::process::Command::new("patchelf")
        .arg("--version")
        .output()
        .await
        .is_ok();

    if !patchelf_available {
        debug!("patchelf not available, skipping RPATH check");
        return findings;
    }

    for entry in WalkDir::new(output_path).follow_links(true) {
        let Ok(entry) = entry else { continue };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        if !is_elf(path) {
            continue;
        }

        let output = match tokio::process::Command::new("patchelf")
            .arg("--print-rpath")
            .arg(path)
            .output()
            .await
        {
            Ok(o) if o.status.success() => o,
            _ => continue,
        };

        let rpath = String::from_utf8_lossy(&output.stdout);
        let rpath = rpath.trim();

        if rpath.is_empty() {
            continue;
        }

        let relative = path.strip_prefix(output_path).unwrap_or(path);

        for entry_path in rpath.split(':') {
            let entry_path = entry_path.trim();
            if entry_path.is_empty() {
                continue;
            }

            // Check for build directory paths baked into RPATH
            if entry_path.contains("/build/")
                || entry_path.starts_with("/tmp/")
                || entry_path.contains("/homeless-shelter/")
            {
                findings.push(AuditFinding {
                    severity: Severity::Error,
                    category: CheckCategory::BinaryIssues,
                    check_name: "build_dir_in_rpath".to_owned(),
                    message: "RPATH contains build/temporary directory".to_owned(),
                    output_name: output_name.to_owned(),
                    file_path: to_utf8(relative),
                    details: Some(entry_path.to_owned()),
                });
            }
            // Check for FHS paths in RPATH
            else if entry_path.starts_with("/usr/")
                || entry_path == "/lib"
                || entry_path == "/lib64"
            {
                findings.push(AuditFinding {
                    severity: Severity::Warning,
                    category: CheckCategory::BinaryIssues,
                    check_name: "fhs_rpath".to_owned(),
                    message: "RPATH contains FHS path (won't work in Nix)".to_owned(),
                    output_name: output_name.to_owned(),
                    file_path: to_utf8(relative),
                    details: Some(entry_path.to_owned()),
                });
            }
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Category 4: Script Issues
// ---------------------------------------------------------------------------

/// Check for shebangs pointing to FHS paths instead of Nix store paths
fn check_shebangs(output_name: &str, output_path: &Path) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    for entry in WalkDir::new(output_path).follow_links(true) {
        let Ok(entry) = entry else { continue };

        if !entry.file_type().is_file() {
            continue;
        }

        let Ok(metadata) = entry.metadata() else {
            continue;
        };

        // Only check executable files
        if metadata.permissions().mode() & 0o111 == 0 {
            continue;
        }

        let path = entry.path();

        // Read first 256 bytes
        let mut buf = [0u8; 256];
        let Ok(bytes_read) = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)) else {
            continue;
        };

        if bytes_read < 2 || buf[0] != b'#' || buf[1] != b'!' {
            continue;
        }

        let header = String::from_utf8_lossy(&buf[2..bytes_read]);
        let shebang_line = header.lines().next().unwrap_or("");
        let interpreter = shebang_line.split_whitespace().next().unwrap_or("");

        if interpreter.is_empty() {
            continue;
        }

        let relative = path.strip_prefix(output_path).unwrap_or(path);

        if interpreter == "/bin/sh" {
            // /bin/sh is acceptable in NixOS via the compatibility wrapper
            findings.push(AuditFinding {
                severity: Severity::Info,
                category: CheckCategory::ScriptIssues,
                check_name: "bin_sh_shebang".to_owned(),
                message: "Uses /bin/sh shebang (works on NixOS but not fully pure)".to_owned(),
                output_name: output_name.to_owned(),
                file_path: to_utf8(relative),
                details: Some(format!("#!{shebang_line}")),
            });
        } else if interpreter.starts_with("/usr/bin/")
            || interpreter.starts_with("/usr/local/")
            || (interpreter.starts_with("/bin/") && interpreter != "/bin/sh")
        {
            findings.push(AuditFinding {
                severity: Severity::Error,
                category: CheckCategory::ScriptIssues,
                check_name: "fhs_shebang".to_owned(),
                message: format!("Shebang uses FHS path: {interpreter}"),
                output_name: output_name.to_owned(),
                file_path: to_utf8(relative),
                details: Some(format!("#!{shebang_line}")),
            });
        }
    }

    findings
}

/// Scan script files for hardcoded FHS paths
fn check_fhs_references(output_name: &str, output_path: &Path) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    // Rust regex doesn't support lookahead, so match /etc/ paths separately
    let fhs_re = match Regex::new(r"/usr/(?:bin|lib|share|include|local)|/opt/") {
        Ok(re) => re,
        Err(e) => {
            warn!("Failed to compile FHS regex: {}", e);
            return findings;
        },
    };

    let etc_re = match Regex::new(r"/etc/[a-zA-Z]") {
        Ok(re) => re,
        Err(e) => {
            warn!("Failed to compile /etc regex: {}", e);
            return findings;
        },
    };

    let script_extensions: HashSet<&str> =
        ["sh", "bash", "py", "pl", "rb", "lua", "fish", "zsh", "csh"]
            .into_iter()
            .collect();

    for entry in WalkDir::new(output_path).follow_links(true) {
        let Ok(entry) = entry else { continue };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        // Only check script-like files (by extension or shebang)
        let is_script_ext = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| script_extensions.contains(ext));

        if !is_script_ext {
            // Check for shebang
            let mut magic = [0u8; 2];
            let has_shebang = std::fs::File::open(path)
                .and_then(|mut f| f.read_exact(&mut magic))
                .is_ok()
                && magic == [b'#', b'!'];
            if !has_shebang {
                continue;
            }
        }

        let Ok(metadata) = entry.metadata() else {
            continue;
        };

        // Skip files larger than 1MB
        if metadata.len() > 1024 * 1024 {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };

        let relative = path.strip_prefix(output_path).unwrap_or(path);
        let mut file_matches = HashSet::new();

        for (line_num, line) in content.lines().enumerate() {
            // Skip comment lines (best effort)
            let trimmed = line.trim();
            if trimmed.starts_with('#') || trimmed.starts_with("//") {
                continue;
            }

            // Check /usr/ and /opt/ paths
            for mat in fhs_re.find_iter(line) {
                let matched = mat.as_str();
                if file_matches.insert(matched.to_owned()) {
                    findings.push(AuditFinding {
                        severity: Severity::Warning,
                        category: CheckCategory::ScriptIssues,
                        check_name: "fhs_path_reference".to_owned(),
                        message: format!("Hardcoded FHS path: {matched}"),
                        output_name: output_name.to_owned(),
                        file_path: to_utf8(relative),
                        details: Some(format!("line {}: {trimmed}", line_num + 1)),
                    });
                }
            }

            // Check /etc/ paths (excluding /etc/nix)
            for mat in etc_re.find_iter(line) {
                let matched = mat.as_str();
                if matched.starts_with("/etc/nix") || matched.starts_with("/etc/n") {
                    continue;
                }
                let key = format!("/etc/{matched}");
                if file_matches.insert(key) {
                    findings.push(AuditFinding {
                        severity: Severity::Warning,
                        category: CheckCategory::ScriptIssues,
                        check_name: "fhs_path_reference".to_owned(),
                        message: format!("Hardcoded FHS path: {matched}"),
                        output_name: output_name.to_owned(),
                        file_path: to_utf8(relative),
                        details: Some(format!("line {}: {trimmed}", line_num + 1)),
                    });
                }
            }
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Category 6: Pkg-config Issues
// ---------------------------------------------------------------------------

/// Check that .pc (pkg-config) files reference valid paths
fn check_pkg_config(output_name: &str, output_path: &Path) -> Vec<AuditFinding> {
    let mut findings = Vec::new();

    // Path-valued fields in .pc files
    let path_fields = [
        "prefix",
        "exec_prefix",
        "libdir",
        "includedir",
        "sharedlibdir",
        "bindir",
        "datarootdir",
        "datadir",
        "sysconfdir",
        "mandir",
        "docdir",
        "infodir",
    ];

    for entry in WalkDir::new(output_path).follow_links(true) {
        let Ok(entry) = entry else { continue };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        // Only check .pc files
        let is_pc = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext == "pc");
        if !is_pc {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };

        let relative = path.strip_prefix(output_path).unwrap_or(path);

        // Collect variable definitions for substitution (e.g. prefix=/nix/store/...)
        let mut vars: Vec<(String, String)> = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            // Variable definitions use '=' without a colon
            // Field definitions use ':' (e.g. "Libs: -L${libdir}")
            if trimmed.starts_with('#') || trimmed.is_empty() || trimmed.contains(':') {
                continue;
            }
            if let Some((key, value)) = trimmed.split_once('=') {
                vars.push((key.trim().to_owned(), value.trim().to_owned()));
            }
        }

        // Resolve a value by substituting ${var} references
        let resolve = |value: &str| -> String {
            let mut result = value.to_owned();
            // Iterate a few times to handle chained substitutions
            for _ in 0..5 {
                let mut changed = false;
                for (k, v) in &vars {
                    let pattern = format!("${{{k}}}");
                    if result.contains(&pattern) {
                        result = result.replace(&pattern, v);
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            result
        };

        for (key, raw_value) in &vars {
            if !path_fields.contains(&key.as_str()) {
                continue;
            }

            let resolved = resolve(raw_value);

            // Only validate absolute paths (skip empty or relative)
            if !resolved.starts_with('/') {
                continue;
            }

            if !Path::new(&resolved).exists() {
                findings.push(AuditFinding {
                    severity: Severity::Error,
                    category: CheckCategory::BrokenReferences,
                    check_name: "pkg_config_bad_path".to_owned(),
                    message: format!(".pc file {key} references non-existent path"),
                    output_name: output_name.to_owned(),
                    file_path: to_utf8(relative),
                    details: Some(format!("{key}={resolved}")),
                });
            }
        }

        // Also check -L and -I flags in Libs/Cflags lines for bad paths
        for line in content.lines() {
            let trimmed = line.trim();
            let field_value = trimmed
                .strip_prefix("Libs:")
                .or_else(|| trimmed.strip_prefix("Libs.private:"))
                .or_else(|| trimmed.strip_prefix("Cflags:"));

            let Some(value) = field_value else {
                continue;
            };

            let resolved = resolve(value);

            for token in resolved.split_whitespace() {
                let dir = token
                    .strip_prefix("-L")
                    .or_else(|| token.strip_prefix("-I"));

                let Some(dir) = dir else { continue };

                if dir.starts_with('/') && !Path::new(dir).exists() {
                    findings.push(AuditFinding {
                        severity: Severity::Error,
                        category: CheckCategory::BrokenReferences,
                        check_name: "pkg_config_bad_path".to_owned(),
                        message: ".pc file references non-existent directory in flags".to_owned(),
                        output_name: output_name.to_owned(),
                        file_path: to_utf8(relative),
                        details: Some(format!("{token} (resolved: {dir})")),
                    });
                }
            }
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Category 7: Metadata Issues
// ---------------------------------------------------------------------------

/// Check package metadata for missing fields
async fn check_metadata(eval_entry_point: &str, attr_path: &str) -> Vec<AuditFinding> {
    use crate::nix::{has_attr, normalize_entry_point};

    let mut findings = Vec::new();
    let normalized = normalize_entry_point(eval_entry_point);

    // Check meta.description
    let has_desc = has_attr(&normalized, &format!("{attr_path}.meta"), "description")
        .await
        .unwrap_or(false);
    if !has_desc {
        findings.push(AuditFinding {
            severity: Severity::Info,
            category: CheckCategory::MetadataIssues,
            check_name: "missing_description".to_owned(),
            message: "meta.description is not set".to_owned(),
            output_name: "meta".to_owned(),
            file_path: None,
            details: None,
        });
    }

    // Check meta.license
    let has_license = has_attr(&normalized, &format!("{attr_path}.meta"), "license")
        .await
        .unwrap_or(false);
    if !has_license {
        findings.push(AuditFinding {
            severity: Severity::Warning,
            category: CheckCategory::MetadataIssues,
            check_name: "missing_license".to_owned(),
            message: "meta.license is not set".to_owned(),
            output_name: "meta".to_owned(),
            file_path: None,
            details: None,
        });
    }

    // Check meta.homepage
    let has_homepage = has_attr(&normalized, &format!("{attr_path}.meta"), "homepage")
        .await
        .unwrap_or(false);
    if !has_homepage {
        findings.push(AuditFinding {
            severity: Severity::Info,
            category: CheckCategory::MetadataIssues,
            check_name: "missing_homepage".to_owned(),
            message: "meta.homepage is not set".to_owned(),
            output_name: "meta".to_owned(),
            file_path: None,
            details: None,
        });
    }

    findings
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// Run all enabled audit checks on the given output paths
pub async fn run_audit(
    output_paths: &[(String, std::path::PathBuf)],
    eval_entry_point: Option<&str>,
    attr_path: Option<&str>,
    config: &AuditConfig,
) -> AuditReport {
    let start = std::time::Instant::now();
    let mut findings = Vec::new();
    let mut checks_run = 0;

    for (output_name, output_path) in output_paths {
        if config.check_symlinks {
            checks_run += 1;
            findings.extend(check_broken_symlinks(output_name, output_path));
        }

        if config.check_store_references {
            checks_run += 1;
            findings.extend(check_store_references(output_name, output_path));
        }

        if config.check_shared_libs {
            checks_run += 1;
            findings.extend(check_shared_libs(output_name, output_path).await);
        }

        if config.check_layout {
            checks_run += 1;
            findings.extend(check_layout(output_name, output_path));
            findings.extend(check_misplaced_executables(output_name, output_path));
        }

        if config.check_rpaths {
            checks_run += 1;
            findings.extend(check_rpaths(output_name, output_path).await);
        }

        if config.check_shebangs {
            checks_run += 1;
            findings.extend(check_shebangs(output_name, output_path));
        }

        if config.check_fhs_references {
            checks_run += 1;
            findings.extend(check_fhs_references(output_name, output_path));
        }

        if config.check_pkg_config {
            checks_run += 1;
            findings.extend(check_pkg_config(output_name, output_path));
        }
    }

    // Metadata checks (need eval context)
    if config.check_metadata {
        if let (Some(entry), Some(attr)) = (eval_entry_point, attr_path) {
            checks_run += 1;
            findings.extend(check_metadata(entry, attr).await);
        }
    }

    // Filter by minimum severity
    findings.retain(|f| f.severity >= config.min_severity);

    // Sort: errors first, then warnings, then info
    findings.sort_by(|a, b| b.severity.cmp(&a.severity));

    let duration_ms = start.elapsed().as_millis() as u64;

    AuditReport {
        attr_path: attr_path.unwrap_or("<unknown>").to_owned(),
        store_paths: output_paths.to_vec(),
        findings,
        checks_run,
        duration_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_elf_non_existent() {
        assert!(!is_elf(Path::new("/nonexistent/path")));
    }

    #[test]
    fn test_to_utf8() {
        let p = Path::new("bin/hello");
        assert_eq!(to_utf8(p), Some(Utf8PathBuf::from("bin/hello")));
    }
}
