//! Nix file rewriting utilities using AST validation and text manipulation

mod attributes;
mod maintainers;
mod patches;
mod variants;

#[cfg(test)]
mod tests;

pub use attributes::find_and_update_attr;
pub use maintainers::replace_maintainers_with_empty;
pub use patches::{is_patches_array_empty, remove_patch_from_array, remove_patches_attribute};
pub use variants::update_variant_attr;
