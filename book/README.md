# ekapkgs-update Documentation

This directory contains the user-facing documentation for ekapkgs-update, built with [mdBook](https://rust-lang.github.io/mdBook/).

## Building the Documentation

### Prerequisites

Install mdBook:

```bash
cargo install mdbook
```

Or with Nix:

```bash
nix-shell -p mdbook
```

### Build

```bash
cd book
mdbook build
```

The generated HTML will be in `book/book/`.

### Serve Locally

```bash
cd book
mdbook serve
```

Then open http://localhost:3000 in your browser.

### Watch for Changes

```bash
cd book
mdbook watch
```

## Documentation Structure

- **Introduction** - Overview and features
- **User Guide** - Installation, quick start, and usage
- **Passthru Attributes** - Comprehensive EEP-0039 documentation
  - `skip` - Disable automatic updates
  - `semver-strategy` - Version constraint strategies
  - `include-prereleases` - Prerelease acceptance
  - `version-regex` - Custom tag extraction
- **Reference** - CLI commands and configuration
- **Advanced Topics** - Special features and package types
- **Contributing** - Development and architecture

## Contributing to Documentation

### Adding New Pages

1. Create a new `.md` file in `src/`
2. Add an entry to `src/SUMMARY.md`
3. Build and verify with `mdbook serve`

### Improving Existing Pages

Many chapters are currently stubs (marked with "Note: This chapter is under construction").
Contributions to expand these chapters are welcome!

### Style Guide

- Use code blocks with language hints: ` ```nix `, ` ```bash `
- Include examples for each feature
- Add troubleshooting sections for complex topics
- Cross-reference related pages

## Deployment

The documentation can be published to GitHub Pages:

```bash
mdbook build
# Deploy book/book/ to gh-pages branch
```

## Questions?

For issues with the documentation content or structure, please open an issue on the ekapkgs-update repository.
