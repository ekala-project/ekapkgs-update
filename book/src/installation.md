# Installation

> **Note:** This chapter is under construction.

## Using Nix

```bash
nix-shell -p ekapkgs-update
```

## From Source

```bash
git clone https://github.com/ekala-project/ekapkgs-update
cd ekapkgs-update
nix develop
cargo build --release
```

## Verification

```bash
ekapkgs-update --version
```

For detailed usage instructions, see [Quick Start](./quick-start.md).
