# Installation

This chapter provides comprehensive installation instructions for ekapkgs-update across different platforms and use cases.

## Prerequisites

ekapkgs-update requires:

- **Nix package manager** - For package evaluation and updates
- **Git** - For repository operations and version control
- **Rust toolchain** (optional) - Only needed when building from source

## Installation Methods

### Method 1: Nix Package (Recommended)

The simplest way to install ekapkgs-update is through the Nix package manager:

```bash
# Install in current shell
nix-shell -p ekapkgs-update

# Or add to your system configuration
environment.systemPackages = [ pkgs.ekapkgs-update ];
```

For Nix flakes users:

```bash
# Run directly
nix run github:ekala-project/ekapkgs-update -- --help

# Install to profile
nix profile install github:ekala-project/ekapkgs-update
```

### Method 2: From Source

For development or when you need the latest unreleased features:

```bash
# Clone the repository
git clone https://github.com/ekala-project/ekapkgs-update
cd ekapkgs-update

# Enter development environment (includes all dependencies)
nix develop

# Build release binary
cargo build --release

# The binary will be at:
# target/release/ekapkgs-update
```

To install the built binary system-wide:

```bash
# Copy to a directory in PATH
sudo cp target/release/ekapkgs-update /usr/local/bin/

# Or use cargo install (installs to ~/.cargo/bin)
cargo install --path .
```

### Method 3: Direct from Cargo

If you have the Rust toolchain installed:

```bash
cargo install ekapkgs-update
```

**Note:** This method requires Nix to be available at runtime for package evaluation.

## Verification

After installation, verify that ekapkgs-update is working correctly:

```bash
# Check version
ekapkgs-update --version

# Display help
ekapkgs-update --help

# Test basic functionality
ekapkgs-update update --help
```

Expected output should show version information and available commands.

## Configuration

### API Tokens (Recommended)

For reliable operation and to avoid rate limits, configure API tokens:

```bash
# GitHub token (for GitHub releases and repositories)
export GITHUB_TOKEN="ghp_your_token_here"

# GitLab token (for GitLab repositories)
export GITLAB_TOKEN="glpat-your_token_here"
```

Add these to your shell profile (`~/.bashrc`, `~/.zshrc`, etc.) for persistence.

**Creating tokens:**

- **GitHub**: Settings → Developer settings → Personal access tokens → Generate new token
  - Required scopes: `public_repo` (for public repositories)
- **GitLab**: User Settings → Access Tokens → Add new token
  - Required scopes: `read_api`, `read_repository`

### Logging

Control logging verbosity with the `RUST_LOG` environment variable:

```bash
# Debug output (verbose, useful for troubleshooting)
export RUST_LOG=debug

# Info output (normal verbosity)
export RUST_LOG=info

# Warning and error only (minimal output)
export RUST_LOG=warn
```

### Cachix (Optional)

If using Cachix for build caching:

```bash
export CACHIX_CACHE_NAME="your-cache-name"
export CACHIX_AUTH_TOKEN="your-auth-token"
```

## Platform-Specific Notes

### NixOS

Add to your system configuration:

```nix
# /etc/nixos/configuration.nix
{
  environment.systemPackages = with pkgs; [
    ekapkgs-update
  ];

  # Optional: Add API tokens to environment
  environment.variables = {
    GITHUB_TOKEN = "ghp_...";  # Better to use secrets management
  };
}
```

**Note:** For sensitive tokens, use `sops-nix`, `agenix`, or other secrets management solutions instead of hardcoding.

### macOS

Install Nix first if not already available:

```bash
# Multi-user installation (recommended)
sh <(curl -L https://nixos.org/nix/install) --daemon
```

Then follow the standard Nix package installation method above.

### Linux (Non-NixOS)

Ensure Nix is installed:

```bash
# Multi-user installation
sh <(curl -L https://nixos.org/nix/install) --daemon

# Or single-user installation
sh <(curl -L https://nixos.org/nix/install) --no-daemon
```

Then install ekapkgs-update via Nix as described above.

## Docker / Container Usage

Run ekapkgs-update in a containerized environment:

```dockerfile
FROM nixos/nix:latest

# Install ekapkgs-update
RUN nix-env -iA nixpkgs.ekapkgs-update

# Set environment variables
ENV GITHUB_TOKEN=""
ENV RUST_LOG="info"

# Run as non-root user
RUN adduser -D updater
USER updater

ENTRYPOINT ["ekapkgs-update"]
CMD ["--help"]
```

Build and run:

```bash
docker build -t ekapkgs-update .
docker run -e GITHUB_TOKEN="$GITHUB_TOKEN" ekapkgs-update update --help
```

## CI/CD Integration

### GitHub Actions

```yaml
name: Update Packages

on:
  schedule:
    - cron: '0 0 * * *'  # Daily at midnight
  workflow_dispatch:

jobs:
  update:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - uses: cachix/install-nix-action@v20
        with:
          nix_path: nixpkgs=channel:nixos-unstable

      - name: Install ekapkgs-update
        run: nix-env -iA nixpkgs.ekapkgs-update

      - name: Run updates
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: ekapkgs-update run --dry-run --file ./default.nix
```

### GitLab CI

```yaml
update-packages:
  image: nixos/nix:latest
  script:
    - nix-env -iA nixpkgs.ekapkgs-update
    - ekapkgs-update run --dry-run --file ./default.nix
  variables:
    GITHUB_TOKEN: $GITHUB_TOKEN
  only:
    - schedules
```

## Troubleshooting

### Command Not Found

If `ekapkgs-update` is not found after installation:

```bash
# Check if binary exists
which ekapkgs-update

# For cargo install, ensure ~/.cargo/bin is in PATH
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc

# For nix profile, ensure profile bin directory is in PATH
# Usually ~/.nix-profile/bin should already be in PATH
```

### Permission Denied

If you encounter permission errors when building from source:

```bash
# Ensure you have write access to the project directory
chmod -R u+w ekapkgs-update/

# For cargo install, ensure ~/.cargo exists and is writable
mkdir -p ~/.cargo
chmod u+w ~/.cargo
```

### Nix Evaluation Errors

If ekapkgs-update fails to evaluate Nix expressions:

```bash
# Ensure Nix is properly installed
nix --version

# Test Nix evaluation manually
nix-instantiate --eval -E '1 + 1'

# Check Nix daemon is running (multi-user installation)
systemctl status nix-daemon
```

## Next Steps

Now that ekapkgs-update is installed, proceed to:

- [Quick Start](./quick-start.md) - Learn basic usage and common workflows
- [CLI Reference](./cli-reference.md) - Explore all available commands and options
- [Passthru Attributes](./passthru-attributes.md) - Configure per-package update behavior
