//! Tests for attribute manipulation (find_and_update_attr)

use super::*;

#[test]
fn test_find_and_update_attr_simple() {
    let content = r#"{
  version = "1.0.0";
  hash = "sha256-old";
}"#;

    let result = find_and_update_attr(content, "version", "2.0.0", Some("1.0.0"));
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(updated.contains(r#"version = "2.0.0";"#));
    assert!(!updated.contains(r#"version = "1.0.0";"#));
}

#[test]
fn test_find_and_update_attr_hash() {
    let content = r#"{
  version = "1.0.0";
  hash = "sha256-oldhashabcdefg";
}"#;

    let result = find_and_update_attr(
        content,
        "hash",
        "sha256-newhashabcdefg",
        Some("sha256-oldhashabcdefg"),
    );
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(updated.contains(r#"hash = "sha256-newhashabcdefg";"#));
    assert!(!updated.contains("sha256-oldhashabcdefg"));
}

#[test]
fn test_find_and_update_attr_not_found() {
    let content = r#"{
  version = "1.0.0";
}"#;

    let result = find_and_update_attr(content, "hash", "newvalue", None);
    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());
}

#[test]
fn test_find_and_update_attr_wrong_old_value() {
    let content = r#"{
  version = "1.0.0";
}"#;

    let result = find_and_update_attr(content, "version", "2.0.0", Some("9.9.9"));
    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());
}

#[test]
fn test_find_and_update_attr_preserves_formatting() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  src = {
    hash = "sha256-abc";
  };
}"#;

    let result = find_and_update_attr(content, "version", "2.0.0", Some("1.0.0"));
    assert!(result.is_ok());
    let updated = result.unwrap();

    // Check that the structure is preserved
    assert!(updated.contains("pname"));
    assert!(updated.contains("src ="));
    assert!(updated.contains(r#"version = "2.0.0";"#));
}

#[test]
fn test_find_and_update_attr_invalid_syntax() {
    let content = r#"{
  version = "1.0.0"
  # missing semicolon
}"#;

    let result = find_and_update_attr(content, "version", "2.0.0", None);
    // Should fail during initial parse validation
    assert!(result.is_err());
}

#[test]
fn test_find_and_update_attr_multiple_occurrences() {
    let content = r#"{
  version = "1.0.0";
  oldVersion = "1.0.0";
}"#;

    let result = find_and_update_attr(content, "version", "2.0.0", Some("1.0.0"));
    assert!(result.is_ok());
    let updated = result.unwrap();

    // Should only update the 'version' attribute, not 'oldVersion'
    assert!(updated.contains(r#"version = "2.0.0";"#));
    assert!(updated.contains(r#"oldVersion = "1.0.0";"#));
}

#[test]
fn test_find_and_update_attr_with_special_chars() {
    let content = r#"{
  version = "1.0.0+build.123";
}"#;

    let result = find_and_update_attr(
        content,
        "version",
        "2.0.0+build.456",
        Some("1.0.0+build.123"),
    );
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(updated.contains(r#"version = "2.0.0+build.456";"#));
}
