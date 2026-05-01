//! Nix file rewriting utilities using AST validation and text manipulation

mod attributes;
mod maintainers;
mod patches;
mod variants;

#[cfg(test)]
mod tests;

pub use attributes::*;
pub use maintainers::*;
pub use patches::*;
pub use variants::*;
