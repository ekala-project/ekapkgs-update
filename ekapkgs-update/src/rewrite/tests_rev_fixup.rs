use super::rev_fixup::*;

#[test]
fn test_split_tag() {
    let cases = [
        ("v5.1.0", "5.1.0", Some(("v", ""))),
        ("release-1.2.3", "1.2.3", Some(("release-", ""))),
        ("1.0.0", "1.0.0", Some(("", ""))),
        ("hdf5_1.14.6", "1.14.6", Some(("hdf5_", ""))),
        ("v2.0-stable", "2.0", Some(("v", "-stable"))),
        ("some-unrelated-tag", "1.0.0", None),
    ];
    for (tag, version, expected) in cases {
        assert_eq!(split_tag(tag, version), expected, "{tag} / {version}");
    }
}

#[test]
fn test_needs_final_attrs_conversion_plain() {
    let content = r#"
python3.pkgs.buildPythonPackage {
  pname = "breathe";
  version = "5.1.0";
}
"#;
    assert!(needs_final_attrs_conversion(content));
}

#[test]
fn test_needs_final_attrs_conversion_rec() {
    let content = r#"
buildGoModule rec {
  pname = "foo";
  version = "1.0.0";
}
"#;
    assert!(!needs_final_attrs_conversion(content));
}

#[test]
fn test_needs_final_attrs_conversion_final_attrs() {
    let content = r#"
stdenv.mkDerivation (finalAttrs: {
  pname = "foo";
  version = "1.0.0";
})
"#;
    assert!(!needs_final_attrs_conversion(content));
}

#[test]
fn test_fixup_commit_sha_with_rec() {
    let content = r#"{
  lib,
  buildGoModule,
  fetchFromGitHub,
}:

buildGoModule rec {
  pname = "foo";
  version = "2.0.0";

  src = fetchFromGitHub {
    owner = "test";
    repo = "foo";
    rev = "abc123def456789012345678901234567890abcd";
    hash = "sha256-AAAA";
  };

  meta = {
    description = "Test";
  };
}"#;
    let result = try_fixup_stale_rev(content, "v2.0.0", "2.0.0").unwrap();
    assert!(result.contains(r#"rev = "v${version}""#));
    assert!(!result.contains("abc123def456789012345678901234567890abcd"));
}

#[test]
fn test_fixup_commit_sha_with_final_attrs() {
    let content = r#"{
  lib,
  python3,
  fetchFromGitHub,
}:

python3.pkgs.buildPythonPackage (finalAttrs: {
  pname = "breathe";
  version = "5.1.0";

  src = fetchFromGitHub {
    owner = "breathe-doc";
    repo = "breathe";
    rev = "9711e826e0c46a635715e5814a83cab9dda79b7b";
    hash = "sha256-AAAA";
  };

  meta = {
    description = "Test";
  };
})"#;
    let result = try_fixup_stale_rev(content, "v5.1.0", "5.1.0").unwrap();
    assert!(result.contains(r#"rev = "v${finalAttrs.version}""#));
    assert!(!result.contains("9711e826e0c46a635715e5814a83cab9dda79b7b"));
}

#[test]
fn test_fixup_commit_sha_bare_version() {
    let content = r#"{
  lib,
  stdenv,
  fetchFromGitHub,
}:

stdenv.mkDerivation (finalAttrs: {
  pname = "test";
  version = "1.0.0";

  src = fetchFromGitHub {
    owner = "test";
    repo = "test";
    rev = "abc123def456789012345678901234567890abcd";
    hash = "sha256-AAAA";
  };

  meta = {
    description = "Test";
  };
})"#;
    // Tag is same as version (no prefix)
    let result = try_fixup_stale_rev(content, "1.0.0", "1.0.0").unwrap();
    assert!(result.contains(r#"rev = "${finalAttrs.version}""#));
}

#[test]
fn test_fixup_skips_unstable() {
    let content = r#"{
  version = "1.0.0-unstable-2025-01-01";
  rev = "abc123def456789012345678901234567890abcd";
}"#;
    let result = try_fixup_stale_rev(
        content,
        "v1.0.0-unstable-2025-01-01",
        "1.0.0-unstable-2025-01-01",
    );
    assert!(result.is_err());
}

#[test]
fn test_fixup_skips_non_sha_rev() {
    let content = r#"{
  version = "1.0.0";
  rev = "v1.0.0";
}"#;
    let result = try_fixup_stale_rev(content, "v1.0.0", "1.0.0");
    assert!(result.is_err());
}

#[test]
fn test_fixup_skips_when_tag_doesnt_contain_version() {
    let content = r#"{
  version = "1.0.0";
  rev = "abc123def456789012345678901234567890abcd";
}"#;
    let result = try_fixup_stale_rev(content, "some-unrelated-tag", "1.0.0");
    assert!(result.is_err());
}

#[test]
fn test_fixup_preserves_other_content() {
    let content = r#"{
  lib,
  stdenv,
  fetchFromGitHub,
}:

stdenv.mkDerivation rec {
  pname = "test";
  version = "2.0.0";

  src = fetchFromGitHub {
    owner = "example";
    repo = "test";
    rev = "abc123def456789012345678901234567890abcd";
    hash = "sha256-AAAA";
  };

  buildInputs = [ ];

  meta = {
    description = "A test package";
    license = lib.licenses.mit;
  };
}"#;
    let result = try_fixup_stale_rev(content, "v2.0.0", "2.0.0").unwrap();
    assert!(result.contains(r#"pname = "test""#));
    assert!(result.contains(r#"owner = "example""#));
    assert!(result.contains(r#"description = "A test package""#));
    assert!(result.contains("buildInputs = [ ]"));
    assert!(result.contains(r#"rev = "v${version}""#));
}

/// A fetchFromGitHub package with `ref_line` as its only git ref.
fn github_package(ref_line: &str) -> String {
    format!(
        "stdenv.mkDerivation (finalAttrs: {{\n  pname = \"hdf5\";\n  version = \"2.2.0\";\n\n  \
         src = fetchFromGitHub {{\n    owner = \"HDFGroup\";\n    repo = \"hdf5\";\n    \
         {ref_line}\n    hash = \"sha256-x\";\n  }};\n}})\n"
    )
}

#[test]
fn test_align_ref_with_tag() {
    let cases = [
        // HDF5 dropped the hdf5_ prefix in 2.x
        (
            "rev = \"hdf5_${version}\";",
            "2.2.0",
            "rev = \"${version}\";",
        ),
        (
            "tag = \"v${finalAttrs.version}\";",
            "release-2.2.0",
            "tag = \"release-${finalAttrs.version}\";",
        ),
        // suffix
        (
            "tag = \"v${finalAttrs.version}\";",
            "v2.2.0-final",
            "tag = \"v${finalAttrs.version}-final\";",
        ),
        // bare version reference gains a prefix
        ("tag = version;", "v2.2.0", "tag = \"v${version}\";"),
        (
            "tag = finalAttrs.version;",
            "v2.2.0",
            "tag = \"v${finalAttrs.version}\";",
        ),
        // literal (already bumped) rev
        ("rev = \"v2.2.0\";", "2.2.0", "rev = \"2.2.0\";"),
        // refs/tags/ is kept
        (
            "rev = \"refs/tags/hdf5_${version}\";",
            "2.2.0",
            "rev = \"refs/tags/${version}\";",
        ),
        // already right: unchanged
        ("tag = \"v${version}\";", "v2.2.0", "tag = \"v${version}\";"),
        ("tag = version;", "2.2.0", "tag = version;"),
        (
            "rev = \"refs/tags/v${version}\";",
            "v2.2.0",
            "rev = \"refs/tags/v${version}\";",
        ),
        ("rev = \"v2.2.0\";", "v2.2.0", "rev = \"v2.2.0\";"),
    ];
    for (ref_line, tag, expected) in cases {
        let content = github_package(ref_line);
        assert!(is_single_ref_from(&content, "hdf5"), "{ref_line}");
        let aligned = align_ref_with_tag(&content, tag, "2.2.0")
            .unwrap_or_else(|e| panic!("{ref_line} / {tag}: {e}"));
        assert_eq!(aligned, github_package(expected), "{ref_line} / {tag}");
    }
}

#[test]
fn test_align_ref_with_tag_refuses() {
    let cases = [
        // commit SHA
        (
            "rev = \"c934a2a15a6df020446ac3dfa07e3acf72b63a8f\";",
            "2.2.0",
        ),
        // other interpolation
        ("rev = \"${finalAttrs.pname}-${version}\";", "2.2.0"),
        ("rev = \"${tagPrefix}${version}\";", "2.2.0"),
        // unrelated identifier
        ("rev = srcRev;", "2.2.0"),
        // version not in the tag
        ("rev = \"hdf5_${version}\";", "hdf5-2_2_0"),
        // tag would need escaping
        ("rev = \"hdf5_${version}\";", "a$b-2.2.0"),
    ];
    for (ref_line, tag) in cases {
        let content = github_package(ref_line);
        let err = align_ref_with_tag(&content, tag, "2.2.0").expect_err(ref_line);
        assert!(err.is_not_found(), "{ref_line} / {tag}: {err}");
    }
}

/// opencv4: the main source uses `tag = version`; the first `rev` pins a
/// third-party data repository.
const OPENCV_LIKE: &str = r#"{ stdenv, fetchFromGitHub }:
let
  version = "5.1.0";
  src = fetchFromGitHub {
    owner = "opencv";
    repo = "opencv";
    tag = version;
    hash = "sha256-a";
  };
  ippicv = fetchFromGitHub {
    owner = "opencv";
    repo = "opencv_3rdparty";
    rev = "c934a2a15a6df020446ac3dfa07e3acf72b63a8f";
    hash = "sha256-b";
  };
in
stdenv.mkDerivation {
  pname = "opencv";
  inherit version src;
}
"#;

#[test]
fn test_several_refs_are_left_alone() {
    assert!(!is_single_ref_from(OPENCV_LIKE, "opencv"));
    assert!(align_ref_with_tag(OPENCV_LIKE, "v5.1.0", "5.1.0").is_err());
    let err = try_fixup_stale_rev(OPENCV_LIKE, "5.1.0", "5.1.0").unwrap_err();
    assert!(err.is_not_found());
}

#[test]
fn test_is_single_ref_from_checks_repo() {
    let content = github_package("rev = \"v${version}\";");
    assert!(is_single_ref_from(&content, "hdf5"));
    assert!(is_single_ref_from(&content, "HDF5"));
    // upstream is another repository (e.g. a mirror named by github-repo)
    assert!(!is_single_ref_from(&content, "hdf5-mirror"));
}
