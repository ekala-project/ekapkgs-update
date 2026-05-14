//! Tests for maintainer manipulation (replace_maintainers_with_empty)

use super::*;

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
