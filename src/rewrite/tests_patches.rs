//! Tests for patch array manipulation (remove_patch, is_patches_empty, remove_patches_attribute)

use super::*;

#[test]
fn test_remove_patch_from_array_simple() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [
    ./fix-build.patch
    ./add-feature.patch
    ./security-fix.patch
  ];
}"#;

    let result = remove_patch_from_array(content, "fix-build.patch");
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(!updated.contains("fix-build.patch"));
    assert!(updated.contains("add-feature.patch"));
    assert!(updated.contains("security-fix.patch"));
}

#[test]
fn test_remove_patch_from_array_middle_element() {
    let content = r#"{
  patches = [
    ./first.patch
    ./middle.patch
    ./last.patch
  ];
}"#;

    let result = remove_patch_from_array(content, "middle.patch");
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(updated.contains("first.patch"));
    assert!(!updated.contains("middle.patch"));
    assert!(updated.contains("last.patch"));
}

#[test]
fn test_remove_patch_from_array_not_found() {
    let content = r#"{
  patches = [
    ./existing.patch
  ];
}"#;

    let result = remove_patch_from_array(content, "nonexistent.patch");
    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());
}

#[test]
fn test_remove_patch_from_array_last_element() {
    let content = r#"{
  patches = [
    ./first.patch
    ./second.patch
    ./third.patch
  ];
}"#;

    let result = remove_patch_from_array(content, "third.patch");
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(updated.contains("first.patch"));
    assert!(updated.contains("second.patch"));
    assert!(!updated.contains("third.patch"));
}

#[test]
fn test_is_patches_array_empty_true() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [ ];
}"#;

    assert!(is_patches_array_empty(content));
}

#[test]
fn test_is_patches_array_empty_compact() {
    let content = r#"{
  pname = "mypackage";
  patches = [];
}"#;

    assert!(is_patches_array_empty(content));
}

#[test]
fn test_is_patches_array_empty_false() {
    let content = r#"{
  patches = [
    ./some.patch
  ];
}"#;

    assert!(!is_patches_array_empty(content));
}

#[test]
fn test_is_patches_array_empty_no_patches() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";
}"#;

    assert!(!is_patches_array_empty(content));
}

#[test]
fn test_is_patches_array_empty_with_single_line_comment() {
    let content = r#"{
  pname = "mypackage";

  patches = [ # all patches removed
  ];
}"#;

    assert!(is_patches_array_empty(content));
}

#[test]
fn test_is_patches_array_empty_with_multiline_comments() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [
    # This patch was removed
    # Another comment
  ];
}"#;

    assert!(is_patches_array_empty(content));
}

#[test]
fn test_is_patches_array_empty_with_mixed_whitespace_and_comments() {
    let content = r#"{
  patches = [

    # Comment after blank line

    # Another comment

  ];
}"#;

    assert!(is_patches_array_empty(content));
}

#[test]
fn test_is_patches_array_empty_with_multiline_comment_inline() {
    let content = r#"{
  pname = "mypackage";

  patches = [ /* all patches removed */ ];
}"#;

    assert!(is_patches_array_empty(content));
}

#[test]
fn test_is_patches_array_empty_with_multiline_comment_spanning_lines() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [
    /* This patch was removed
       because it's no longer needed
       after the upstream fix */
  ];
}"#;

    assert!(is_patches_array_empty(content));
}

#[test]
fn test_is_patches_array_empty_with_mixed_comment_styles() {
    let content = r#"{
  patches = [
    # Single line comment
    /* Multiline comment */
    # Another single line
  ];
}"#;

    assert!(is_patches_array_empty(content));
}

#[test]
fn test_remove_patches_attribute() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [ ];

  src = fetchurl {
    url = "https://example.com/file.tar.gz";
  };
}"#;

    let result = remove_patches_attribute(content);
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(!updated.contains("patches"));
    assert!(updated.contains("pname"));
    assert!(updated.contains("src"));
}

#[test]
fn test_remove_patches_attribute_not_found() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";
}"#;

    let result = remove_patches_attribute(content);
    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());
}

#[test]
fn test_remove_patches_attribute_non_empty() {
    let content = r#"{
  patches = [
    ./some.patch
  ];
}"#;

    let result = remove_patches_attribute(content);
    assert!(result.is_err());
}

#[test]
fn test_remove_patches_attribute_with_comments() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [
    # All patches were removed
    # This is now empty
  ];

  src = fetchurl {
    url = "https://example.com/file.tar.gz";
  };
}"#;

    let result = remove_patches_attribute(content);
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(!updated.contains("patches"));
    assert!(!updated.contains("All patches were removed"));
    assert!(updated.contains("pname"));
    assert!(updated.contains("src"));
}

#[test]
fn test_remove_patches_attribute_with_inline_comment() {
    let content = r#"{
  pname = "mypackage";

  patches = [ # obsolete patches removed
  ];
}"#;

    let result = remove_patches_attribute(content);
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(!updated.contains("patches"));
    assert!(!updated.contains("obsolete"));
}

#[test]
fn test_remove_patches_attribute_with_multiline_comment() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [ /* Patches no longer needed */ ];

  src = fetchurl {
    url = "https://example.com/file.tar.gz";
  };
}"#;

    let result = remove_patches_attribute(content);
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(!updated.contains("patches"));
    assert!(!updated.contains("Patches no longer needed"));
    assert!(updated.contains("pname"));
    assert!(updated.contains("src"));
}

#[test]
fn test_remove_patches_attribute_with_multiline_comment_spanning() {
    let content = r#"{
  pname = "mypackage";

  patches = [
    /* These patches were removed
       after upstream merged the fixes */
  ];

  buildInputs = [ pkg1 ];
}"#;

    let result = remove_patches_attribute(content);
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(!updated.contains("patches"));
    assert!(!updated.contains("upstream merged"));
    assert!(updated.contains("buildInputs"));
}

#[test]
fn test_remove_patches_attribute_with_mixed_comments() {
    let content = r#"{
  patches = [
    # Hash comment
    /* Block comment */
  ];
}"#;

    let result = remove_patches_attribute(content);
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(!updated.contains("patches"));
    assert!(!updated.contains("Hash comment"));
    assert!(!updated.contains("Block comment"));
}

#[test]
fn test_remove_patches_attribute_preserves_blank_lines() {
    let content = r#"{
  pname = "mypackage";
  version = "1.0.0";

  patches = [ ];

  src = fetchurl {
    url = "https://example.com/file.tar.gz";
  };
}"#;

    let result = remove_patches_attribute(content);
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(!updated.contains("patches"));

    // Verify blank line before src is preserved
    assert!(updated.contains("\n  src = "));

    // Verify indentation of src is preserved
    assert!(updated.contains("  src = fetchurl"));
}

#[test]
fn test_remove_patches_attribute_preserves_following_indentation() {
    let content = r#"{
  pname = "mypackage";
  patches = [ ];
  buildInputs = [ pkg1 pkg2 ];
}"#;

    let result = remove_patches_attribute(content);
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(!updated.contains("patches"));

    // Verify the buildInputs line maintains its indentation
    assert!(updated.contains("  buildInputs = "));
}
