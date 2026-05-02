//! Flake package update workflow
//!
//! This module implements the update workflow for packages exposed by Nix flakes.
//! It mirrors the traditional update workflow but uses flake-specific commands.

use anyhow::Context;
use tracing::{debug, info, warn};

use super::{build_flake_package, handle_commit_or_pr};
use crate::nix::flake::{
    build_installable, detect_system, eval_flake_attr, get_flake_package_metadata,
    get_flake_package_position,
};
use crate::rewrite::find_and_update_attr;
use crate::vcs_sources::{SemverStrategy, UpstreamSource};

/// Update a flake package
///
/// This is the main entry point for updating packages exposed by flakes.
///
/// # Arguments
/// * `flake_ref` - The flake reference (typically ".")
/// * `attr_path` - The package attribute (e.g., "hello" or "packages.x86_64-linux.hello")
/// * `flake_output` - Optional output prefix (e.g., "packages.x86_64-linux")
/// * `strategy` - Version selection strategy
/// * `commit` - Whether to create a git commit
/// * `create_pr` - Whether to create a pull request
/// * `upstream` - Upstream git remote
/// * `fork` - Fork git remote
/// * `run_passthru_tests` - Whether to run passthru.tests
/// * `src_only` - Skip dependency hash updates
/// * `explicit_version` - Explicit version to update to (overrides strategy)
/// * `version_regex` - Custom regex for version extraction
/// * `format` - Whether to format the file with nixfmt
/// * `override_filename` - Override the filename to update (ignores meta.position)
///
/// # Returns
/// Ok(()) on success
pub async fn update_flake_package(
    flake_ref: String,
    attr_path: String,
    flake_output: Option<String>,
    strategy: SemverStrategy,
    commit: bool,
    create_pr: bool,
    upstream: Option<String>,
    fork: String,
    run_passthru_tests: bool,
    src_only: bool,
    explicit_version: Option<String>,
    version_regex: Option<String>,
    format: bool,
    override_filename: Option<String>,
) -> anyhow::Result<()> {
    info!("Updating flake package: {}", attr_path);

    // Auto-detect system if flake_output not specified
    let output_prefix = match flake_output {
        Some(prefix) => prefix,
        None => {
            let system = detect_system().await?;
            let detected = format!("packages.{}", system);
            info!("Auto-detected output prefix: {}", detected);
            detected
        },
    };

    // Build the full installable path
    let installable = build_installable(&flake_ref, Some(&output_prefix), &attr_path);
    info!("Full installable: {}", installable);

    // Step 1: Extract package metadata
    info!("Extracting package metadata...");
    let metadata = get_flake_package_metadata(&installable)
        .await
        .context("Failed to extract flake package metadata")?;

    info!("Current version: {}", metadata.version);
    if let Some(ref url) = metadata.src_url {
        debug!("Source URL: {}", url);
    }

    // Step 2: Determine upstream source
    let upstream_source = match &metadata.src_url {
        Some(url) => UpstreamSource::from_url(url).context("Failed to parse source URL")?,
        None => {
            anyhow::bail!("Package has no src.url attribute - cannot determine upstream source");
        },
    };

    info!("Upstream source: {:?}", upstream_source);

    // Step 3: Find best release according to strategy
    info!("Fetching compatible release with strategy: {:?}", strategy);
    let best_release = upstream_source
        .get_compatible_release(
            &metadata.version,
            strategy,
            None,
            explicit_version.as_deref(),
            version_regex.as_deref(),
        )
        .await
        .context("Failed to find compatible release")?;

    info!(
        "Best release: {} -> {}",
        metadata.version, best_release.tag_name
    );

    // If already up to date, we're done
    if best_release.tag_name == metadata.version {
        info!("Package is already at version {}", metadata.version);
        return Ok(());
    }

    // Step 4: Get file location
    let file_path = if let Some(ref override_file) = override_filename {
        info!("Using override filename: {}", override_file);
        override_file.clone()
    } else {
        info!("Finding package definition location...");
        get_flake_package_position(&installable)
            .await
            .context("Failed to get package file location")?
    };

    info!("Package defined in: {}", file_path);

    // Step 5: Update source hash
    info!(
        "Updating version from {} to {}...",
        metadata.version, best_release.tag_name
    );

    update_source_hash_flake(
        &installable,
        &file_path,
        &metadata.version,
        &best_release.tag_name,
    )
    .await
    .context("Failed to update source hash")?;

    // Step 6: Update dependency hashes (unless --src-only is set)
    if !src_only {
        // Update cargo hash if needed
        if metadata.cargo_hash.is_some() {
            info!("Updating cargoHash...");
            update_cargo_hash_flake(&installable, &file_path)
                .await
                .context("Failed to update cargoHash")?;
        }

        // Update vendor hash if needed
        if metadata.vendor_hash.is_some() {
            info!("Updating vendorHash...");
            update_vendor_hash_flake(&installable, &file_path)
                .await
                .context("Failed to update vendorHash")?;
        }

        // Update npm deps hash if needed
        if metadata.npm_deps_hash.is_some() {
            info!("Updating npmDepsHash...");
            update_npm_deps_hash_flake(&installable, &file_path)
                .await
                .context("Failed to update npmDepsHash")?;
        }

        // Update nuget deps hash if needed
        if metadata.nuget_deps_hash.is_some() {
            info!("Updating nugetDeps hash...");
            update_nuget_deps_hash_flake(&installable, &file_path)
                .await
                .context("Failed to update nugetDeps hash")?;
        }

        // Update composer deps hash if needed
        if metadata.composer_deps_hash.is_some() {
            info!("Updating composerDepsHash...");
            update_composer_deps_hash_flake(&installable, &file_path)
                .await
                .context("Failed to update composerDepsHash")?;
        }
    } else {
        info!("Skipping dependency hash updates (--src-only flag set)");
    }

    // Step 7: Build and verify
    info!("Building updated package...");
    let (success, _stdout, stderr) = build_flake_package(&installable, None).await?;

    if !success {
        warn!("Build failed after update");
        anyhow::bail!("Build failed: {}", stderr);
    }

    info!("Build succeeded!");

    // Step 9: Run tests if requested
    if run_passthru_tests {
        let test_result = run_package_tests_flake(&installable).await;
        match test_result {
            Ok(_) => info!("Tests passed"),
            Err(e) => {
                warn!("Tests failed: {}", e);
                // Continue anyway - tests are optional
            },
        }
    }

    // Step 10: Format the file if requested
    if format {
        use std::path::Path;

        use super::format_nix_file;
        format_nix_file(Path::new(&file_path)).await?;
    }

    // Step 11: Commit or create PR
    if commit || create_pr {
        handle_commit_or_pr(
            &attr_path,
            &metadata,
            &best_release.tag_name,
            commit,
            create_pr,
            upstream,
            &fork,
            run_passthru_tests,
        )
        .await?;
    }

    info!(
        "Successfully updated {} to {}",
        attr_path, best_release.tag_name
    );
    Ok(())
}

/// Helper to update a single attribute in a flake file
async fn update_flake_file_attr(
    file_path: &str,
    attr_name: &str,
    old_value: Option<&str>,
    new_value: &str,
) -> anyhow::Result<()> {
    let content = tokio::fs::read_to_string(file_path).await?;
    let updated = find_and_update_attr(&content, attr_name, new_value, old_value)?;
    tokio::fs::write(file_path, updated).await?;
    Ok(())
}

/// Update source hash for a flake package using invalid hash discovery
async fn update_source_hash_flake(
    installable: &str,
    file_path: &str,
    old_version: &str,
    new_version: &str,
) -> anyhow::Result<()> {
    use crate::hash_discovery::extract_hash;

    // Step 1: Update version
    update_flake_file_attr(file_path, "version", Some(old_version), new_version).await?;

    // Step 2: Set invalid hash to trigger discovery
    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    // Try to find current hash attribute name (hash, sha256, outputHash)
    let hash_attr_candidates = ["hash", "sha256", "outputHash"];
    let mut hash_attr = "outputHash"; // Default fallback

    for &attr in &hash_attr_candidates {
        let check_attr = format!("src.{}", attr);
        if eval_flake_attr(installable, &check_attr).await.is_ok() {
            hash_attr = attr;
            break;
        }
    }

    // Get current hash value to use as old value
    let current_hash = eval_flake_attr(installable, &format!("src.{}", hash_attr))
        .await
        .ok();

    update_flake_file_attr(file_path, hash_attr, current_hash.as_deref(), invalid_hash).await?;

    // Step 3: Build to get the correct hash
    debug!("Building with invalid hash to discover correct hash...");
    let (success, _stdout, stderr) = build_flake_package(installable, None).await?;

    if success {
        warn!("Build succeeded with invalid hash - this shouldn't happen");
        anyhow::bail!("Build unexpectedly succeeded with invalid hash");
    }

    // Step 4: Extract correct hash from error
    let correct_hash = extract_hash(&stderr).context("Failed to extract hash from build error")?;

    info!("Discovered correct hash: {}", correct_hash);

    // Step 5: Update with correct hash
    update_flake_file_attr(file_path, hash_attr, Some(invalid_hash), &correct_hash).await?;

    // Step 6: Verify build succeeds
    let (success, _stdout, stderr) = build_flake_package(installable, None).await?;

    if !success {
        anyhow::bail!("Build still fails after updating hash: {}", stderr);
    }

    Ok(())
}

/// Update cargo hash for a flake package
async fn update_cargo_hash_flake(installable: &str, file_path: &str) -> anyhow::Result<()> {
    use crate::hash_discovery::extract_hash;

    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    // Try to get current cargoHash
    let current_hash = eval_flake_attr(installable, "cargoDeps.outputHash")
        .await
        .ok();

    // Update to invalid hash
    update_flake_file_attr(
        file_path,
        "cargoHash",
        current_hash.as_deref(),
        invalid_hash,
    )
    .await?;

    // Build to discover correct hash
    let (success, _stdout, stderr) = build_flake_package(installable, None).await?;

    if success {
        warn!("Build succeeded with invalid cargoHash - skipping cargo hash update");
        return Ok(());
    }

    // Extract correct hash
    let correct_hash = extract_hash(&stderr).context("Failed to extract cargo hash from error")?;

    info!("Discovered correct cargoHash: {}", correct_hash);

    // Update with correct hash
    update_flake_file_attr(file_path, "cargoHash", Some(invalid_hash), &correct_hash).await?;

    Ok(())
}

/// Update vendor hash for a flake package
async fn update_vendor_hash_flake(installable: &str, file_path: &str) -> anyhow::Result<()> {
    use crate::hash_discovery::extract_hash;

    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    // Try vendorHash first, fall back to vendorSha256
    let hash_attr_candidates = ["vendorHash", "vendorSha256"];
    let mut hash_attr = "vendorHash"; // Default fallback
    let mut current_hash = None;

    for &attr in &hash_attr_candidates {
        if let Ok(hash) = eval_flake_attr(installable, attr).await {
            hash_attr = attr;
            current_hash = Some(hash);
            break;
        }
    }

    // Update to invalid hash
    update_flake_file_attr(file_path, hash_attr, current_hash.as_deref(), invalid_hash).await?;

    // Build to discover correct hash
    let (success, _stdout, stderr) = build_flake_package(installable, None).await?;

    if success {
        warn!(
            "Build succeeded with invalid {} - skipping vendor hash update",
            hash_attr
        );
        return Ok(());
    }

    // Extract correct hash
    let correct_hash = extract_hash(&stderr).context("Failed to extract vendor hash from error")?;

    info!("Discovered correct {}: {}", hash_attr, correct_hash);

    // Update with correct hash
    update_flake_file_attr(file_path, hash_attr, Some(invalid_hash), &correct_hash).await?;

    Ok(())
}

/// Update npm deps hash for a flake package
async fn update_npm_deps_hash_flake(installable: &str, file_path: &str) -> anyhow::Result<()> {
    use crate::hash_discovery::extract_hash;

    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    // Try to get current npmDepsHash
    let current_hash = eval_flake_attr(installable, "npmDepsHash").await.ok();

    // Update to invalid hash
    update_flake_file_attr(
        file_path,
        "npmDepsHash",
        current_hash.as_deref(),
        invalid_hash,
    )
    .await?;

    // Build to discover correct hash
    let (success, _stdout, stderr) = build_flake_package(installable, None).await?;

    if success {
        warn!("Build succeeded with invalid npmDepsHash - skipping npm deps hash update");
        return Ok(());
    }

    // Extract correct hash
    let correct_hash =
        extract_hash(&stderr).context("Failed to extract npm deps hash from error")?;

    info!("Discovered correct npmDepsHash: {}", correct_hash);

    // Update with correct hash
    update_flake_file_attr(file_path, "npmDepsHash", Some(invalid_hash), &correct_hash).await?;

    Ok(())
}

/// Update nuget deps hash for a flake package
async fn update_nuget_deps_hash_flake(installable: &str, file_path: &str) -> anyhow::Result<()> {
    use crate::hash_discovery::extract_hash;

    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    // Try to get current nugetDeps hash
    let current_hash = eval_flake_attr(installable, "nugetDeps").await.ok();

    // Update to invalid hash
    update_flake_file_attr(
        file_path,
        "nugetDeps",
        current_hash.as_deref(),
        invalid_hash,
    )
    .await?;

    // Build to discover correct hash
    let (success, _stdout, stderr) = build_flake_package(installable, None).await?;

    if success {
        warn!("Build succeeded with invalid nugetDeps hash - skipping nuget deps hash update");
        return Ok(());
    }

    // Extract correct hash
    let correct_hash =
        extract_hash(&stderr).context("Failed to extract nuget deps hash from error")?;

    info!("Discovered correct nugetDeps hash: {}", correct_hash);

    // Update with correct hash
    update_flake_file_attr(file_path, "nugetDeps", Some(invalid_hash), &correct_hash).await?;

    Ok(())
}

/// Update composer deps hash for a flake package
async fn update_composer_deps_hash_flake(installable: &str, file_path: &str) -> anyhow::Result<()> {
    use crate::hash_discovery::extract_hash;

    let invalid_hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    // Try to get current composerDepsHash
    let current_hash = eval_flake_attr(installable, "composerDepsHash").await.ok();

    // Update to invalid hash
    update_flake_file_attr(
        file_path,
        "composerDepsHash",
        current_hash.as_deref(),
        invalid_hash,
    )
    .await?;

    // Build to discover correct hash
    let (success, _stdout, stderr) = build_flake_package(installable, None).await?;

    if success {
        warn!("Build succeeded with invalid composerDepsHash - skipping composer deps hash update");
        return Ok(());
    }

    // Extract correct hash
    let correct_hash =
        extract_hash(&stderr).context("Failed to extract composer deps hash from error")?;

    info!("Discovered correct composerDepsHash: {}", correct_hash);

    // Update with correct hash
    update_flake_file_attr(
        file_path,
        "composerDepsHash",
        Some(invalid_hash),
        &correct_hash,
    )
    .await?;

    Ok(())
}

/// Run passthru.tests for a flake package
async fn run_package_tests_flake(installable: &str) -> anyhow::Result<()> {
    info!("Running passthru.tests...");

    // Check if tests exist
    let has_tests = eval_flake_attr(installable, "passthru.tests").await.is_ok();

    if !has_tests {
        info!("No passthru.tests defined");
        return Ok(());
    }

    // Build the tests
    let (success, _stdout, stderr) =
        build_flake_package(installable, Some("passthru.tests")).await?;

    if !success {
        anyhow::bail!("Tests failed: {}", stderr);
    }

    info!("All tests passed");
    Ok(())
}
