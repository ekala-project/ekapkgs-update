use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Context;
use regex::Regex;
use tokio::process::Command;
use tracing::debug;

/// Extra arguments appended to every `nix-build` invocation.
/// Set once at process startup (e.g., `--builders 'external'`).
static EXTRA_NIX_BUILD_ARGS: OnceLock<Vec<String>> = OnceLock::new();

/// Initialize extra nix-build arguments. Must be called before any builds.
/// Calling more than once is a no-op (OnceLock semantics).
pub fn set_extra_nix_build_args(args: Vec<String>) {
    let _ = EXTRA_NIX_BUILD_ARGS.set(args);
}

pub fn get_extra_nix_build_args() -> &'static [String] {
    EXTRA_NIX_BUILD_ARGS.get().map_or(&[], |v| v.as_slice())
}

/// Build Nix expression and return stdout/stderr
///
/// Returns a tuple of (success, stdout, stderr)
pub async fn build_nix_expr(
    eval_entry_point: &str,
    attr_path: &str,
    attr_suffix: Option<&str>,
) -> anyhow::Result<(bool, String, String)> {
    let full_attr = if let Some(suffix) = attr_suffix {
        format!("{attr_path}.{suffix}")
    } else {
        attr_path.to_owned()
    };

    debug!("Building {}", full_attr);

    let mut cmd = Command::new("nix-build");
    cmd.arg(eval_entry_point).arg("-A").arg(&full_attr);
    for arg in get_extra_nix_build_args() {
        cmd.arg(arg);
    }
    let output = cmd.output().await?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((output.status.success(), stdout, stderr))
}

/// Build Nix expression for a specific target system
///
/// Like [`build_nix_expr`] but passes `--system <system>` so that
/// `builtins.currentSystem` resolves to the requested platform.
#[allow(dead_code)] // public API, not called yet
pub async fn build_nix_expr_for_system(
    eval_entry_point: &str,
    attr_path: &str,
    attr_suffix: Option<&str>,
    system: &str,
) -> anyhow::Result<(bool, String, String)> {
    let full_attr = if let Some(suffix) = attr_suffix {
        format!("{attr_path}.{suffix}")
    } else {
        attr_path.to_owned()
    };

    debug!("Building {} for system {}", full_attr, system);

    let mut cmd = Command::new("nix-build");
    cmd.arg(eval_entry_point)
        .arg("-A")
        .arg(&full_attr)
        .arg("--system")
        .arg(system);
    for arg in get_extra_nix_build_args() {
        cmd.arg(arg);
    }
    let output = cmd.output().await?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((output.status.success(), stdout, stderr))
}

/// Build a flake package and return stdout/stderr
///
/// Uses `nix build <installable>` to build flake packages.
///
/// # Arguments
/// * `installable` - The flake installable path (e.g., ".#hello" or
///   ".#packages.x86_64-linux.hello")
/// * `attr_suffix` - Optional suffix to append (e.g., "passthru.tests")
///
/// # Returns
/// A tuple of (success, stdout, stderr)
pub async fn build_flake_package(
    installable: &str,
    attr_suffix: Option<&str>,
) -> anyhow::Result<(bool, String, String)> {
    let full_installable = if let Some(suffix) = attr_suffix {
        format!("{installable}.{suffix}")
    } else {
        installable.to_owned()
    };

    debug!("Building flake package: {}", full_installable);

    let output = Command::new("nix")
        .arg("build")
        .arg(&full_installable)
        .arg("--no-link") // Don't create result symlink
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((output.status.success(), stdout, stderr))
}

/// Detect reversed patch errors and extract the patch filename
///
/// Looks for "Reversed (or previously applied) patch detected!" in the last 20 lines
/// and extracts the patch name from the preceding "applying patch" line.
///
/// Returns the patch filename to be removed from the patches array.
pub fn detect_reversed_patch(stderr: &str) -> Option<String> {
    let patch_regex = Regex::new(r"applying patch /nix/store/[^-]+-(.+)").ok()?;

    // Take last 20 lines in reverse order
    let last_lines: Vec<_> = stderr.lines().rev().take(20).collect();

    // Find position of error message
    last_lines
        .iter()
        .position(|line| line.contains("Reversed (or previously applied) patch detected!"))
        .and_then(|error_pos| {
            // From error position onward, find first patch name
            last_lines[error_pos..].iter().find_map(|line| {
                patch_regex
                    .captures(line)
                    .and_then(|caps| caps.get(1))
                    .map(|m| m.as_str().to_owned())
            })
        })
}

/// Temporary directory holding `nix-build` out-links, removed on drop.
///
/// Every build gets its own directory so concurrent builds (run/watch
/// update many packages at once) never share or clobber `result*` links.
struct OutLinkDir(PathBuf);

impl OutLinkDir {
    fn create() -> anyhow::Result<Self> {
        let dir =
            std::env::temp_dir().join(format!("ekapkgs-update-build-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create out-link directory {}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for OutLinkDir {
    fn drop(&mut self) {
        // Removing the links only drops their GC roots; the outputs stay in
        // the store until the next garbage collection.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Build a Nix expression and return all output paths
///
/// Builds the package and returns a list of (output_name, store_path) tuples
/// for all outputs (e.g., "out", "dev", "lib", etc.), with "out" first.
///
/// This never reads or changes the process working directory: `nix-build`
/// runs in `work_dir` (when given, so a relative `eval_entry_point`
/// resolves against it) and its out-links go to a private temporary
/// directory that is removed before returning. It is safe to call
/// concurrently.
///
/// # Arguments
/// * `work_dir` - Directory to run `nix-build` in (`None`: inherit the cwd)
/// * `eval_entry_point` - Path to the Nix file to evaluate
/// * `attr_path` - Attribute path to build
/// * `attr_suffix` - Optional suffix to append
///
/// # Returns
/// A vector of (output_name, store_path) tuples, or an error if the build failed
pub async fn build_and_get_outputs(
    work_dir: Option<&Path>,
    eval_entry_point: &str,
    attr_path: &str,
    attr_suffix: Option<&str>,
) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let full_attr = if let Some(suffix) = attr_suffix {
        format!("{attr_path}.{suffix}")
    } else {
        attr_path.to_owned()
    };

    debug!("Building {} and extracting outputs", full_attr);

    let links = OutLinkDir::create()?;
    let out_link = links.0.join("result");

    let mut cmd = Command::new("nix-build");
    cmd.arg(eval_entry_point)
        .arg("-A")
        .arg(&full_attr)
        .arg("-o")
        .arg(&out_link);
    if let Some(dir) = work_dir {
        cmd.current_dir(dir);
    }
    for arg in get_extra_nix_build_args() {
        cmd.arg(arg);
    }
    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Build failed for {full_attr}: {stderr}");
    }

    collect_out_links(&links.0)
}

/// Read the `result` / `result-<output>` links `nix-build -o <dir>/result`
/// created, as (output_name, store_path) with "out" first and the rest
/// sorted by name.
fn collect_out_links(dir: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let mut outputs = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let output_name = match name.strip_prefix("result") {
            Some("") => "out",
            Some(rest) => match rest.strip_prefix('-') {
                Some(suffix) if !suffix.is_empty() => suffix,
                _ => continue,
            },
            None => continue,
        };
        let store_path = std::fs::canonicalize(entry.path())
            .with_context(|| format!("resolve build output link {name}"))?;
        outputs.push((output_name.to_owned(), store_path));
    }
    outputs.sort_by(|(a, _), (b, _)| (a != "out", a).cmp(&(b != "out", b)));
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn collect_out_links_names_outputs_out_first() {
        let links = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        for name in ["out", "dev", "lib", "man"] {
            std::fs::create_dir(store.path().join(name)).unwrap();
        }
        symlink(store.path().join("man"), links.path().join("result-man")).unwrap();
        symlink(store.path().join("out"), links.path().join("result")).unwrap();
        symlink(store.path().join("dev"), links.path().join("result-dev")).unwrap();
        symlink(store.path().join("lib"), links.path().join("result-lib")).unwrap();
        // Not out-links.
        std::fs::write(links.path().join("resultx"), "").unwrap();
        std::fs::write(links.path().join("result-"), "").unwrap();
        std::fs::write(links.path().join("other"), "").unwrap();

        let outputs = collect_out_links(links.path()).unwrap();
        let names: Vec<_> = outputs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["out", "dev", "lib", "man"]);
        let store = std::fs::canonicalize(store.path()).unwrap();
        assert_eq!(outputs[0].1, store.join("out"));
        assert_eq!(outputs[3].1, store.join("man"));
    }

    #[test]
    fn out_link_dirs_are_unique_and_removed_on_drop() {
        let a = OutLinkDir::create().unwrap();
        let b = OutLinkDir::create().unwrap();
        assert_ne!(a.0, b.0);
        let path = a.0.clone();
        assert!(path.is_dir());
        drop(a);
        assert!(!path.exists());
    }
}
