//! Typed errors for the `rewrite` module.
//!
//! Public AST-rewriting functions in this module return [`RewriteError`] so
//! callers (and tests) can pattern-match on specific failure modes instead
//! of resorting to `e.to_string().contains("not found")` heuristics.
//!
//! `RewriteError` implements `std::error::Error`, so it round-trips into
//! `anyhow::Error` via `?` at the application boundary.

use thiserror::Error;

/// Errors produced by Nix-file rewriting operations.
#[derive(Debug, Error)]
pub enum RewriteError {
    /// The input content failed to parse as Nix.
    #[error("Failed to parse Nix file: {0}")]
    Parse(String),

    /// A target attribute, variant, or patch was not found.
    ///
    /// This is the "not found" variant that callers (e.g.,
    /// `commands::update::file_update`) inspect to fall back to alternative
    /// search strategies (e.g., sibling `mkManyVariants` files).
    #[error("{what} '{name}' not found{}", .context.as_deref().map(|c| format!(" {c}")).unwrap_or_default())]
    NotFound {
        /// What kind of thing was missing (e.g., "Attribute", "Variant", "Patch").
        what: &'static str,
        /// The name that was searched for.
        name: String,
        /// Optional disambiguating context (e.g., "in variant 'v0_20'").
        context: Option<String>,
    },

    /// Applying the rewrite would have produced invalid Nix syntax.
    #[error("{operation} would create invalid Nix syntax")]
    InvalidResult {
        /// Which operation produced the invalid result (e.g., "Replacement",
        /// "Removal").
        operation: &'static str,
    },

    /// A regex used to locate or extract content was invalid.
    #[error(transparent)]
    Regex(#[from] regex::Error),

    /// A structural invariant was violated (used sparingly for cases that
    /// should be unreachable in well-formed input).
    #[error("{0}")]
    Structural(String),
}

impl RewriteError {
    /// Construct a `NotFound` for an attribute.
    pub fn attr_not_found(name: impl Into<String>) -> Self {
        RewriteError::NotFound {
            what: "Attribute",
            name: name.into(),
            context: None,
        }
    }

    /// Construct a `NotFound` for an attribute scoped to a particular variant.
    pub fn attr_not_found_in_variant(
        attr_name: impl Into<String>,
        variant_name: impl Into<String>,
    ) -> Self {
        RewriteError::NotFound {
            what: "Attribute",
            name: attr_name.into(),
            context: Some(format!("in variant '{}'", variant_name.into())),
        }
    }

    /// Construct a `NotFound` for a patch.
    pub fn patch_not_found(name: impl Into<String>) -> Self {
        RewriteError::NotFound {
            what: "Patch",
            name: name.into(),
            context: Some("in patches array".to_owned()),
        }
    }

    /// Construct a `NotFound` for a variant.
    pub fn variant_not_found(name: impl Into<String>) -> Self {
        RewriteError::NotFound {
            what: "Variant",
            name: name.into(),
            context: Some("in file".to_owned()),
        }
    }

    /// Construct a `NotFound` for a missing empty `patches = [];` attribute.
    pub fn empty_patches_not_found() -> Self {
        RewriteError::NotFound {
            what: "Empty patches attribute",
            name: "patches".to_owned(),
            context: None,
        }
    }

    /// Returns true if this error indicates a "not found" condition.
    ///
    /// Provided for ergonomic pattern matching in callers that historically
    /// used `e.to_string().contains("not found")`.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, RewriteError::NotFound { .. })
    }
}

/// Convenience alias for results from this module.
pub type Result<T> = std::result::Result<T, RewriteError>;
