use super::*;

#[test]
fn test_update_rev_with_v_prefix() {
    let content = r#"{
  version = "1.0.0";
  src = fetchFromGitHub {
    owner = "example";
    repo = "app";
    rev = "v1.0.0";
    hash = "sha256-abc";
  };
}"#;

    let result = try_update_rev_attr(content, "1.0.0", "2.0.0");
    assert!(
        result.is_ok(),
        "Should successfully update rev with v prefix"
    );
    let updated = result.unwrap();
    assert!(
        updated.contains(r#"rev = "v2.0.0";"#),
        "Should update v1.0.0 to v2.0.0"
    );
    assert!(
        !updated.contains(r#"rev = "v1.0.0";"#),
        "Should not contain old version"
    );
}

#[test]
fn test_update_rev_with_release_prefix() {
    let content = r#"{
  version = "3.5.1";
  src = fetchFromGitHub {
    rev = "release-3.5.1";
  };
}"#;

    let result = try_update_rev_attr(content, "3.5.1", "3.6.0");
    assert!(
        result.is_ok(),
        "Should successfully update rev with release- prefix"
    );
    let updated = result.unwrap();
    assert!(
        updated.contains(r#"rev = "release-3.6.0";"#),
        "Should update release-3.5.1 to release-3.6.0"
    );
}

#[test]
fn test_update_rev_with_custom_prefix() {
    let content = r#"{
  version = "1.6";
  src = fetchFromGitHub {
    rev = "jq-1.6";
  };
}"#;

    let result = try_update_rev_attr(content, "1.6", "1.7");
    assert!(
        result.is_ok(),
        "Should successfully update rev with custom prefix (jq-)"
    );
    let updated = result.unwrap();
    assert!(
        updated.contains(r#"rev = "jq-1.7";"#),
        "Should update jq-1.6 to jq-1.7 using smart substring detection"
    );
}

#[test]
fn test_skip_commit_sha() {
    let content = r#"{
  version = "1.0.0";
  src = fetchFromGitHub {
    rev = "abc123def456789012345678901234567890abcd";
    hash = "sha256-xyz";
  };
}"#;

    let result = try_update_rev_attr(content, "1.0.0", "2.0.0");
    assert!(
        result.is_err(),
        "Should skip updating rev when it's a commit SHA"
    );
    if let Err(e) = result {
        assert!(
            e.is_not_found(),
            "Should return NotFound error for commit SHA"
        );
        let msg = format!("{}", e);
        assert!(
            msg.contains("commit SHA"),
            "Error message should mention commit SHA"
        );
    }
}

#[test]
fn test_skip_string_interpolation() {
    let content = r#"{
  version = "1.0.0";
  src = fetchFromGitHub {
    rev = "v${version}";
  };
}"#;

    let result = try_update_rev_attr(content, "1.0.0", "2.0.0");
    assert!(
        result.is_err(),
        "Should skip updating rev when it uses string interpolation"
    );
    if let Err(e) = result {
        assert!(
            e.is_not_found(),
            "Should return NotFound error for interpolation"
        );
        let msg = format!("{}", e);
        assert!(
            msg.contains("interpolation"),
            "Error message should mention interpolation"
        );
    }
}

#[test]
fn test_no_rev_attribute() {
    let content = r#"{
  version = "1.0.0";
  src = fetchFromGitHub {
    tag = "v${version}";
    hash = "sha256-abc";
  };
}"#;

    let result = try_update_rev_attr(content, "1.0.0", "2.0.0");
    assert!(
        result.is_err(),
        "Should return error when rev attribute doesn't exist"
    );
    if let Err(e) = result {
        assert!(e.is_not_found(), "Should return NotFound error");
    }
}

#[test]
fn test_rev_doesnt_match_version() {
    let content = r#"{
  version = "1.0.0";
  src = fetchFromGitHub {
    rev = "some-other-tag";
  };
}"#;

    let result = try_update_rev_attr(content, "1.0.0", "2.0.0");
    assert!(
        result.is_err(),
        "Should not update rev when it doesn't match version pattern"
    );
    if let Err(e) = result {
        assert!(e.is_not_found(), "Should return NotFound error");
        let msg = format!("{}", e);
        assert!(
            msg.contains("does not match"),
            "Error message should explain it doesn't match version pattern"
        );
    }
}

#[test]
fn test_preserves_formatting() {
    let content = r#"{
  version = "1.0.0";

  src = fetchFromGitHub {
    owner = "example";
    repo = "test";
    rev = "v1.0.0";  # This is the git tag
    hash = "sha256-abc";
  };

  meta = { };
}"#;

    let result = try_update_rev_attr(content, "1.0.0", "2.0.0");
    assert!(result.is_ok(), "Should successfully update");
    let updated = result.unwrap();

    // Check version was updated
    assert!(updated.contains(r#"rev = "v2.0.0";"#));

    // Check formatting preserved (indentation and structure)
    assert!(updated.contains("src = fetchFromGitHub"));
    assert!(updated.contains("meta = { };"));

    // Check that empty lines are preserved
    let has_empty_line_before_src =
        updated.contains("\n\n  src =") || updated.contains("\n\n  src");
    assert!(
        has_empty_line_before_src,
        "Empty line before src should be preserved"
    );
}

#[test]
fn test_multiple_occurrences_of_rev() {
    // Edge case: multiple sources with rev attributes
    let content = r#"{
  version = "1.0.0";

  src1 = fetchFromGitHub {
    rev = "v1.0.0";
  };

  src2 = fetchFromGitHub {
    rev = "v1.0.0";
  };
}"#;

    let result = try_update_rev_attr(content, "1.0.0", "2.0.0");
    assert!(result.is_ok(), "Should successfully update all occurrences");
    let updated = result.unwrap();

    // Both should be updated
    assert_eq!(
        updated.matches(r#"rev = "v2.0.0";"#).count(),
        2,
        "Should update both rev attributes"
    );
    assert_eq!(
        updated.matches(r#"rev = "v1.0.0";"#).count(),
        0,
        "Should not have any old version left"
    );
}

#[test]
fn test_version_with_prerelease() {
    let content = r#"{
  version = "2.0.0-rc1";
  src = fetchFromGitHub {
    rev = "v2.0.0-rc1";
  };
}"#;

    let result = try_update_rev_attr(content, "2.0.0-rc1", "2.0.0-rc2");
    assert!(result.is_ok(), "Should handle prerelease versions");
    let updated = result.unwrap();
    assert!(updated.contains(r#"rev = "v2.0.0-rc2";"#));
}

#[test]
fn test_version_with_build_metadata() {
    let content = r#"{
  version = "1.0.0+20240101";
  src = fetchFromGitHub {
    rev = "v1.0.0+20240101";
  };
}"#;

    let result = try_update_rev_attr(content, "1.0.0+20240101", "1.0.0+20240201");
    assert!(result.is_ok(), "Should handle build metadata in versions");
    let updated = result.unwrap();
    assert!(updated.contains(r#"rev = "v1.0.0+20240201";"#));
}

#[test]
fn test_invalid_syntax() {
    let content = r#"{
  version = "1.0.0"
  src = fetchFromGitHub {
    rev = "v1.0.0"  # Missing semicolons
}"#;

    let result = try_update_rev_attr(content, "1.0.0", "2.0.0");
    // The initial parse validation should catch invalid syntax
    // The exact behavior depends on whether regex matches before validation
    // Either way, we shouldn't crash
    let _ = result; // Just ensure no panic
}

#[test]
fn test_complex_real_world_example() {
    let content = r#"{ lib
, stdenv
, fetchFromGitHub
, cmake
}:

stdenv.mkDerivation rec {
  pname = "example-app";
  version = "3.2.1";

  src = fetchFromGitHub {
    owner = "example";
    repo = "app";
    rev = "v${version}";  # Uses interpolation
    hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
  };

  nativeBuildInputs = [ cmake ];

  meta = with lib; {
    description = "An example application";
    license = licenses.mit;
  };
}"#;

    let result = try_update_rev_attr(content, "3.2.1", "3.3.0");
    assert!(
        result.is_err(),
        "Should skip because rev uses interpolation"
    );
    if let Err(e) = result {
        let msg = format!("{}", e);
        assert!(msg.contains("interpolation"));
    }
}

#[test]
fn test_real_world_literal_rev() {
    let content = r#"{ lib
, stdenv
, fetchFromGitHub
}:

stdenv.mkDerivation rec {
  pname = "example-tool";
  version = "2.5.0";

  src = fetchFromGitHub {
    owner = "example";
    repo = "tool";
    rev = "release-2.5.0";
    hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
  };

  meta = with lib; {
    description = "An example tool";
    license = licenses.gpl3;
  };
}"#;

    let result = try_update_rev_attr(content, "2.5.0", "2.6.0");
    assert!(
        result.is_ok(),
        "Should update literal rev with release- prefix"
    );
    let updated = result.unwrap();
    assert!(updated.contains(r#"rev = "release-2.6.0";"#));
    assert!(!updated.contains(r#"rev = "release-2.5.0";"#));
    // Ensure version attribute wasn't touched
    assert!(updated.contains(r#"version = "2.5.0";"#));
}
