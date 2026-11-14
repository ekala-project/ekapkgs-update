//! Tests for fetchPypi rewriting (update_fetch_pypi_sdist, update_fetch_pypi_wheel)

use super::error::RewriteError;
use super::*;

fn package(src_body: &str) -> String {
    format!(
        "buildPythonPackage (finalAttrs: {{\n  pname = \"foo-bar\";\n  version = \"2.0\";\n\n  \
         src = fetchPypi {{\n{src_body}\n  }};\n}})\n"
    )
}

#[test]
fn test_update_fetch_pypi_sdist() {
    let cases: &[(&str, Option<&str>, Option<&str>, &str)] = &[
        // inherit (finalAttrs) pname version → explicit pname
        (
            "    inherit (finalAttrs) pname version;\n    hash = \"sha256-x\";",
            Some("foo_bar"),
            None,
            "    pname = \"foo_bar\";\n    inherit (finalAttrs) version;\n    hash = \"sha256-x\";",
        ),
        // pname listed after version
        (
            "    inherit (finalAttrs) version pname;\n    hash = \"sha256-x\";",
            Some("foo_bar"),
            None,
            "    pname = \"foo_bar\";\n    inherit (finalAttrs) version;\n    hash = \"sha256-x\";",
        ),
        // plain inherit from a let binding
        (
            "    inherit pname version;\n    hash = \"sha256-x\";",
            Some("foo_bar"),
            None,
            "    pname = \"foo_bar\";\n    inherit version;\n    hash = \"sha256-x\";",
        ),
        // inherit holding only pname is replaced outright
        (
            "    inherit (finalAttrs) pname;\n    version = \"2.0\";",
            Some("foo_bar"),
            None,
            "    pname = \"foo_bar\";\n    version = \"2.0\";",
        ),
        // explicit pname is replaced
        (
            "    pname = \"Foo-Bar\";\n    inherit (finalAttrs) version;\n    hash = \"sha256-x\";",
            Some("foo_bar"),
            None,
            "    pname = \"foo_bar\";\n    inherit (finalAttrs) version;\n    hash = \"sha256-x\";",
        ),
        // zip → default tar.gz removes the extension
        (
            "    inherit (finalAttrs) pname version;\n    hash = \"sha256-x\";\n    extension = \"zip\";",
            None,
            Some("tar.gz"),
            "    inherit (finalAttrs) pname version;\n    hash = \"sha256-x\";",
        ),
        // extension replaced
        (
            "    inherit (finalAttrs) pname version;\n    extension = \"tar.bz2\";\n    hash = \"sha256-x\";",
            None,
            Some("zip"),
            "    inherit (finalAttrs) pname version;\n    extension = \"zip\";\n    hash = \"sha256-x\";",
        ),
        // extension added
        (
            "    inherit (finalAttrs) pname version;\n    hash = \"sha256-x\";",
            None,
            Some("zip"),
            "    inherit (finalAttrs) pname version;\n    hash = \"sha256-x\";\n    extension = \"zip\";",
        ),
        // default extension with none present is a no-op
        (
            "    inherit (finalAttrs) pname version;\n    hash = \"sha256-x\";",
            None,
            Some("tar.gz"),
            "    inherit (finalAttrs) pname version;\n    hash = \"sha256-x\";",
        ),
        // both at once
        (
            "    inherit (finalAttrs) pname version;\n    hash = \"sha256-x\";\n    extension = \"zip\";",
            Some("foo_bar"),
            Some("tar.gz"),
            "    pname = \"foo_bar\";\n    inherit (finalAttrs) version;\n    hash = \"sha256-x\";",
        ),
        // interpolation in the body
        (
            "    pname = \"Foo\";\n    version = \"${finalAttrs.version}\";",
            Some("foo"),
            None,
            "    pname = \"foo\";\n    version = \"${finalAttrs.version}\";",
        ),
    ];

    for (body, pname, extension, expected_body) in cases {
        let content = package(body);
        let expected = package(expected_body);
        let updated = update_fetch_pypi_sdist(&content, *pname, *extension)
            .unwrap_or_else(|e| panic!("rewrite failed for body {body:?}: {e}"));
        assert_eq!(updated, expected, "body: {body:?}");
    }
}

#[test]
fn test_update_fetch_pypi_sdist_capital_p() {
    let content = "{ src = fetchPyPi {\n    inherit pname version;\n  }; }";
    let updated = update_fetch_pypi_sdist(content, Some("foo_bar"), None).unwrap();
    assert!(updated.contains("pname = \"foo_bar\";\n    inherit version;"));
}

#[test]
fn test_update_fetch_pypi_sdist_leaves_outer_pname() {
    let content = package("    inherit (finalAttrs) pname version;");
    let updated = update_fetch_pypi_sdist(&content, Some("foo_bar"), None).unwrap();
    assert!(updated.contains("  pname = \"foo-bar\";\n  version = \"2.0\";"));
}

#[test]
fn test_update_fetch_pypi_sdist_errors() {
    // No fetchPypi call
    let err = update_fetch_pypi_sdist("{ src = fetchurl { }; }", Some("x"), None).unwrap_err();
    assert!(err.is_not_found());

    // pname neither set nor inherited
    let err = update_fetch_pypi_sdist(
        &package("    inherit (finalAttrs) version;"),
        Some("x"),
        None,
    )
    .unwrap_err();
    assert!(err.is_not_found());

    // Two calls: ambiguous
    let content =
        "{ a = fetchPypi { inherit pname version; }; b = fetchPypi { inherit pname version; }; }";
    let err = update_fetch_pypi_sdist(content, Some("x"), None).unwrap_err();
    assert!(matches!(err, RewriteError::Structural(_)));

    // Invalid input
    let err = update_fetch_pypi_sdist("{ src = fetchPypi { ", Some("x"), None).unwrap_err();
    assert!(matches!(err, RewriteError::Parse(_)));
}

#[test]
fn test_update_fetch_pypi_wheel() {
    type Case<'a> = (
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        &'a str,
    );
    let cases: &[Case<'_>] = &[
        // python tag change: both default independently, so both are added
        (
            "    inherit (finalAttrs) pname version;\n    format = \"wheel\";\n    hash = \"sha256-x\";",
            None,
            Some("py3"),
            Some("py3"),
            "    inherit (finalAttrs) pname version;\n    format = \"wheel\";\n    hash = \"sha256-x\";\n    python = \"py3\";\n    dist = \"py3\";",
        ),
        // existing attributes are replaced in place
        (
            "    inherit (finalAttrs) pname version;\n    format = \"wheel\";\n    python = \"py2\";\n    dist = \"py2\";\n    hash = \"sha256-x\";",
            None,
            Some("py3"),
            Some("py3"),
            "    inherit (finalAttrs) pname version;\n    format = \"wheel\";\n    python = \"py3\";\n    dist = \"py3\";\n    hash = \"sha256-x\";",
        ),
        // back to the default removes them
        (
            "    inherit pname version;\n    format = \"wheel\";\n    python = \"py3\";\n    dist = \"py3\";\n    hash = \"sha256-x\";",
            None,
            Some("py2.py3"),
            Some("py2.py3"),
            "    inherit pname version;\n    format = \"wheel\";\n    hash = \"sha256-x\";",
        ),
        // only the directory
        (
            "    inherit pname version;\n    format = \"wheel\";\n    python = \"py3\";",
            None,
            None,
            Some("py3"),
            "    inherit pname version;\n    format = \"wheel\";\n    python = \"py3\";\n    dist = \"py3\";",
        ),
        // name spelling
        (
            "    inherit (finalAttrs) pname version;\n    format = \"wheel\";",
            Some("foo_bar"),
            None,
            None,
            "    pname = \"foo_bar\";\n    inherit (finalAttrs) version;\n    format = \"wheel\";",
        ),
    ];

    for (body, pname, python, dist, expected_body) in cases {
        let content = package(body);
        let expected = package(expected_body);
        let updated = update_fetch_pypi_wheel(&content, *pname, *python, *dist)
            .unwrap_or_else(|e| panic!("rewrite failed for body {body:?}: {e}"));
        assert_eq!(updated, expected, "body: {body:?}");
    }
}

#[test]
fn test_update_fetch_pypi_wheel_refuses_non_literal() {
    let bodies = [
        // `dist = python;` would gain a second `dist` binding
        "    inherit pname version;\n    format = \"wheel\";\n    python = \"py3\";\n    dist = python;",
        // inherited from the enclosing scope
        "    inherit pname version python dist;\n    format = \"wheel\";",
        "    inherit pname version;\n    inherit (finalAttrs) dist;\n    format = \"wheel\";",
    ];
    for body in bodies {
        let err = update_fetch_pypi_wheel(&package(body), None, Some("py3"), Some("py3"))
            .expect_err(body);
        assert!(matches!(err, RewriteError::Structural(_)), "body: {body:?}");
    }
}
