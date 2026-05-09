# Architecture

> **Note:** This chapter is under construction.

## Overview

ekapkgs-update is written in Rust and consists of:

- **CLI** - Command-line interface (clap)
- **Package Metadata** - Nix evaluation and metadata extraction
- **VCS Sources** - GitHub, GitLab, PyPI integration
- **Rewriting** - Nix file modification
- **Update Logic** - Version comparison and update orchestration

## Key Modules

- `src/cli.rs` - Command-line parsing
- `src/package/` - Package metadata extraction
- `src/vcs_sources/` - Upstream version fetching
- `src/commands/` - Update and run commands
- `src/rewrite/` - Nix file rewriting

See the source code for detailed documentation.
