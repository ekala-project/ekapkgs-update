# ekapkgs-update Documentation

This directory contains the mdbook-based documentation for ekapkgs-update.

## Building Locally

### Prerequisites

Install mdBook:

```bash
# Via cargo
cargo install mdbook

# Via Nix
nix-shell -p mdbook

# Via Nix flakes
nix run nixpkgs#mdbook
```

### Build

```bash
# Build the book
cd docs
mdbook build

# Serve locally with auto-reload
mdbook serve
# Open http://localhost:3000
```

### Clean

```bash
mdbook clean
```

## Structure

```
docs/
├── book.toml          # mdBook configuration
├── src/
│   ├── SUMMARY.md     # Table of contents
│   ├── introduction.md
│   ├── installation.md
│   ├── quick-start.md
│   ├── configuration.md
│   ├── nixos-module.md
│   ├── systemd.md
│   ├── web-dashboard.md
│   ├── cli/           # Command-line reference
│   ├── use-cases/     # Usage examples
│   ├── advanced/      # Advanced topics
│   └── reference/     # Reference documentation
└── book/              # Generated output (gitignored)
```

## Contributing

To add or update documentation:

1. Edit or create markdown files in `src/`
2. Update `src/SUMMARY.md` if adding new pages
3. Test locally with `mdbook serve`
4. Commit changes

## Deployment

Documentation is automatically deployed to GitHub Pages via `.github/workflows/docs.yml`:

- Triggered on pushes to `master`/`main` that modify `docs/**`
- Built with latest mdBook
- Deployed to https://ekapkgs.github.io/ekapkgs-update/

To trigger manual deployment:
- Go to Actions → Deploy Documentation → Run workflow

## Style Guide

### Code Blocks

Use language-specific syntax highlighting:

````markdown
```bash
ekapkgs-update run --dry-run
```

```nix
{
  services.ekapkgs-update.enable = true;
}
```
````

### Cross-References

Link to other pages using relative paths:

```markdown
See [Configuration](./configuration.md) for details.
```

### Examples

Always include practical examples:

```markdown
## Example: Update with Tests

```bash
ekapkgs-update update gcc --run-passthru-tests
```

This will:
1. Check for updates
2. Update the package
3. Run all passthru.tests
4. Only commit if tests pass
```

### Admonitions

Use blockquotes for warnings/notes:

```markdown
> **Note**: This requires GitHub token in `GITHUB_TOKEN`

> **Warning**: This will modify your repository
```

## Testing

Before committing:

1. Build locally: `mdbook build`
2. Check for broken links: `mdbook test`
3. Review in browser: `mdbook serve`

## License

Documentation is licensed under the same terms as ekapkgs-update.
