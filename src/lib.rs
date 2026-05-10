//! Library interface for ekapkgs-update
//!
//! This library target exists primarily to support doctests in the module
//! documentation. The primary interface is the binary (`src/main.rs`).

pub mod cachix;
pub mod cli;
pub mod commands;
pub mod config;
pub mod cve;
pub mod database;
pub mod directory_diff;
pub mod git;
pub mod github;
pub mod gitlab;
pub mod hash_discovery;
pub mod init;
pub mod nix;
pub mod package;
pub mod paths;
pub mod pypi;
pub mod repology;
pub mod rewrite;
pub mod sourcehut;
pub mod variant_strategy;
pub mod vcs_sources;
