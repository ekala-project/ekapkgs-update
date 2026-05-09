# Development Guide

This guide will help you set up a development environment and contribute to ekapkgs-update.

## Prerequisites

- **Nix** - For the development environment and testing
- **Git** - For version control
- **GitHub account** - For contributing via pull requests

## Getting Started

### 1. Fork and Clone

```bash
# Fork the repository on GitHub first

# Clone your fork
git clone https://github.com/YOUR_USERNAME/ekapkgs-update
cd ekapkgs-update

# Add upstream remote
git remote add upstream https://github.com/ekala-project/ekapkgs-update
```

### 2. Development Environment

Enter the Nix development shell:

```bash
nix develop
```

This provides:
- Rust toolchain (stable)
- Cargo
- rustfmt
- clippy
- All build dependencies
- Development tools

**Without Nix flakes:**

```bash
nix-shell
```

### 3. Build

Build the project:

```bash
# Debug build (faster, for development)
cargo build

# Release build (optimized)
cargo build --release
```

The binary will be at:
- Debug: `target/debug/ekapkgs-update`
- Release: `target/release/ekapkgs-update`

### 4. Run

```bash
# Run from source
cargo run -- --help

# Run specific command
cargo run -- update --help

# Run with arguments
cargo run -- update mypackage --dry-run
```

## Testing

### Run All Tests

```bash
cargo test
```

### Run Specific Test

```bash
# Run tests matching a pattern
cargo test test_semver_strategy

# Run tests in a specific module
cargo test vcs_sources::

# Run tests in a file
cargo test --test integration_test
```

### Test with Output

```bash
# Show println! output
cargo test -- --nocapture

# Show test names as they run
cargo test -- --test-threads=1 --nocapture
```

### Integration Tests

```bash
# Run only integration tests
cargo test --test '*'

# Run specific integration test
cargo test --test update_test
```

## Code Quality

### Format Code

```bash
# Check formatting
cargo fmt --check

# Format all code
cargo fmt
```

**Note:** CI enforces formatting. Always run `cargo fmt` before committing.

### Linting

```bash
# Run clippy
cargo clippy

# Fix warnings automatically (when possible)
cargo clippy --fix
```

**Clippy is enforced in CI.** Fix all warnings before submitting a PR.

### All Checks at Once

```bash
# Run before committing
cargo fmt && cargo clippy && cargo test
```

## Development Workflow

### 1. Create a Branch

```bash
# Create feature branch
git checkout -b feature/my-new-feature

# Create bugfix branch
git checkout -b fix/issue-123
```

### 2. Make Changes

Edit code, add tests, update documentation.

### 3. Test Locally

```bash
# Run tests
cargo test

# Test manually
cargo run -- update mypackage --dry-run

# Check formatting and linting
cargo fmt && cargo clippy
```

### 4. Commit

```bash
git add .
git commit -m "Add feature: description

Detailed explanation of changes.

Fixes #123"
```

**Commit message guidelines:**

- Use imperative mood ("Add feature" not "Added feature")
- First line: concise summary (50 chars or less)
- Blank line after summary
- Detailed description (if needed)
- Reference issues with "Fixes #123"

### 5. Push and Create PR

```bash
# Push to your fork
git push -u origin feature/my-new-feature

# Create PR on GitHub
gh pr create --title "Add feature: description" --body "..."
```

## Debugging

### Enable Debug Logging

```bash
RUST_LOG=debug cargo run -- update mypackage
```

### Debug Specific Module

```bash
RUST_LOG=ekapkgs_update::rewrite=trace cargo run -- update mypackage
```

### Use Debugger

With `rust-gdb`:

```bash
cargo build
rust-gdb target/debug/ekapkgs-update

# In gdb:
(gdb) run update mypackage
(gdb) break src/commands/update/mod.rs:100
```

With VS Code:

1. Install "rust-analyzer" extension
2. Install "CodeLLDB" extension
3. Add debug configuration in `.vscode/launch.json`:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "lldb",
      "request": "launch",
      "name": "Debug ekapkgs-update",
      "cargo": {
        "args": ["build", "--bin=ekapkgs-update"],
        "filter": {
          "name": "ekapkgs-update",
          "kind": "bin"
        }
      },
      "args": ["update", "mypackage"],
      "cwd": "${workspaceFolder}",
      "env": {
        "RUST_LOG": "debug"
      }
    }
  ]
}
```

## Project Structure

```
ekapkgs-update/
├── src/
│   ├── lib.rs              # Library root
│   ├── main.rs             # Binary entry point
│   ├── cli.rs              # CLI argument parsing
│   ├── config.rs           # Configuration types
│   ├── package/            # Package metadata
│   │   ├── mod.rs          # PackageMetadata extraction
│   │   └── flake.rs        # Flake package support
│   ├── vcs_sources/        # VCS integrations
│   │   ├── mod.rs          # Release types, version matching
│   │   ├── github.rs       # GitHub API client
│   │   ├── gitlab.rs       # GitLab API client
│   │   ├── pypi.rs         # PyPI API client
│   │   └── git.rs          # Git tag fetching
│   ├── rewrite/            # Nix file rewriting
│   │   ├── mod.rs          # AST manipulation
│   │   └── tests.rs        # Rewriting tests
│   ├── commands/           # Command implementations
│   │   ├── update/         # Update command
│   │   │   ├── mod.rs      # Main update logic
│   │   │   ├── config.rs   # Update configuration
│   │   │   ├── git.rs      # Git operations
│   │   │   ├── pr.rs       # PR creation
│   │   │   ├── build.rs    # Package building
│   │   │   └── ...
│   │   └── run/            # Daemon mode
│   │       ├── mod.rs      # Main daemon logic
│   │       ├── checker.rs  # Update checking
│   │       ├── updater.rs  # Update execution
│   │       └── ...
│   └── ...
├── tests/                  # Integration tests
├── book/                   # mdBook documentation
├── Cargo.toml              # Dependencies
├── flake.nix               # Nix flake
└── README.md
```

## Adding Features

### Adding a New Command

1. Add command variant to `src/cli.rs`:

```rust
pub enum Command {
    Update(UpdateArgs),
    Run(RunArgs),
    YourNewCommand(YourNewCommandArgs),  // Add this
}
```

2. Create command implementation in `src/commands/your_command.rs`:

```rust
pub async fn run(args: YourNewCommandArgs) -> anyhow::Result<()> {
    // Implementation
    Ok(())
}
```

3. Add to `src/commands/mod.rs`:

```rust
pub mod your_command;
```

4. Add dispatch in `src/main.rs`:

```rust
Command::YourNewCommand(args) => commands::your_command::run(args).await?,
```

5. Add tests in `tests/your_command_test.rs`

6. Update documentation in `book/src/`

### Adding a New VCS Source

1. Create `src/vcs_sources/your_vcs.rs`:

```rust
use super::{Release, VersionSource};

pub struct YourVcs {
    // Fields
}

impl YourVcs {
    pub async fn get_releases(&self) -> anyhow::Result<Vec<Release>> {
        // Fetch releases from your VCS
        Ok(vec![])
    }
}
```

2. Add to `src/vcs_sources/mod.rs`:

```rust
pub mod your_vcs;
pub use your_vcs::YourVcs;
```

3. Integrate in version source detection

4. Add tests

### Adding a New Passthru Attribute

1. Add field to `PackageMetadata` in `src/package/mod.rs`:

```rust
pub struct PackageMetadata {
    // ... existing fields
    pub your_new_attr: Option<YourType>,
}
```

2. Query attribute in `PackageMetadata::from_attr_path`:

```rust
let your_new_attr = package
    .get_attr("passthru.ekapkgs-update.your-new-attr or null")
    .await
    .and_then(|s| parse_your_type(s));
```

3. Use attribute in update logic (`src/commands/update/mod.rs` or `src/commands/run/checker.rs`)

4. Update tests

5. Update documentation:
   - Create `book/src/passthru-attributes/your-new-attr.md`
   - Add to `book/src/SUMMARY.md`
   - Update `book/src/passthru-attributes.md`

6. Update EEP-0039 in the eeps repository

## Testing Guidelines

### Unit Tests

Place unit tests in the same file as the code:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_something() {
        assert_eq!(1 + 1, 2);
    }
}
```

### Integration Tests

Place integration tests in `tests/`:

```rust
// tests/update_test.rs

#[test]
fn test_update_command() {
    // Test the full update workflow
}
```

### Test Fixtures

Use test fixtures in `src/vcs_sources/tests.rs` for consistent test data:

```rust
pub fn create_test_metadata() -> PackageMetadata {
    PackageMetadata {
        version: "1.0.0".to_owned(),
        // ...
    }
}
```

## Documentation

### Code Documentation

Use rustdoc comments:

```rust
/// Updates a package to the latest version.
///
/// # Arguments
///
/// * `package` - The package metadata
/// * `strategy` - The semver strategy to use
///
/// # Returns
///
/// The updated package metadata
///
/// # Errors
///
/// Returns an error if the update fails
pub async fn update_package(
    package: &PackageMetadata,
    strategy: SemverStrategy,
) -> anyhow::Result<PackageMetadata> {
    // Implementation
}
```

Generate docs:

```bash
cargo doc --open
```

### User Documentation

Update mdBook documentation in `book/src/`:

```bash
cd book
mdbook serve
# Open http://localhost:3000
```

## Release Process

1. **Update version** in `Cargo.toml`

2. **Update CHANGELOG.md** with new version and changes

3. **Run tests** and ensure all pass

4. **Create release commit:**

```bash
git commit -am "Release v0.2.0"
git tag v0.2.0
git push && git push --tags
```

5. **GitHub Actions** will automatically:
   - Build binaries
   - Create GitHub release
   - Publish to crates.io (if configured)

## Common Development Tasks

### Update Dependencies

```bash
# Update Cargo.lock
cargo update

# Update specific dependency
cargo update -p tokio

# Check for outdated dependencies
cargo outdated
```

### Check Build Times

```bash
cargo clean
cargo build --timings
# Opens HTML report showing build times
```

### Generate Test Coverage

```bash
# Install tarpaulin
cargo install cargo-tarpaulin

# Generate coverage
cargo tarpaulin --out Html
# Opens HTML coverage report
```

## Troubleshooting

### Build Failures

```bash
# Clean and rebuild
cargo clean
cargo build

# Update dependencies
cargo update
```

### Test Failures

```bash
# Run specific test with output
cargo test test_name -- --nocapture

# Run with backtrace
RUST_BACKTRACE=1 cargo test
```

### Nix Issues

```bash
# Rebuild development shell
nix develop --rebuild

# Or without flakes
nix-shell --run "cargo build"
```

## Getting Help

- **Issues**: Open an issue on GitHub
- **Discussions**: Use GitHub Discussions for questions
- **IRC**: `#ekala-project` on Libera.Chat (if applicable)
- **Documentation**: See [Architecture](./architecture.md) for code structure

## Code of Conduct

Be respectful, inclusive, and constructive. See the project's CODE_OF_CONDUCT.md for details.

## See Also

- [Architecture](./architecture.md) - Code structure and design
- [Quick Start](../quick-start.md) - User-facing documentation
- [CLI Reference](../cli-reference.md) - Command documentation
