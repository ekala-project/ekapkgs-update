use super::*;

#[test]
fn test_from_url_github() {
    let url = "https://github.com/owner/repo";
    let source = UpstreamSource::from_url(url);
    assert!(matches!(
        source,
        Some(UpstreamSource::GitHub { ref owner, ref repo })
            if owner == "owner" && repo == "repo"
    ));
}

#[test]
fn test_from_url_gitlab() {
    let url = "https://gitlab.com/owner/project";
    let source = UpstreamSource::from_url(url);
    assert!(matches!(
        source,
        Some(UpstreamSource::GitLab { ref owner, ref project })
            if owner == "owner" && project == "project"
    ));
}

#[test]
fn test_from_url_invalid() {
    let url = "https://example.com/some/path";
    let source = UpstreamSource::from_url(url);
    assert!(source.is_none());
}

#[test]
fn test_from_url_pypi_project() {
    let url = "https://pypi.org/project/requests/";
    let source = UpstreamSource::from_url(url);
    assert!(matches!(
        source,
        Some(UpstreamSource::PyPI { ref pname }) if pname == "requests"
    ));
}

#[test]
fn test_from_url_pypi_python_org() {
    let url = "https://pypi.python.org/project/django/";
    let source = UpstreamSource::from_url(url);
    assert!(matches!(
        source,
        Some(UpstreamSource::PyPI { ref pname }) if pname == "django"
    ));
}

#[test]
fn test_from_url_pypi_files() {
    let url = "https://files.pythonhosted.org/packages/abc/def/requests-2.28.1.tar.gz";
    let source = UpstreamSource::from_url(url);
    assert!(matches!(
        source,
        Some(UpstreamSource::PyPI { ref pname }) if pname == "requests"
    ));
}

#[test]
fn test_from_url_pypi_mirror() {
    let url = "mirror://pypi/a/azure-mgmt-advisor/azure-mgmt-advisor-9.0.0.zip";
    let source = UpstreamSource::from_url(url);
    assert!(matches!(
        source,
        Some(UpstreamSource::PyPI { ref pname }) if pname == "azure-mgmt-advisor"
    ));
}

#[test]
fn test_from_url_pypi_mirror_single_letter() {
    let url = "mirror://pypi/r/requests/requests-2.28.1.tar.gz";
    let source = UpstreamSource::from_url(url);
    assert!(matches!(
        source,
        Some(UpstreamSource::PyPI { ref pname }) if pname == "requests"
    ));
}

#[test]
fn test_description_github() {
    let source = UpstreamSource::GitHub {
        owner: "owner".to_owned(),
        repo: "repo".to_owned(),
    };
    assert_eq!(source.description(), "GitHub repo: owner/repo");
}

#[test]
fn test_description_gitlab() {
    let source = UpstreamSource::GitLab {
        owner: "owner".to_owned(),
        project: "project".to_owned(),
    };
    assert_eq!(source.description(), "GitLab project: owner/project");
}

#[test]
fn test_get_version() {
    let release = Release {
        tag_name: "v1.2.3".to_owned(),
        is_prerelease: false,
    };
    assert_eq!(UpstreamSource::get_version(&release), "1.2.3");
}

// SemverStrategy tests
#[test]
fn test_semver_strategy_from_str() {
    assert_eq!(
        SemverStrategy::from_str("latest").unwrap(),
        SemverStrategy::Latest
    );
    assert_eq!(
        SemverStrategy::from_str("major").unwrap(),
        SemverStrategy::Major
    );
    assert_eq!(
        SemverStrategy::from_str("minor").unwrap(),
        SemverStrategy::Minor
    );
    assert_eq!(
        SemverStrategy::from_str("patch").unwrap(),
        SemverStrategy::Patch
    );

    // Test case insensitivity
    assert_eq!(
        SemverStrategy::from_str("LATEST").unwrap(),
        SemverStrategy::Latest
    );
    assert_eq!(
        SemverStrategy::from_str("MaJoR").unwrap(),
        SemverStrategy::Major
    );

    // Test invalid
    assert!(SemverStrategy::from_str("invalid").is_err());
}

// is_version_acceptable tests - Latest strategy
#[test]
fn test_version_acceptable_latest() {
    // Latest accepts any newer version
    assert!(is_version_acceptable(
        "1.0.0",
        "2.0.0",
        SemverStrategy::Latest
    ));
    assert!(is_version_acceptable(
        "1.0.0",
        "1.1.0",
        SemverStrategy::Latest
    ));
    assert!(is_version_acceptable(
        "1.0.0",
        "1.0.1",
        SemverStrategy::Latest
    ));

    // Doesn't accept same or older
    assert!(!is_version_acceptable(
        "1.0.0",
        "1.0.0",
        SemverStrategy::Latest
    ));
    assert!(!is_version_acceptable(
        "2.0.0",
        "1.0.0",
        SemverStrategy::Latest
    ));
}

// is_version_acceptable tests - Major strategy
#[test]
fn test_version_acceptable_major() {
    // Major accepts any newer version (same as latest)
    assert!(is_version_acceptable(
        "1.0.0",
        "2.0.0",
        SemverStrategy::Major
    ));
    assert!(is_version_acceptable(
        "1.0.0",
        "1.1.0",
        SemverStrategy::Major
    ));
    assert!(is_version_acceptable(
        "1.0.0",
        "1.0.1",
        SemverStrategy::Major
    ));

    // Doesn't accept same or older
    assert!(!is_version_acceptable(
        "1.0.0",
        "1.0.0",
        SemverStrategy::Major
    ));
    assert!(!is_version_acceptable(
        "2.0.0",
        "1.0.0",
        SemverStrategy::Major
    ));
}

// is_version_acceptable tests - Minor strategy
#[test]
fn test_version_acceptable_minor() {
    // Minor accepts only same major version
    assert!(is_version_acceptable(
        "1.0.0",
        "1.1.0",
        SemverStrategy::Minor
    ));
    assert!(is_version_acceptable(
        "1.0.0",
        "1.0.1",
        SemverStrategy::Minor
    ));
    assert!(is_version_acceptable(
        "1.5.2",
        "1.9.0",
        SemverStrategy::Minor
    ));

    // Doesn't accept different major version
    assert!(!is_version_acceptable(
        "1.0.0",
        "2.0.0",
        SemverStrategy::Minor
    ));
    assert!(!is_version_acceptable(
        "1.0.0",
        "2.1.0",
        SemverStrategy::Minor
    ));

    // Doesn't accept same or older
    assert!(!is_version_acceptable(
        "1.5.0",
        "1.5.0",
        SemverStrategy::Minor
    ));
    assert!(!is_version_acceptable(
        "1.5.0",
        "1.4.0",
        SemverStrategy::Minor
    ));
}

// is_version_acceptable tests - Patch strategy
#[test]
fn test_version_acceptable_patch() {
    // Patch accepts only same major.minor version
    assert!(is_version_acceptable(
        "1.0.0",
        "1.0.1",
        SemverStrategy::Patch
    ));
    assert!(is_version_acceptable(
        "1.0.0",
        "1.0.9",
        SemverStrategy::Patch
    ));
    assert!(is_version_acceptable(
        "2.3.4",
        "2.3.5",
        SemverStrategy::Patch
    ));

    // Doesn't accept different minor version
    assert!(!is_version_acceptable(
        "1.0.0",
        "1.1.0",
        SemverStrategy::Patch
    ));
    assert!(!is_version_acceptable(
        "1.0.0",
        "1.1.1",
        SemverStrategy::Patch
    ));

    // Doesn't accept different major version
    assert!(!is_version_acceptable(
        "1.0.0",
        "2.0.0",
        SemverStrategy::Patch
    ));
    assert!(!is_version_acceptable(
        "1.0.0",
        "2.0.1",
        SemverStrategy::Patch
    ));

    // Doesn't accept same or older
    assert!(!is_version_acceptable(
        "1.0.5",
        "1.0.5",
        SemverStrategy::Patch
    ));
    assert!(!is_version_acceptable(
        "1.0.5",
        "1.0.4",
        SemverStrategy::Patch
    ));
}

// Test with version prefixes
#[test]
fn test_version_acceptable_with_prefixes() {
    // v prefix
    assert!(is_version_acceptable(
        "v1.0.0",
        "v2.0.0",
        SemverStrategy::Latest
    ));
    assert!(is_version_acceptable(
        "v1.0.0",
        "2.0.0",
        SemverStrategy::Latest
    ));
    assert!(is_version_acceptable(
        "1.0.0",
        "v2.0.0",
        SemverStrategy::Latest
    ));

    // Minor with v prefix
    assert!(is_version_acceptable(
        "v1.0.0",
        "v1.1.0",
        SemverStrategy::Minor
    ));
    assert!(!is_version_acceptable(
        "v1.0.0",
        "v2.0.0",
        SemverStrategy::Minor
    ));
}

// Test non-semver versions
#[test]
fn test_version_acceptable_non_semver() {
    // Latest/Major strategies should fall back to string comparison
    assert!(is_version_acceptable(
        "2024.01.01",
        "2024.12.01",
        SemverStrategy::Latest
    ));
    assert!(is_version_acceptable(
        "2024.01.01",
        "2024.12.01",
        SemverStrategy::Major
    ));

    // Minor/Patch strategies should reject non-semver
    assert!(!is_version_acceptable(
        "2024.01.01",
        "2024.12.01",
        SemverStrategy::Minor
    ));
    assert!(!is_version_acceptable(
        "2024.01.01",
        "2024.12.01",
        SemverStrategy::Patch
    ));
}

// Test edge case: version 0.x.y
#[test]
fn test_version_acceptable_zero_versions() {
    // 0.x versions
    assert!(is_version_acceptable(
        "0.1.0",
        "0.2.0",
        SemverStrategy::Latest
    ));
    assert!(is_version_acceptable(
        "0.1.0",
        "0.2.0",
        SemverStrategy::Major
    ));
    assert!(is_version_acceptable(
        "0.1.0",
        "0.2.0",
        SemverStrategy::Minor
    ));
    assert!(!is_version_acceptable(
        "0.1.0",
        "0.2.0",
        SemverStrategy::Patch
    ));

    assert!(is_version_acceptable(
        "0.1.0",
        "0.1.1",
        SemverStrategy::Patch
    ));
}

// Test normalize_version function
#[test]
fn test_normalize_version() {
    // Two-component versions
    assert_eq!(normalize_version("1.25"), "1.25.0");
    assert_eq!(normalize_version("1.9"), "1.9.0");
    assert_eq!(normalize_version("0.5"), "0.5.0");

    // Single-component versions
    assert_eq!(normalize_version("2"), "2.0.0");
    assert_eq!(normalize_version("10"), "10.0.0");

    // Already normalized (three components)
    assert_eq!(normalize_version("1.2.3"), "1.2.3");
    assert_eq!(normalize_version("0.0.1"), "0.0.1");

    // With pre-release suffixes
    assert_eq!(normalize_version("1.0-beta"), "1.0.0-beta");
    assert_eq!(normalize_version("2.5-rc1"), "2.5.0-rc1");
    assert_eq!(normalize_version("1.2.3-alpha"), "1.2.3-alpha");

    // With unstable suffix (gets normalized before truncation)
    assert_eq!(normalize_version("1.25-unstable"), "1.25.0-unstable");
}

// Test two-component version comparison (the libdeflate 1.25 vs 1.9 issue)
#[test]
fn test_version_acceptable_two_components() {
    // This is the key test case: 1.25 should be considered newer than 1.9
    assert!(!is_version_acceptable(
        "1.25",
        "1.9",
        SemverStrategy::Latest
    ));
    assert!(is_version_acceptable("1.9", "1.25", SemverStrategy::Latest));

    // More two-component version tests
    assert!(is_version_acceptable("1.0", "1.1", SemverStrategy::Latest));
    assert!(is_version_acceptable("1.9", "1.10", SemverStrategy::Latest));
    assert!(is_version_acceptable("2.0", "2.1", SemverStrategy::Latest));

    // Two-component with Minor strategy
    assert!(is_version_acceptable("1.9", "1.25", SemverStrategy::Minor));
    assert!(!is_version_acceptable("1.9", "2.0", SemverStrategy::Minor));

    // Two-component with Patch strategy (should upgrade minor version)
    assert!(!is_version_acceptable("1.9", "1.25", SemverStrategy::Patch));
    assert!(is_version_acceptable("1.9", "1.9.1", SemverStrategy::Patch));
}

// Test mixed component version comparison
#[test]
fn test_version_acceptable_mixed_components() {
    // Two-component current, three-component new
    assert!(is_version_acceptable(
        "1.9",
        "1.25.0",
        SemverStrategy::Latest
    ));
    assert!(is_version_acceptable(
        "1.9",
        "1.9.1",
        SemverStrategy::Latest
    ));

    // Three-component current, two-component new
    assert!(is_version_acceptable(
        "1.9.0",
        "1.25",
        SemverStrategy::Latest
    ));
    assert!(!is_version_acceptable(
        "1.25.0",
        "1.9",
        SemverStrategy::Latest
    ));
}

// Test that single-component versions are rejected
#[test]
fn test_version_acceptable_single_component_rejected() {
    // Single-component versions should be rejected (no major.minor)
    // Date-like version (the ninja 120715 case)
    assert!(!is_version_acceptable(
        "1.13.1",
        "120715",
        SemverStrategy::Latest
    ));
    assert!(!is_version_acceptable(
        "1.13.1",
        "120715",
        SemverStrategy::Minor
    ));

    // Regular single-component version
    assert!(!is_version_acceptable("1.0.0", "2", SemverStrategy::Latest));
    assert!(!is_version_acceptable("1", "2", SemverStrategy::Latest));

    // With v prefix
    assert!(!is_version_acceptable(
        "v1.0.0",
        "v2",
        SemverStrategy::Latest
    ));

    // Two-component versions should still work
    assert!(is_version_acceptable(
        "1.13",
        "1.14",
        SemverStrategy::Latest
    ));
    assert!(is_version_acceptable(
        "1.13.1",
        "1.13.2",
        SemverStrategy::Latest
    ));
}
