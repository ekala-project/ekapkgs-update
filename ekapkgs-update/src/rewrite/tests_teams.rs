//! Tests for teams manipulation (replace_teams_with_empty)

use super::*;

#[test]
fn test_replace_teams_with_empty_simple() {
    let content = r#"{
  pname = "mypackage";
  meta = {
    description = "A package";
    teams = [ teams.foo teams.bar ];
  };
}"#;

    let result = replace_teams_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(changed);
    assert!(updated.contains("teams = [ ];"));
    assert!(!updated.contains("foo"));
    assert!(!updated.contains("bar"));
}

#[test]
fn test_replace_teams_with_empty_with_lib() {
    let content = r#"{
  meta = {
    teams = with lib.teams; [ foo bar baz ];
  };
}"#;

    let result = replace_teams_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(changed);
    assert!(updated.contains("teams = [ ];"));
    assert!(!updated.contains("with lib.teams"));
}

#[test]
fn test_replace_teams_with_empty_no_teams() {
    let content = r#"{
  pname = "mypackage";
  meta = {
    description = "A package";
  };
}"#;

    let result = replace_teams_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(!changed);
    assert_eq!(updated, content);
}

#[test]
fn test_replace_teams_with_empty_already_empty() {
    let content = r#"{
  meta = {
    teams = [ ];
  };
}"#;

    let result = replace_teams_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(!changed);
    assert!(updated.contains("teams = [ ];"));
}

#[test]
fn test_replace_teams_with_empty_multiple_attributes() {
    let content = r#"{
  meta = {
    description = "A package";
    homepage = "https://example.com";
    teams = [ teams.foo ];
    license = licenses.mit;
  };
}"#;

    let result = replace_teams_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(changed);
    assert!(updated.contains("teams = [ ];"));
    assert!(updated.contains("description"));
    assert!(updated.contains("homepage"));
    assert!(updated.contains("license"));
}

#[test]
fn test_replace_teams_with_empty_preserves_indentation() {
    let content = r#"{
  meta = {
    teams = with teams; [ foo ];
  };
}"#;

    let result = replace_teams_with_empty(content);
    assert!(result.is_ok());
    let (updated, changed) = result.unwrap();
    assert!(changed);
    assert!(updated.contains("    teams = [ ];"));
}
