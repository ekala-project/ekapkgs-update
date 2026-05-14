use crate::package::PackageMetadata;

/// Detect the OSV.dev ecosystem for a package based on its metadata
///
/// Maps nixpkgs packages to their corresponding OSV ecosystems by analyzing
/// package metadata (dependency hashes, source URLs, etc.).
///
/// Supported ecosystems:
/// - PyPI (Python packages)
/// - crates.io (Rust packages)
/// - npm (JavaScript/Node packages)
/// - Packagist (PHP/Composer packages)
/// - NuGet (.NET packages)
///
/// # Arguments
/// * `metadata` - The package metadata extracted from Nix evaluation
///
/// # Returns
/// Some(ecosystem_name) if the ecosystem can be determined, None otherwise
///
/// # Note
/// If None is returned, CVE checking will be skipped for this package as we
/// cannot reliably query OSV without knowing the ecosystem.
pub fn detect_ecosystem(metadata: &PackageMetadata) -> Option<String> {
    // Check for Rust/Cargo packages
    if metadata.cargo_hash.is_some() || metadata.vendor_hash.is_some() {
        return Some("crates.io".to_owned());
    }

    // Check for npm packages
    if metadata.npm_deps_hash.is_some() {
        return Some("npm".to_owned());
    }

    // Check for PHP/Composer packages
    if metadata.composer_deps_hash.is_some() {
        return Some("Packagist".to_owned());
    }

    // Check for NuGet/.NET packages
    if metadata.nuget_deps_hash.is_some() {
        return Some("NuGet".to_owned());
    }

    // Check for PyPI packages by examining source URL
    if let Some(ref src_url) = metadata.src_url {
        if src_url.contains("pypi.org") || src_url.contains("files.pythonhosted.org") {
            return Some("PyPI".to_owned());
        }

        // Check for other ecosystems based on URL patterns
        if src_url.contains("crates.io") {
            return Some("crates.io".to_owned());
        }

        if src_url.contains("registry.npmjs.org") {
            return Some("npm".to_owned());
        }

        if src_url.contains("packagist.org") {
            return Some("Packagist".to_owned());
        }
    }

    // Could not reliably determine ecosystem
    None
}

/// Get the package name to use for OSV queries
///
/// For most ecosystems, we can use the `pname` field directly. This function
/// provides a hook for ecosystem-specific name transformations if needed.
///
/// # Arguments
/// * `metadata` - The package metadata
/// * `ecosystem` - The detected ecosystem name
///
/// # Returns
/// The package name to use in OSV queries, or None if no valid name
pub fn get_package_name(metadata: &PackageMetadata, _ecosystem: &str) -> Option<String> {
    // For now, we just use pname directly
    // In the future, we might need ecosystem-specific transformations
    // For example, some npm packages might need scope handling (@org/package)
    metadata.pname.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_metadata() -> PackageMetadata {
        PackageMetadata {
            version: "1.0.0".to_owned(),
            src_url: None,
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: Some("test-package".to_owned()),
            description: None,
            homepage: None,
            changelog: None,
            skip: None,
            semver_strategy: None,
            include_prereleases: None,
            version_regex: None,
        }
    }

    #[test]
    fn test_detect_rust_package_cargo_hash() {
        let mut metadata = create_test_metadata();
        metadata.cargo_hash = Some("sha256-abc123".to_owned());

        assert_eq!(detect_ecosystem(&metadata), Some("crates.io".to_owned()));
    }

    #[test]
    fn test_detect_rust_package_vendor_hash() {
        let mut metadata = create_test_metadata();
        metadata.vendor_hash = Some("sha256-xyz789".to_owned());

        assert_eq!(detect_ecosystem(&metadata), Some("crates.io".to_owned()));
    }

    #[test]
    fn test_detect_npm_package() {
        let mut metadata = create_test_metadata();
        metadata.npm_deps_hash = Some("sha256-npm123".to_owned());

        assert_eq!(detect_ecosystem(&metadata), Some("npm".to_owned()));
    }

    #[test]
    fn test_detect_composer_package() {
        let mut metadata = create_test_metadata();
        metadata.composer_deps_hash = Some("sha256-comp123".to_owned());

        assert_eq!(detect_ecosystem(&metadata), Some("Packagist".to_owned()));
    }

    #[test]
    fn test_detect_nuget_package() {
        let mut metadata = create_test_metadata();
        metadata.nuget_deps_hash = Some("sha256-nuget123".to_owned());

        assert_eq!(detect_ecosystem(&metadata), Some("NuGet".to_owned()));
    }

    #[test]
    fn test_detect_pypi_from_url() {
        let mut metadata = create_test_metadata();
        metadata.src_url =
            Some("https://files.pythonhosted.org/packages/test/test-1.0.0.tar.gz".to_owned());

        assert_eq!(detect_ecosystem(&metadata), Some("PyPI".to_owned()));
    }

    #[test]
    fn test_detect_pypi_from_url_alternate() {
        let mut metadata = create_test_metadata();
        metadata.src_url = Some("https://pypi.org/project/test-package/".to_owned());

        assert_eq!(detect_ecosystem(&metadata), Some("PyPI".to_owned()));
    }

    #[test]
    fn test_detect_crates_io_from_url() {
        let mut metadata = create_test_metadata();
        metadata.src_url = Some("https://crates.io/api/v1/crates/test/1.0.0/download".to_owned());

        assert_eq!(detect_ecosystem(&metadata), Some("crates.io".to_owned()));
    }

    #[test]
    fn test_detect_unknown_ecosystem() {
        let metadata = create_test_metadata();
        assert_eq!(detect_ecosystem(&metadata), None);
    }

    #[test]
    fn test_get_package_name() {
        let mut metadata = create_test_metadata();
        metadata.pname = Some("my-package".to_owned());

        assert_eq!(
            get_package_name(&metadata, "PyPI"),
            Some("my-package".to_owned())
        );
    }

    #[test]
    fn test_get_package_name_none() {
        let mut metadata = create_test_metadata();
        metadata.pname = None;

        assert_eq!(get_package_name(&metadata, "PyPI"), None);
    }
}
