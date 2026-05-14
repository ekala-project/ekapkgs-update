//! Nix file rewriting utilities using AST validation and text manipulation
//!
//! Public functions in this module return [`RewriteError`] so callers can
//! discriminate failure modes (notably [`RewriteError::NotFound`]) without
//! string-matching on error messages.

mod attributes;
mod error;
mod maintainers;
mod patches;
mod rev_update;
mod variants;

#[cfg(test)]
mod tests_attributes;
#[cfg(test)]
mod tests_maintainers;
#[cfg(test)]
mod tests_patches;
#[cfg(test)]
mod tests_rev;

pub use attributes::find_and_update_attr;
// `RewriteError` is intentionally kept private to the `rewrite` module: callers
// don't need to name the type — they reach `is_not_found()` (and `Display`)
// via method resolution on the `Err` half of the public functions' return
// type. Tests in `rewrite/tests.rs` use `super::*` to access the type.
pub use maintainers::replace_maintainers_with_empty;
pub use patches::{is_patches_array_empty, remove_patch_from_array, remove_patches_attribute};
pub use rev_update::try_update_rev_attr;
pub use variants::update_variant_attr;
