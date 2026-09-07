//! Tests for variant manipulation (update_variant_attr, add_variant_entry)

use super::*;

#[test]
fn test_add_variant_entry_basic() {
    let content = r#"{
  v0_20 = {
    version = "0.20.5";
    src-hash = "sha256-abc";
  };
  v0_23 = {
    version = "0.23.2";
    src-hash = "sha256-def";
  };
}"#;

    let result = add_variant_entry(
        content,
        "v0_27",
        &[("version", "0.27.0"), ("src-hash", "sha256-new")],
    );
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(updated.contains("v0_27"));
    assert!(updated.contains(r#"version = "0.27.0";"#));
    assert!(updated.contains(r#"src-hash = "sha256-new";"#));
    // Existing variants are preserved
    assert!(updated.contains(r#"version = "0.20.5";"#));
    assert!(updated.contains(r#"version = "0.23.2";"#));
}

#[test]
fn test_add_variant_entry_preserves_existing() {
    let content = r#"{
  v1 = {
    version = "1.25.0";
    src-hash = "sha256-aaa";
  };
}"#;

    let result = add_variant_entry(
        content,
        "v2",
        &[("version", "2.0.0"), ("src-hash", "sha256-bbb")],
    );
    assert!(result.is_ok());
    let updated = result.unwrap();
    // Both variants present
    assert!(updated.contains("v1 = {"));
    assert!(updated.contains("v2 = {"));
    assert!(updated.contains(r#"version = "1.25.0";"#));
    assert!(updated.contains(r#"version = "2.0.0";"#));
}

#[test]
fn test_add_variant_entry_duplicate_rejected() {
    let content = r#"{
  v0_20 = {
    version = "0.20.5";
    src-hash = "sha256-abc";
  };
}"#;

    let result = add_variant_entry(
        content,
        "v0_20",
        &[("version", "0.20.6"), ("src-hash", "sha256-new")],
    );
    assert!(result.is_err());
}

#[test]
fn test_add_variant_entry_invalid_input() {
    let content = "{ this is not valid nix {{{";

    let result = add_variant_entry(
        content,
        "v1",
        &[("version", "1.0.0"), ("src-hash", "sha256-aaa")],
    );
    assert!(result.is_err());
}

#[test]
fn test_add_variant_entry_matches_indentation() {
    let content = r#"{
    v0_20 = {
        version = "0.20.5";
        src-hash = "sha256-abc";
    };
}"#;

    let result = add_variant_entry(
        content,
        "v0_27",
        &[("version", "0.27.0"), ("src-hash", "sha256-new")],
    );
    assert!(result.is_ok());
    let updated = result.unwrap();
    // New variant should use the same 4-space indent
    assert!(updated.contains("    v0_27 = {"));
}

#[test]
fn test_add_variant_entry_multiple_attrs() {
    let content = r#"{
  v0_20 = {
    version = "0.20.5";
    src-hash = "sha256-abc";
  };
}"#;

    let result = add_variant_entry(
        content,
        "v0_27",
        &[
            ("version", "0.27.0"),
            ("src-hash", "sha256-new"),
            ("x86_64-linux-hash", "sha256-plat1"),
            ("aarch64-linux-hash", "sha256-plat2"),
        ],
    );
    assert!(result.is_ok());
    let updated = result.unwrap();
    assert!(updated.contains(r#"x86_64-linux-hash = "sha256-plat1";"#));
    assert!(updated.contains(r#"aarch64-linux-hash = "sha256-plat2";"#));
}

#[test]
fn test_add_variant_entry_result_is_valid_nix() {
    let content = r#"{
  v0_20 = {
    version = "0.20.5";
    src-hash = "sha256-abc";
  };
}"#;

    let result = add_variant_entry(
        content,
        "v0_27",
        &[("version", "0.27.0"), ("src-hash", "sha256-new")],
    )
    .unwrap();

    // Verify the result parses as valid Nix
    let parse = rnix::Root::parse(&result);
    assert!(
        parse.errors().is_empty(),
        "Result has parse errors: {:?}",
        parse.errors()
    );
}
