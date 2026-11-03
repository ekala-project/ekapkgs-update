use super::lint::*;

// -- Attribute parsing tests --

#[test]
fn test_parse_simple_attrs() {
    let body = r#"  pname = "hello";
  version = "1.0";

  src = ./src;

  meta = { };
"#;
    let attrs = parse_attributes(body, 1);
    assert_eq!(attrs.len(), 4);
    assert_eq!(attrs[0].name, "pname");
    assert_eq!(attrs[1].name, "version");
    assert_eq!(attrs[2].name, "src");
    assert_eq!(attrs[3].name, "meta");
}

#[test]
fn test_parse_multiline_attrs() {
    let body = r#"  pname = "test";
  version = "2.0";

  src = fetchFromGitHub {
    owner = "foo";
    repo = "bar";
    hash = "sha256-abc=";
  };

  meta = {
    description = "A test";
    license = lib.licenses.mit;
  };
"#;
    let attrs = parse_attributes(body, 1);
    assert_eq!(attrs.len(), 4);
    assert_eq!(attrs[0].name, "pname");
    assert_eq!(attrs[1].name, "version");
    assert_eq!(attrs[2].name, "src");
    assert_eq!(attrs[3].name, "meta");
}

#[test]
fn test_parse_attrs_with_lists() {
    let body = r#"  pname = "test";
  version = "1.0";

  nativeBuildInputs = [
    cmake
    pkg-config
  ];

  buildInputs = [
    openssl
    zlib
  ];
"#;
    let attrs = parse_attributes(body, 1);
    assert_eq!(attrs.len(), 4);
    assert_eq!(attrs[2].name, "nativeBuildInputs");
    assert_eq!(attrs[3].name, "buildInputs");
}

#[test]
fn test_blank_lines_counted() {
    let body = r#"  pname = "a";
  version = "1";

  src = ./src;

  meta = { };
"#;
    let attrs = parse_attributes(body, 1);
    assert_eq!(attrs[0].blank_lines_before, 0);
    assert_eq!(attrs[1].blank_lines_before, 0);
    assert_eq!(attrs[2].blank_lines_before, 1);
    assert_eq!(attrs[3].blank_lines_before, 1);
}

#[test]
fn test_skip_inherit() {
    let body = r#"  inherit pname version;

  src = ./src;
"#;
    let attrs = parse_attributes(body, 1);
    assert_eq!(attrs.len(), 1);
    assert_eq!(attrs[0].name, "src");
}

// -- Lint rule tests --

#[test]
fn test_well_formatted_no_violations() {
    let content = r#"{
  lib,
  buildPythonPackage,
  fetchPypi,
  setuptools,
  pythonOlder,
}:

buildPythonPackage (finalAttrs: {
  pname = "example";
  version = "1.0.0";
  pyproject = true;
  disabled = pythonOlder "3.6";

  src = fetchPypi {
    inherit (finalAttrs) pname version;
    hash = "sha256-abc=";
  };

  nativeBuildInputs = [ setuptools ];

  pythonImportsCheck = [ "example" ];

  meta = {
    description = "An example package";
    license = lib.licenses.mit;
    platforms = lib.platforms.all;
  };
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    let violations = check_rules(&attrs);
    assert!(
        violations.is_empty(),
        "Expected no violations, got: {violations:?}"
    );
}

#[test]
fn test_header_attr_after_body_attr() {
    let attrs = vec![
        AttrSpan {
            name: "pname".into(),
            start_line: 1,
            text: "  pname = \"a\";".into(),
            blank_lines_before: 0,
        },
        AttrSpan {
            name: "src".into(),
            start_line: 3,
            text: "  src = ./src;".into(),
            blank_lines_before: 1,
        },
        AttrSpan {
            name: "version".into(),
            start_line: 5,
            text: "  version = \"1\";".into(),
            blank_lines_before: 1,
        },
    ];
    let violations = check_rules(&attrs);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v.rule, LintRule::HeaderGrouping)),
        "Expected HeaderGrouping violation"
    );
}

#[test]
fn test_blank_line_within_header_group() {
    let attrs = vec![
        AttrSpan {
            name: "pname".into(),
            start_line: 1,
            text: "  pname = \"a\";".into(),
            blank_lines_before: 0,
        },
        AttrSpan {
            name: "version".into(),
            start_line: 3,
            text: "  version = \"1\";".into(),
            blank_lines_before: 1,
        },
        AttrSpan {
            name: "src".into(),
            start_line: 5,
            text: "  src = ./src;".into(),
            blank_lines_before: 1,
        },
    ];
    let violations = check_rules(&attrs);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v.rule, LintRule::HeaderGrouping)
                && v.message.contains("blank line within header")),
        "Expected blank-line-within-header violation"
    );
}

#[test]
fn test_missing_blank_line_between_body_attrs() {
    let attrs = vec![
        AttrSpan {
            name: "pname".into(),
            start_line: 1,
            text: "  pname = \"a\";".into(),
            blank_lines_before: 0,
        },
        AttrSpan {
            name: "version".into(),
            start_line: 2,
            text: "  version = \"1\";".into(),
            blank_lines_before: 0,
        },
        AttrSpan {
            name: "src".into(),
            start_line: 4,
            text: "  src = ./src;".into(),
            blank_lines_before: 1,
        },
        AttrSpan {
            name: "buildInputs".into(),
            start_line: 5,
            text: "  buildInputs = [];".into(),
            blank_lines_before: 0,
        },
    ];
    let violations = check_rules(&attrs);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v.rule, LintRule::BlankLineSeparation)
                && v.message.contains("buildInputs")),
        "Expected blank-line violation for buildInputs"
    );
}

#[test]
fn test_meta_not_at_end() {
    let attrs = vec![
        AttrSpan {
            name: "pname".into(),
            start_line: 1,
            text: "  pname = \"a\";".into(),
            blank_lines_before: 0,
        },
        AttrSpan {
            name: "meta".into(),
            start_line: 3,
            text: "  meta = { };".into(),
            blank_lines_before: 1,
        },
        AttrSpan {
            name: "src".into(),
            start_line: 5,
            text: "  src = ./src;".into(),
            blank_lines_before: 1,
        },
    ];
    let violations = check_rules(&attrs);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v.rule, LintRule::MetaAtEnd)),
        "Expected MetaAtEnd violation"
    );
}

#[test]
fn test_no_meta_no_violation() {
    let attrs = vec![
        AttrSpan {
            name: "pname".into(),
            start_line: 1,
            text: "  pname = \"a\";".into(),
            blank_lines_before: 0,
        },
        AttrSpan {
            name: "src".into(),
            start_line: 3,
            text: "  src = ./src;".into(),
            blank_lines_before: 1,
        },
    ];
    let violations = check_rules(&attrs);
    assert!(violations.is_empty());
}

// -- Fix tests --

#[test]
fn test_fix_adds_blank_lines() {
    let content = r#"buildPythonPackage (finalAttrs: {
  pname = "test";
  version = "1.0";
  src = ./src;
  meta = { };
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    let fixed = apply_fix(content, &range, &attrs);

    assert!(
        fixed.contains("  version = \"1.0\";\n\n  src"),
        "Expected blank line before src"
    );
    assert!(
        fixed.contains("  src = ./src;\n\n  meta"),
        "Expected blank line before meta"
    );
}

#[test]
fn test_fix_moves_meta_to_end() {
    let content = r#"buildPythonPackage (finalAttrs: {
  pname = "test";
  version = "1.0";

  meta = { };

  src = ./src;
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    let fixed = apply_fix(content, &range, &attrs);

    // meta should come after src
    let src_pos = fixed.find("src = ./src;").unwrap();
    let meta_pos = fixed.find("meta = { };").unwrap();
    assert!(
        meta_pos > src_pos,
        "meta should be after src in fixed output"
    );
}

#[test]
fn test_fix_moves_header_attrs_to_top() {
    let content = r#"buildPythonPackage (finalAttrs: {
  src = ./src;

  pname = "test";

  version = "1.0";

  meta = { };
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    let fixed = apply_fix(content, &range, &attrs);

    let pname_pos = fixed.find("pname =").unwrap();
    let version_pos = fixed.find("version =").unwrap();
    let src_pos = fixed.find("src =").unwrap();
    assert!(pname_pos < version_pos, "pname before version");
    assert!(version_pos < src_pos, "version before src");
}

#[test]
fn test_fix_idempotent_on_well_formatted() {
    let content = r#"buildPythonPackage (finalAttrs: {
  pname = "test";
  version = "1.0";
  pyproject = true;

  src = ./src;

  nativeBuildInputs = [ setuptools ];

  meta = { };
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    let fixed = apply_fix(content, &range, &attrs);
    assert_eq!(
        fixed, content,
        "Fix should not change already-compliant file"
    );
}

#[test]
fn test_fix_produces_valid_nix() {
    let content = r#"{ lib, buildPythonPackage, fetchPypi, setuptools }:

buildPythonPackage (finalAttrs: {
  pname = "example";
  version = "1.0";
  meta = {
    description = "test";
    license = lib.licenses.mit;
    platforms = lib.platforms.all;
  };
  src = fetchPypi {
    inherit (finalAttrs) pname version;
    hash = "sha256-abc=";
  };
  nativeBuildInputs = [ setuptools ];
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    let fixed = apply_fix(content, &range, &attrs);

    let parse = rnix::Root::parse(&fixed);
    assert!(
        parse.errors().is_empty(),
        "Fixed output should be valid Nix, got errors: {:?}",
        parse.errors()
    );
}

// -- Derivation detection tests --

#[test]
fn test_detect_attrset_with_pname_and_version() {
    let content = r#"buildPythonPackage (finalAttrs: {
  pname = "x";
  version = "1.0";
})"#;
    assert!(find_derivation_attrset(content).is_some());
}

#[test]
fn test_detect_attrset_with_inherit_version() {
    let content = r#"stdenv.mkDerivation (finalAttrs: {
  pname = "x";
  inherit version;
})"#;
    assert!(find_derivation_attrset(content).is_some());
}

#[test]
fn test_detect_skips_attrset_without_version() {
    let content = r#"stdenv.mkDerivation (finalAttrs: rec {
  pname = "x";
})"#;
    assert!(find_derivation_attrset(content).is_none());
}

#[test]
fn test_no_derivation_in_plain_file() {
    let content = r#"{ x = 1; y = 2; }"#;
    assert!(find_derivation_attrset(content).is_none());
}

// -- find_matching_brace tests --

#[test]
fn test_matching_brace_simple() {
    let content = "{ a = 1; }";
    assert_eq!(find_matching_brace(content, 0), Some(9));
}

#[test]
fn test_matching_brace_nested() {
    let content = "{ a = { b = 1; }; }";
    assert_eq!(find_matching_brace(content, 0), Some(18));
}

#[test]
fn test_matching_brace_string() {
    let content = r#"{ a = "} not a brace"; }"#;
    assert_eq!(find_matching_brace(content, 0), Some(23));
}

#[test]
fn test_matching_brace_comment() {
    let content = "{ a = 1; # } not a brace\n}";
    assert_eq!(find_matching_brace(content, 0), Some(25));
}

// -- Let block tests --

#[test]
fn test_parse_attrs_with_let_in_value() {
    let content = r#"{ lib, stdenv }:

stdenv.mkDerivation (finalAttrs: {
  pname = "example";
  version = let
    maj = "1";
    min = "0";
  in "${maj}.${min}";

  src = ./src;

  meta = {
    description = "test";
    license = lib.licenses.mit;
    platforms = lib.platforms.all;
  };
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);

    assert_eq!(attrs.len(), 4);
    assert_eq!(attrs[0].name, "pname");
    assert_eq!(attrs[1].name, "version");
    assert_eq!(attrs[2].name, "src");
    assert_eq!(attrs[3].name, "meta");

    let violations = check_rules(&attrs);
    assert!(
        violations.is_empty(),
        "Expected no violations, got: {violations:?}"
    );
}

#[test]
fn test_fix_with_let_block_preserves_content() {
    let content = r#"{ lib, stdenv }:

stdenv.mkDerivation (finalAttrs: {
  pname = "example";
  version = let
    maj = "1";
    min = "0";
  in "${maj}.${min}";
  src = ./src;
  meta = {
    description = "test";
    license = lib.licenses.mit;
    platforms = lib.platforms.all;
  };
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);

    // Should have violations: missing blank lines before src and meta
    let violations = check_rules(&attrs);
    assert!(!violations.is_empty());

    let fixed = apply_fix(content, &range, &attrs);

    // Should still be valid Nix
    let parse = rnix::Root::parse(&fixed);
    assert!(
        parse.errors().is_empty(),
        "Fixed output should be valid Nix, got errors: {:?}",
        parse.errors()
    );

    // The let block should be preserved intact
    assert!(
        fixed
            .contains("version = let\n    maj = \"1\";\n    min = \"0\";\n  in \"${maj}.${min}\";")
    );

    // Blank lines should be added
    assert!(fixed.contains("in \"${maj}.${min}\";\n\n  src"));
    assert!(fixed.contains("src = ./src;\n\n  meta"));
}

#[test]
fn test_parse_attrs_with_complex_let_block() {
    let content = r#"{ lib, stdenv, fetchFromGitHub }:

stdenv.mkDerivation (finalAttrs: {
  pname = "complex";
  version = "2.0";

  src = let
    base = fetchFromGitHub {
      owner = "foo";
      repo = "bar";
      tag = finalAttrs.version;
      hash = "sha256-abc=";
    };
  in base;

  env = let
    flags = [ "-O2" "-Wall" ];
    joined = builtins.concatStringsSep " " flags;
  in {
    CFLAGS = joined;
    LDFLAGS = "-L/lib";
  };

  meta = {
    description = "A package with complex let blocks";
    license = lib.licenses.mit;
    platforms = lib.platforms.all;
  };
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);

    assert_eq!(attrs.len(), 5, "attrs: {attrs:?}");
    assert_eq!(attrs[0].name, "pname");
    assert_eq!(attrs[1].name, "version");
    assert_eq!(attrs[2].name, "src");
    assert_eq!(attrs[3].name, "env");
    assert_eq!(attrs[4].name, "meta");

    let violations = check_rules(&attrs);
    assert!(
        violations.is_empty(),
        "Expected no violations, got: {violations:?}"
    );
}

#[test]
fn test_fix_with_assert_and_with_expressions() {
    let content = r#"{ lib, buildPythonPackage, fetchPypi }:

buildPythonPackage (finalAttrs: {
  pname = "example";
  version = "1.0";
  src = fetchPypi {
    inherit (finalAttrs) pname version;
    hash = "sha256-abc=";
  };
  meta = with lib; {
    description = "test";
    license = licenses.mit;
    platforms = platforms.all;
  };
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);

    assert_eq!(attrs.len(), 4);
    assert_eq!(attrs[3].name, "meta");

    // Fix should add blank lines
    let fixed = apply_fix(content, &range, &attrs);
    let parse = rnix::Root::parse(&fixed);
    assert!(parse.errors().is_empty(), "Errors: {:?}", parse.errors());

    // meta with `with lib;` should be preserved intact
    assert!(fixed.contains("meta = with lib; {"));
}

// -- inherit preservation tests --

#[test]
fn test_fix_preserves_inherit_statements() {
    let content = r#"stdenv.mkDerivation (finalAttrs: {
  pname = "cairomm";
  inherit version;
  src = ./src;
  buildInputs = [ boost ];
  meta = { };
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    let fixed = apply_fix(content, &range, &attrs);

    // inherit must still be present
    assert!(
        fixed.contains("inherit version;"),
        "inherit statement was dropped! Fixed:\n{fixed}"
    );
    // meta must still be present
    assert!(
        fixed.contains("meta = { };"),
        "meta was dropped! Fixed:\n{fixed}"
    );
    // must be valid Nix
    let parse = rnix::Root::parse(&fixed);
    assert!(parse.errors().is_empty(), "Errors: {:?}", parse.errors());
}

#[test]
fn test_fix_preserves_comments() {
    let content = r#"stdenv.mkDerivation (finalAttrs: {
  pname = "test";
  version = "1.0";
  # This is an important comment
  src = ./src;
  meta = { };
})
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    let fixed = apply_fix(content, &range, &attrs);

    assert!(
        fixed.contains("# This is an important comment"),
        "Comment was dropped! Fixed:\n{fixed}"
    );
    let parse = rnix::Root::parse(&fixed);
    assert!(parse.errors().is_empty(), "Errors: {:?}", parse.errors());
}

// -- trailing comment tests --

#[test]
fn test_fix_preserves_trailing_line_comment() {
    let content = r#"stdenv.mkDerivation {
  pname = "test";
  version = "1.0";

  doCheck = false; # broken upstream
  checkPhase = "make check";

  meta = { };
}
"#;
    let range = find_derivation_attrset(content).unwrap();
    let body = &content[range.body_start..range.body_end];
    let body_start_line = content[..range.body_start].lines().count() + 1;
    let attrs = parse_attributes(body, body_start_line);
    let fixed = apply_fix(content, &range, &attrs);

    assert!(
        fixed.contains("doCheck = false; # broken upstream"),
        "Trailing comment was split from its statement! Fixed:\n{fixed}"
    );
    let parse = rnix::Root::parse(&fixed);
    assert!(
        parse.errors().is_empty(),
        "Errors: {:?}\nFixed:\n{fixed}",
        parse.errors()
    );
}

// -- lint_content function tests --

#[test]
fn test_lint_content_returns_violations() {
    let content = r#"buildPythonPackage (finalAttrs: {
  pname = "test";
  version = "1.0";
  meta = { };
  src = ./src;
})
"#;
    let violations = lint_content(content);
    assert!(violations.is_some());
    let v = violations.unwrap();
    assert!(!v.is_empty(), "Should have violations");
}

#[test]
fn test_fix_content_fixes_violations() {
    let content = r#"buildPythonPackage (finalAttrs: {
  pname = "test";
  version = "1.0";
  meta = { };
  src = ./src;
})
"#;
    let fixed = fix_content(content).unwrap();
    assert!(fixed.is_some());
    let fixed_content = fixed.unwrap();

    // Verify meta is at end, blank lines present
    let src_pos = fixed_content.find("src =").unwrap();
    let meta_pos = fixed_content.find("meta =").unwrap();
    assert!(meta_pos > src_pos);
    assert!(fixed_content.contains("src = ./src;\n\n  meta"));
}
