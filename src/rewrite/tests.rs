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

#[test]
fn test_replace_maintainers_with_empty_simple() {
    let content = r#"{
  pname = "mypackage";
  meta = {
description = "A package";
maintainers = [ maintainers.alice maintainers.bob ];
  };
}"#;

    let result = replace_maintainers_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(changed);
    assert!(updated.contains("maintainers = [ ];"));
    assert!(!updated.contains("alice"));
    assert!(!updated.contains("bob"));
}

#[test]
fn test_replace_maintainers_with_empty_with_lib() {
    let content = r#"{
  meta = {
maintainers = with lib.maintainers; [ alice bob charlie ];
  };
}"#;

    let result = replace_maintainers_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(changed);
    assert!(updated.contains("maintainers = [ ];"));
    assert!(!updated.contains("with lib.maintainers"));
}

#[test]
fn test_replace_maintainers_with_empty_no_maintainers() {
    let content = r#"{
  pname = "mypackage";
  meta = {
description = "A package";
  };
}"#;

    let result = replace_maintainers_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(!changed);
    assert_eq!(updated, content);
}

#[test]
fn test_replace_maintainers_with_empty_already_empty() {
    let content = r#"{
  meta = {
maintainers = [ ];
  };
}"#;

    let result = replace_maintainers_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(!changed); // Not changed because maintainers is already empty
    assert!(updated.contains("maintainers = [ ];"));
}

#[test]
fn test_replace_maintainers_with_empty_multiple_attributes() {
    let content = r#"{
  meta = {
description = "A package";
homepage = "https://example.com";
maintainers = [ maintainers.alice ];
license = licenses.mit;
  };
}"#;

    let result = replace_maintainers_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(changed);
    assert!(updated.contains("maintainers = [ ];"));
    assert!(updated.contains("description"));
    assert!(updated.contains("homepage"));
    assert!(updated.contains("license"));
}

#[test]
fn test_replace_maintainers_with_empty_preserves_indentation() {
    let content = r#"{
  meta = {
    maintainers = with maintainers; [ alice ];
  };
}"#;

    let result = replace_maintainers_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(changed);
    // Check that indentation is preserved
    assert!(updated.contains("    maintainers = [ ];"));
}
