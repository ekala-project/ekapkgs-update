use std::path::{Path, PathBuf};

use anyhow::Context;
use tracing::{debug, info, warn};

use super::{ReleaseTag, build_nix_expr, build_nix_expr_with, update_hash_attr, update_nix_file};
use crate::hash_discovery;

/// The invalid hash written to provoke a hash mismatch.
const FAKE_HASH: &str = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
use crate::package::PackageQuery;
use crate::pypi::sdist::{
    SdistFix, Wheel, WheelFix, plan_sdist_fix, plan_wheel_fix, wheel_url_dist,
};
use crate::pypi::{PypiFile, fetch_release_files};
use crate::rewrite::{update_fetch_pypi_sdist, update_fetch_pypi_wheel};

/// Update source hash using the invalid hash discovery pattern
///
/// `pypi_pname` is the PyPI project for packages whose upstream is PyPI; the
/// `fetchPypi` filename is then checked against the files PyPI lists for the
/// new release before the hash is discovered (see [`align_pypi_src`]).
pub async fn update_source_hash(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_version: &str,
    new_version: &str,
    old_hash: Option<&str>,
    release_tag: Option<ReleaseTag<'_>>,
    pypi_pname: Option<&str>,
) -> anyhow::Result<PathBuf> {
    // Files to put back if the source can't be updated
    let package_dir = file_location.parent().unwrap_or(file_location);
    let snapshot = super::file_update::package_files(package_dir).await?;

    // Step 1: Update version in file with invalid hash
    let invalid_hash = FAKE_HASH;
    let actual_file_location = update_nix_file(
        eval_entry_point,
        attr_path,
        file_location,
        old_version,
        new_version,
        old_hash,
        Some(invalid_hash),
        release_tag,
    )
    .await?;

    info!(
        "Updated version and set invalid hash in {}",
        actual_file_location.display()
    );

    let discovered = discover_source_hash(
        eval_entry_point,
        attr_path,
        &actual_file_location,
        (old_version, new_version),
        old_hash,
        pypi_pname,
    )
    .await;
    let correct_hash = match discovered {
        Ok(hash) => hash,
        Err(e) => {
            // Leave the package as it was: no fake hash, no half-done bump.
            for (path, content) in &snapshot {
                if let Err(restore) = tokio::fs::write(path, content).await {
                    warn!("Could not restore {}: {restore}", path.display());
                }
            }
            return Err(e);
        },
    };

    // Step 3: Update hash with correct value
    update_nix_file(
        eval_entry_point,
        attr_path,
        &actual_file_location,
        new_version,
        new_version,
        Some(invalid_hash),
        Some(&correct_hash),
        None, // rev already fixed in step 1
    )
    .await?;

    info!("Updated hash in {}", actual_file_location.display());

    // Step 4: Build source again to verify
    let (success, _stdout, stderr) =
        build_nix_expr(eval_entry_point, attr_path, Some("src")).await?;

    if !success {
        anyhow::bail!("Source build failed after hash update:\n{stderr}");
    }

    info!("Source build successful");

    Ok(actual_file_location)
}

/// Steps 2 of [`update_source_hash`]: with the fake hash written, align a PyPI
/// source and learn the real hash from the mismatch.
async fn discover_source_hash(
    eval_entry_point: &str,
    attr_path: &str,
    file: &Path,
    versions: (&str, &str),
    old_hash: Option<&str>,
    pypi_pname: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(pypi_pname) = pypi_pname {
        align_pypi_src(&PypiSrc {
            query: PackageQuery::new(eval_entry_point, attr_path),
            attr_path,
            nix_file: file,
            pypi_pname,
            version: versions.1,
        })
        .await?;
    }

    // Step 2: Build source to get correct hash
    let (success, _stdout, stderr) =
        build_nix_expr(eval_entry_point, attr_path, Some("src")).await?;

    if success {
        warn!("Build succeeded with invalid hash - this shouldn't happen");
        anyhow::bail!("Expected hash mismatch error but build succeeded");
    }

    let correct_hash = hash_discovery::extract_hash_for(&stderr, FAKE_HASH).ok_or_else(|| {
        anyhow::anyhow!("Could not extract correct hash from build error:\n{stderr}")
    })?;

    info!("Extracted correct hash: {}", correct_hash);

    // A new version with the old source means the fetcher doesn't follow the
    // version (a literal tag, rev or URL): the "update" would only relabel it.
    let (old_version, new_version) = versions;
    if old_version != new_version
        && old_hash.is_some_and(|old| super::fod_refresh::same_hash(&correct_hash, old))
    {
        anyhow::bail!(
            "The source of {attr_path} did not change from {old_version} to {new_version} \
             (hash {correct_hash}): its fetcher does not follow the version. Make its \
             tag/rev/url derive from `version`."
        );
    }
    Ok(correct_hash)
}

/// A package whose `fetchPypi` source is being checked against PyPI.
struct PypiSrc<'a> {
    query: PackageQuery,
    attr_path: &'a str,
    nix_file: &'a Path,
    pypi_pname: &'a str,
    version: &'a str,
}

/// Make the `fetchPypi` source match a file PyPI publishes for `version`
///
/// Projects often rename their sdists to the normalised form between releases
/// (`sphinxcontrib-openapi-0.8.4.tar.gz` → `sphinxcontrib_openapi-0.10.0.tar.gz`),
/// change the archive type (`.zip` → `.tar.gz`), or change their wheel's python
/// tag (`py2.py3` → `py3`). `fetchPypi` derives the URL from its arguments, so
/// after a version bump the URL 404s. When the release has a file that is a
/// spelling of the same one, set the `fetchPypi` arguments that differ:
/// `pname` and/or `extension` for sdists ([`align_pypi_sdist`]); `pname`,
/// `python` and/or `dist` for wheels ([`align_pypi_wheel`]).
///
/// An sdist-based package whose release has only wheels fails here with a
/// clear error rather than a 404. Anything else that does not fit (unrelated
/// names, a missing platform wheel, a PyPI outage, a `fetchPypi` call that
/// can't be rewritten) is logged and left for the source build to report.
async fn align_pypi_src(src: &PypiSrc<'_>) -> anyhow::Result<()> {
    let Some(current_url) = first_src_url(&src.query).await else {
        debug!(
            "{}: no src URL to compare against PyPI files",
            src.attr_path
        );
        return Ok(());
    };
    let Some(current) = url_filename(&current_url) else {
        debug!("{}: src URL {current_url} has no filename", src.attr_path);
        return Ok(());
    };

    let files = match fetch_release_files(src.pypi_pname, src.version).await {
        Ok(files) => files,
        Err(e) => {
            warn!(
                "{}: could not list PyPI files for {} {}: {e:#}",
                src.attr_path, src.pypi_pname, src.version
            );
            return Ok(());
        },
    };

    if current.ends_with(".whl") {
        align_pypi_wheel(src, &current_url, current, &files).await
    } else {
        align_pypi_sdist(src, current, &files).await
    }
}

/// [`align_pypi_src`] for an sdist source.
async fn align_pypi_sdist(
    src: &PypiSrc<'_>,
    current: &str,
    files: &[PypiFile],
) -> anyhow::Result<()> {
    let sdists: Vec<&str> = files
        .iter()
        .filter(|f| f.packagetype == "sdist")
        .map(|f| f.filename.as_str())
        .collect();

    match plan_sdist_fix(current, src.version, &sdists) {
        SdistFix::Matches => Ok(()),
        SdistFix::NoSdist => {
            let wheels: Vec<&str> = files.iter().map(|f| f.filename.as_str()).collect();
            anyhow::bail!(
                "PyPI release {} {} has no sdist, only: {}. \
                 Fetch a wheel or the upstream source archive instead.",
                src.pypi_pname,
                src.version,
                wheels.join(", ")
            )
        },
        SdistFix::NoMatch => {
            warn!(
                "{}: src file {current} is not among the sdists PyPI lists for {} {}: {}",
                src.attr_path,
                src.pypi_pname,
                src.version,
                sdists.join(", ")
            );
            Ok(())
        },
        SdistFix::Rewrite {
            pname,
            extension,
            filename,
        } => {
            info!(
                "{}: PyPI sdist is now {filename} (was {current}); updating fetchPypi",
                src.attr_path
            );
            rewrite_fetch_pypi(src, &filename, None, |content| {
                update_fetch_pypi_sdist(content, pname.as_deref(), extension.as_deref())
            })
            .await
        },
    }
}

/// [`align_pypi_src`] for a wheel source (`format = "wheel";`).
async fn align_pypi_wheel(
    src: &PypiSrc<'_>,
    current_url: &str,
    current: &str,
    files: &[PypiFile],
) -> anyhow::Result<()> {
    let Some(current_dist) = wheel_url_dist(current_url) else {
        debug!(
            "{}: wheel URL {current_url} has no directory segment",
            src.attr_path
        );
        return Ok(());
    };
    let wheels: Vec<Wheel<'_>> = files
        .iter()
        .filter(|f| f.packagetype == "bdist_wheel")
        .map(|f| Wheel {
            filename: &f.filename,
            dist: &f.python_version,
        })
        .collect();

    match plan_wheel_fix(current, current_dist, src.version, &wheels) {
        WheelFix::Matches => Ok(()),
        WheelFix::NoMatch => {
            let available: Vec<&str> = files.iter().map(|f| f.filename.as_str()).collect();
            warn!(
                "{}: no wheel PyPI lists for {} {} matches src file {current}: {}",
                src.attr_path,
                src.pypi_pname,
                src.version,
                available.join(", ")
            );
            Ok(())
        },
        WheelFix::Rewrite {
            pname,
            python,
            dist,
            filename,
        } => {
            let new_dist = dist.as_deref().unwrap_or(current_dist).to_owned();
            info!(
                "{}: PyPI wheel is now {new_dist}/{filename} (was {current_dist}/{current}); \
                 updating fetchPypi",
                src.attr_path
            );
            rewrite_fetch_pypi(src, &filename, Some(&new_dist), |content| {
                update_fetch_pypi_wheel(
                    content,
                    pname.as_deref(),
                    python.as_deref(),
                    dist.as_deref(),
                )
            })
            .await
        },
    }
}

/// Apply a `fetchPypi` rewrite to the package file, then re-evaluate `src` and
/// check that it now requests `filename` (and, for wheels, from `dist`).
///
/// A rewrite that can't be applied is logged and skipped; a rewrite that
/// applies but evaluates to the wrong URL is an error.
async fn rewrite_fetch_pypi<E: std::fmt::Display>(
    src: &PypiSrc<'_>,
    filename: &str,
    dist: Option<&str>,
    rewrite: impl FnOnce(&str) -> Result<String, E>,
) -> anyhow::Result<()> {
    let nix_file = src.nix_file;
    let content = tokio::fs::read_to_string(nix_file)
        .await
        .with_context(|| format!("read nix file {}", nix_file.display()))?;
    let updated = match rewrite(&content) {
        Ok(updated) => updated,
        Err(e) => {
            warn!(
                "{}: could not rewrite fetchPypi in {} for {filename}: {e}",
                src.attr_path,
                nix_file.display()
            );
            return Ok(());
        },
    };
    tokio::fs::write(nix_file, updated)
        .await
        .with_context(|| format!("write nix file {}", nix_file.display()))?;

    let now = first_src_url(&src.query).await;
    let now_filename = now.as_deref().and_then(url_filename);
    let dist_ok = dist.is_none_or(|dist| now.as_deref().and_then(wheel_url_dist) == Some(dist));
    if now_filename == Some(filename) && dist_ok {
        return Ok(());
    }
    anyhow::bail!(
        "Rewrote fetchPypi for {}{filename}, but src now points at {}",
        dist.map(|d| format!("{d}/")).unwrap_or_default(),
        now.as_deref().unwrap_or("nothing")
    )
}

/// The package's first `src` URL.
async fn first_src_url(query: &PackageQuery) -> Option<String> {
    let urls = query.get_src_url().await?;
    urls.split_whitespace().next().map(str::to_owned)
}

/// Last path component of a URL.
fn url_filename(url: &str) -> Option<&str> {
    url.rsplit('/').next().filter(|name| !name.is_empty())
}

/// Update `cargoHash` for Rust packages (see [`discover_dependency_hash`]).
pub async fn update_cargo_hash_if_needed(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_hash: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(old_hash) = old_hash else {
        return Ok(None);
    };
    info!("Detected Rust package, updating cargoHash");
    discover_dependency_hash(
        eval_entry_point,
        attr_path,
        file_location,
        "cargoHash",
        old_hash,
    )
    .await
    .map(Some)
}

/// Update `vendorHash` for Go packages (see [`discover_dependency_hash`]).
pub async fn update_vendor_hash_if_needed(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_hash: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(old_hash) = old_hash else {
        return Ok(None);
    };
    info!("Detected Go package, updating vendorHash");
    discover_dependency_hash(
        eval_entry_point,
        attr_path,
        file_location,
        "vendorHash",
        old_hash,
    )
    .await
    .map(Some)
}

/// Build package with automatic recovery from reversed patches
/// Returns a list of patch names that were removed during the build process
pub async fn build_with_patch_recovery(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
) -> anyhow::Result<Vec<String>> {
    use super::{build_nix_expr, detect_reversed_patch};
    use crate::rewrite::{
        is_patches_array_empty, remove_patch_from_array, remove_patches_attribute,
    };

    let mut removed_patches = Vec::new();

    loop {
        let (success, _stdout, stderr) = build_nix_expr(eval_entry_point, attr_path, None).await?;

        if success {
            // Build succeeded - check if patches array is now empty
            let content = tokio::fs::read_to_string(file_location)
                .await
                .with_context(|| format!("read nix file {}", file_location.display()))?;
            if is_patches_array_empty(&content) {
                match remove_patches_attribute(&content) {
                    Ok(updated_content) => {
                        tokio::fs::write(file_location, updated_content)
                            .await
                            .with_context(|| {
                                format!("write nix file {}", file_location.display())
                            })?;
                        info!("Removed empty patches attribute");
                    },
                    Err(e) => {
                        debug!("Could not remove empty patches attribute: {}", e);
                        // Not a critical error, continue
                    },
                }
            }
            break;
        }

        // Build failed - check for reversed patch errors
        if let Some(patch_name) = detect_reversed_patch(&stderr) {
            info!("Detected reversed/already-applied patch: {}", patch_name);

            // Read the file
            let content = tokio::fs::read_to_string(file_location)
                .await
                .with_context(|| format!("read nix file {}", file_location.display()))?;

            // Remove the patch
            match remove_patch_from_array(&content, &patch_name) {
                Ok(updated_content) => {
                    // Write the updated content back
                    tokio::fs::write(file_location, updated_content)
                        .await
                        .with_context(|| format!("write nix file {}", file_location.display()))?;
                    info!("Removed obsolete patch: {}", patch_name);
                    removed_patches.push(patch_name.clone());

                    // Delete the patch file from the tree if it's a local file
                    // with no other references
                    delete_unreferenced_patch_file(file_location, &patch_name).await;

                    // Continue loop to retry the build
                },
                Err(e) => {
                    warn!("Failed to remove patch {}: {}", patch_name, e);
                    // Can't remove the patch, return the original error
                    anyhow::bail!(
                        "Package build failed after update. Detected reversed/already-applied \
                         patch '{patch_name}' but couldn't remove it from the patches array: \
                         {e}\n\nBuild error:\n{stderr}"
                    );
                },
            }
        } else {
            // No reversed patch detected - this is a real build failure
            warn!("Full package build failed:\n{}", stderr);
            anyhow::bail!(
                "Package build failed after update with no reversed patches detected. This \
                 indicates a real build issue that needs manual intervention.\n\nBuild \
                 error:\n{stderr}"
            );
        }
    }

    Ok(removed_patches)
}

/// Delete a patch file from the tree if it exists as a local file and has no
/// other references in the repository.
///
/// This is best-effort: failures are logged but do not block the update.
async fn delete_unreferenced_patch_file(nix_file: &Path, patch_name: &str) {
    let Some(pkg_dir) = nix_file.parent() else {
        return;
    };
    let patch_path = pkg_dir.join(patch_name);

    // Only act on local patch files that actually exist on disk
    if !patch_path.is_file() {
        return;
    }

    // Check whether any .nix file in the repo still references this patch name.
    // We restrict to *.nix so the patch file's own contents are never a false
    // positive. `git grep` searches the working tree, which already has the
    // updated .nix file (patch reference removed).
    let has_other_refs = match tokio::process::Command::new("git")
        .args(["grep", "-l", "--fixed-strings", patch_name, "--", "*.nix"])
        .output()
        .await
    {
        Ok(output) => output.status.success() && !output.stdout.is_empty(),
        Err(e) => {
            debug!("git grep failed for patch {}: {}", patch_name, e);
            // If we can't determine references, err on the safe side
            return;
        },
    };

    if has_other_refs {
        debug!(
            "Patch file {} is still referenced elsewhere, keeping it",
            patch_path.display()
        );
        return;
    }

    match tokio::fs::remove_file(&patch_path).await {
        Ok(()) => info!("Deleted unreferenced patch file: {}", patch_path.display()),
        Err(e) => warn!(
            "Could not delete patch file {}: {}",
            patch_path.display(),
            e
        ),
    }
}

/// Result of running passthru.tests
#[derive(Debug, Clone)]
pub enum TestResult {
    /// Tests were not requested (--run-passthru-tests not set)
    NotRequested,
    /// Package has no passthru.tests attribute
    NoTests,
    /// Tests ran and passed
    Passed,
    /// Tests ran and failed, with the error output
    Failed(String),
}

impl TestResult {
    pub fn passed(&self) -> bool {
        matches!(self, TestResult::Passed)
    }

    pub fn failed(&self) -> bool {
        matches!(self, TestResult::Failed(_))
    }
}

/// Run passthru.tests if they exist and return the result
pub async fn run_package_tests(
    eval_entry_point: &str,
    attr_path: &str,
    run_passthru_tests: bool,
) -> anyhow::Result<TestResult> {
    use crate::nix::{has_passthru_tests, normalize_entry_point};

    if !run_passthru_tests {
        return Ok(TestResult::NotRequested);
    }

    info!("Checking for passthru.tests...");
    let normalized_entry = normalize_entry_point(eval_entry_point);

    if !has_passthru_tests(&normalized_entry, attr_path).await? {
        info!("No passthru.tests found for {}", attr_path);
        return Ok(TestResult::NoTests);
    }

    info!("Found {}.passthru.tests, building tests...", attr_path);
    let (success, _stdout, stderr) =
        build_nix_expr(eval_entry_point, attr_path, Some("passthru.tests")).await?;

    if success {
        info!("✓ Tests passed");
        Ok(TestResult::Passed)
    } else {
        warn!("{}: passthru.tests failed", attr_path);
        Ok(TestResult::Failed(stderr))
    }
}

/// Update `npmDepsHash` for Node.js packages (see [`discover_dependency_hash`]).
pub async fn update_npm_deps_hash_if_needed(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_hash: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(old_hash) = old_hash else {
        return Ok(None);
    };
    info!("Detected Node.js package, updating npmDepsHash");
    discover_dependency_hash(
        eval_entry_point,
        attr_path,
        file_location,
        "npmDepsHash",
        old_hash,
    )
    .await
    .map(Some)
}

/// Update `nugetDeps` for .NET packages (see [`discover_dependency_hash`]).
pub async fn update_nuget_deps_hash_if_needed(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_hash: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(old_hash) = old_hash else {
        return Ok(None);
    };
    info!("Detected .NET package, updating nugetDeps");
    discover_dependency_hash(
        eval_entry_point,
        attr_path,
        file_location,
        "nugetDeps",
        old_hash,
    )
    .await
    .map(Some)
}

/// Update `composerDepsHash` for PHP packages (see [`discover_dependency_hash`]).
pub async fn update_composer_deps_hash_if_needed(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    old_hash: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(old_hash) = old_hash else {
        return Ok(None);
    };
    info!("Detected PHP package, updating composerDepsHash");
    discover_dependency_hash(
        eval_entry_point,
        attr_path,
        file_location,
        "composerDepsHash",
        old_hash,
    )
    .await
    .map(Some)
}

/// Discover a dependency hash bound by `attr` with the invalid hash pattern:
/// write the fake hash, build the package, take the `got:` hash from the
/// mismatch and write it.
///
/// If no hash can be discovered (the fetch fails for another reason, or the
/// build unexpectedly succeeds), `old_hash` is written back so the file is
/// never left with the fake hash.
async fn discover_dependency_hash(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    attr: &str,
    old_hash: &str,
) -> anyhow::Result<String> {
    update_hash_attr(file_location, attr, old_hash, FAKE_HASH).await?;
    info!("Set invalid {attr} in {}", file_location.display());

    // --keep-going: another stale FOD (one this update refreshes later) may
    // fail first; the mismatch wanted is the one for the fake hash.
    let build = build_nix_expr_with(eval_entry_point, attr_path, None, &["--keep-going"]).await;
    let discovered = match build {
        Ok((true, _, _)) => Err(anyhow::anyhow!(
            "Expected {attr} mismatch error but build succeeded"
        )),
        Ok((false, _, stderr)) => {
            hash_discovery::extract_hash_for(&stderr, FAKE_HASH).ok_or_else(|| {
                anyhow::anyhow!("Could not extract correct {attr} from build error:\n{stderr}")
            })
        },
        Err(e) => Err(e),
    };
    let hash = match discovered {
        Ok(hash) => hash,
        Err(e) => {
            if let Err(restore) = update_hash_attr(file_location, attr, FAKE_HASH, old_hash).await {
                warn!(
                    "Could not restore {attr} in {}: {restore:#}",
                    file_location.display()
                );
            }
            return Err(e);
        },
    };

    info!("Extracted correct {attr}: {hash}");
    update_hash_attr(file_location, attr, FAKE_HASH, &hash).await?;
    info!("Updated {attr} in {}", file_location.display());
    Ok(hash)
}

/// Discover and update source hashes for non-native platforms.
///
/// For packages with `passthru.ekapkgs-update.platform-hashes`, the normal update
/// flow only discovers the hash for the build machine's platform. This function
/// handles the remaining platforms by evaluating `src.url` with `--system` to get
/// the platform-specific download URL, then prefetching it to compute the hash.
pub async fn update_platform_hashes(
    eval_entry_point: &str,
    attr_path: &str,
    file_location: &Path,
    platforms: &[String],
) -> anyhow::Result<()> {
    use crate::nix::{eval_nix_expr_for_system, normalize_entry_point};

    let native_system = super::fod_platforms::native_system();

    let normalized_entry = normalize_entry_point(eval_entry_point);

    for platform in platforms {
        if platform == &native_system {
            debug!(
                "{}: Skipping platform {} (native, already updated)",
                attr_path, platform
            );
            continue;
        }

        info!(
            "{}: Discovering source hash for platform {}",
            attr_path, platform
        );

        // Step 1: Evaluate the current (stale) hash for this platform
        let hash_expr = format!(
            "let pkgs = import {} {{ }}; in pkgs.{}.src.outputHash",
            normalized_entry, attr_path
        );
        let stale_hash = match eval_nix_expr_for_system(&hash_expr, platform).await {
            Ok(h) => h,
            Err(e) => {
                warn!(
                    "{}: Could not evaluate src.outputHash for {}: {}",
                    attr_path, platform, e
                );
                continue;
            },
        };

        // Step 2: Evaluate the source URL for this platform
        let url_expr = format!(
            "let pkgs = import {} {{ }}; in pkgs.{}.src.url or (builtins.head pkgs.{}.src.urls)",
            normalized_entry, attr_path, attr_path
        );
        let src_url = match eval_nix_expr_for_system(&url_expr, platform).await {
            Ok(u) => u,
            Err(e) => {
                warn!(
                    "{}: Could not evaluate src.url for {}: {}",
                    attr_path, platform, e
                );
                continue;
            },
        };

        // Step 3: Prefetch the URL to get the correct hash
        let correct_hash = match prefetch_url_hash(&src_url).await {
            Ok(h) => h,
            Err(e) => {
                warn!(
                    "{}: Could not prefetch {} for {}: {}",
                    attr_path, src_url, platform, e
                );
                continue;
            },
        };

        // Step 4: Replace the stale hash in the file
        let content = tokio::fs::read_to_string(file_location).await?;
        if !content.contains(&stale_hash) {
            warn!(
                "{}: Stale hash for {} not found in {}, skipping",
                attr_path,
                platform,
                file_location.display()
            );
            continue;
        }
        let updated = content.replacen(&stale_hash, &correct_hash, 1);
        tokio::fs::write(file_location, &updated).await?;

        info!(
            "{}: Updated hash for {}: {}",
            attr_path, platform, correct_hash
        );
    }

    Ok(())
}

/// Prefetch a URL and return its SRI hash
///
/// Uses `nix store prefetch-file --json <url>` to download the file and
/// compute its hash without requiring a full build.
pub(super) async fn prefetch_url_hash(url: &str) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("nix")
        .args(["store", "prefetch-file", "--json", url])
        .output()
        .await
        .context("Failed to execute nix store prefetch-file")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("nix store prefetch-file failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).context("Failed to parse prefetch-file JSON output")?;

    json["hash"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("No hash field in prefetch-file output"))
}
