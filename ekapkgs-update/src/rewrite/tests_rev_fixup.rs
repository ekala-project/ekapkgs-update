use super::rev_fixup::*;

#[test]
fn test_infer_tag_prefix_v() {
    assert_eq!(infer_tag_prefix("v5.1.0", "5.1.0"), Some("v"));
}

#[test]
fn test_infer_tag_prefix_release() {
    assert_eq!(infer_tag_prefix("release-1.2.3", "1.2.3"), Some("release-"));
}

#[test]
fn test_infer_tag_prefix_bare() {
    assert_eq!(infer_tag_prefix("1.0.0", "1.0.0"), Some(""));
}

#[test]
fn test_infer_tag_prefix_no_match() {
    assert_eq!(infer_tag_prefix("some-unrelated-tag", "1.0.0"), None);
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
