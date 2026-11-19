//! Retry command for resuming failed updates

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use tokio::fs;
use tokio::process::Command;
use tracing::info;

use crate::commands::run::preservation::{find_preserved, list_preserved_failures};

/// Retry a failed update from preserved worktree
pub async fn retry(
    database_path: String,
    attr_path: String,
    repo: Option<String>,
    from_phase: Option<String>,
    apply_patch: Option<PathBuf>,
    version: Option<String>,
) -> anyhow::Result<()> {
    let _expanded_path = shellexpand::tilde(&database_path).to_string();

    if let Some(phase) = from_phase {
        bail!(
            "--from-phase is not implemented yet (requested: {phase}); retry only prepares a copy \
             of the preserved worktree for manual work"
        );
    }
    if let Some(version) = version {
        bail!("--version is not implemented yet (requested: {version})");
    }

    // Resolve the patch before anything else: a relative path is relative
    // to the directory the command was run from.
    let apply_patch = apply_patch
        .map(|p| std::fs::canonicalize(&p).with_context(|| format!("patch file {}", p.display())))
        .transpose()?;

    info!("Attempting to retry update for: {}", attr_path);

    // Find the most recent preserved failure for this package
    let artifacts_list = list_preserved_failures().await?;
    let artifact = find_preserved(&artifacts_list, &attr_path, repo.as_deref())?;

    info!(
        "Found preserved failure from session {} (phase: {})",
        artifact.session_id,
        artifact.failed_phase.as_str()
    );

    // Verify worktree exists
    if !artifact.worktree_path.exists() {
        bail!(
            "Preserved worktree no longer exists at: {}",
            artifact.worktree_path.display()
        );
    }

    // Create a temporary working directory for the retry
    let retry_dir = std::env::temp_dir().join(format!(
        "ekapkgs-retry-{}-{}",
        attr_path.replace(['.', '/'], "_"),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&retry_dir).await?;

    info!("Copying worktree to: {}", retry_dir.display());

    // Copy the preserved worktree to the retry directory
    copy_dir_all(&artifact.worktree_path, &retry_dir).await?;

    // Apply patch if provided
    if let Some(ref patch_path) = apply_patch {
        info!("Applying patch: {}", patch_path.display());
        apply_patch_file(&retry_dir, patch_path).await?;
    }

    info!("Retrying update for {}", attr_path);

    // For now, we'll just invoke the update command with the current state
    // In a full implementation, we would:
    // 1. Detect the eval entry point from the worktree
    // 2. Determine the version config
    // 3. Skip phases that already succeeded if resuming

    // Read metadata from preserved artifacts to understand the update parameters
    let metadata_path = artifact
        .worktree_path
        .parent()
        .context("No parent directory")?
        .join("metadata.json");

    if !metadata_path.exists() {
        bail!("Cannot retry: metadata not found in preserved artifacts");
    }

    // For now, provide instructions to the user
    println!("\n{}", "=".repeat(80));
    println!("Retry Environment Prepared");
    println!("{}", "=".repeat(80));
    println!();
    println!("Working directory: {}", retry_dir.display());
    println!("Package:           {}", attr_path);
    println!("Failed phase:      {}", artifact.failed_phase.as_str());
    println!();

    if apply_patch.is_some() {
        println!("Patch applied successfully.");
        println!();
    }

    println!("Next steps:");
    println!(
        "1. Review changes with: git -C {} diff",
        retry_dir.display()
    );
    println!("2. Manually run update command:");
    println!("   cd {}", retry_dir.display());
    println!("   ekapkgs-update update <file> {} [options]", attr_path);
    println!();
    println!("Or, build and test manually:");
    println!("   cd {}", retry_dir.display());
    println!("   nix-build -A {}", attr_path);
    println!();
    println!("{}", "=".repeat(80));
    println!();
    println!("Note: Automatic retry execution is not yet fully implemented.");
    println!("The worktree has been prepared for manual intervention.");
    println!();

    Ok(())
}

/// Whether a symlink named `name` is a `nix-build` out-link (`result`,
/// `result-dev`, …), which is not part of the tree and is not copied.
fn is_out_link(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|n| n == "result" || n.starts_with("result-"))
}

/// Copy directory recursively (async version). Symlinks are copied as
/// symlinks (never followed into the Nix store), except out-links.
async fn copy_dir_all(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst).await?;
    let mut entries = fs::read_dir(src).await?;

    while let Some(entry) = entries.next_entry().await? {
        // `DirEntry::file_type` does not follow symlinks.
        let ty = entry.file_type().await?;
        let name = entry.file_name();
        let dst_path = dst.join(&name);

        if ty.is_symlink() {
            if is_out_link(&name) {
                continue;
            }
            let target = fs::read_link(entry.path()).await?;
            fs::symlink(&target, &dst_path)
                .await
                .with_context(|| format!("copy symlink {}", entry.path().display()))?;
        } else if ty.is_dir() {
            Box::pin(copy_dir_all(&entry.path(), &dst_path)).await?;
        } else {
            fs::copy(entry.path(), dst_path).await?;
        }
    }
    Ok(())
}

/// Apply a patch file to the tree in `dir`
async fn apply_patch_file(dir: &Path, patch_path: &Path) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(["apply", "--verbose"])
        .arg(patch_path)
        .current_dir(dir)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to apply patch: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        info!("Patch output: {}", stdout);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_links_are_result_symlinks() {
        for (name, expected) in [
            ("result", true),
            ("result-dev", true),
            ("result-bin", true),
            ("results", false),
            ("default.nix", false),
        ] {
            assert_eq!(is_out_link(std::ffi::OsStr::new(name)), expected, "{name}");
        }
    }

    #[tokio::test]
    async fn copy_dir_all_keeps_symlinks_and_skips_out_links() {
        let tmp = std::env::temp_dir().join(format!("ekapkgs-retry-test-{}", uuid::Uuid::new_v4()));
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        std::fs::create_dir_all(src.join("pkgs")).unwrap();
        std::fs::write(src.join("pkgs/default.nix"), "{ }").unwrap();
        std::os::unix::fs::symlink("pkgs/default.nix", src.join("link.nix")).unwrap();
        std::os::unix::fs::symlink("/nonexistent/store/path", src.join("result")).unwrap();

        copy_dir_all(&src, &dst).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.join("pkgs/default.nix")).unwrap(),
            "{ }"
        );
        assert_eq!(
            std::fs::read_link(dst.join("link.nix")).unwrap(),
            PathBuf::from("pkgs/default.nix")
        );
        assert!(std::fs::symlink_metadata(dst.join("result")).is_err());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
