# Installation

ekapkgs-update can be installed via Nix flakes, added to your system configuration, or built from source.

## Via Nix Flakes (Recommended)

The easiest way to try ekapkgs-update is using Nix flakes:

```bash
# Run directly without installing
nix run github:ekapkgs/ekapkgs-update -- --help

# Install to your profile
nix profile install github:ekapkgs/ekapkgs-update

# Or add to your flake inputs
{
  inputs.ekapkgs-update.url = "github:ekapkgs/ekapkgs-update";
}
```

## NixOS System Configuration

Add ekapkgs-update to your NixOS configuration:

### Flake-based NixOS

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    ekapkgs-update.url = "github:ekapkgs/ekapkgs-update";
  };

  outputs = { nixpkgs, ekapkgs-update, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        ekapkgs-update.nixosModules.default
        {
          services.ekapkgs-update = {
            enable = true;
            packagesFile = /path/to/packages.nix;
            # Additional configuration...
          };
        }
      ];
    };
  };
}
```

### Classic NixOS Configuration

If using channels or not using flakes:

```nix
{ config, pkgs, ... }:

let
  ekapkgs-update = (import (builtins.fetchTarball {
    url = "https://github.com/ekapkgs/ekapkgs-update/archive/master.tar.gz";
  })).packages.${pkgs.system}.default;
in {
  # Add package to system packages
  environment.systemPackages = [ ekapkgs-update ];

  # Or use as a service (import the module separately)
  imports = [
    (import (builtins.fetchTarball {
      url = "https://github.com/ekapkgs/ekapkgs-update/archive/master.tar.gz";
    }) + "/nix/module.nix")
  ];

  services.ekapkgs-update = {
    enable = true;
    packagesFile = /path/to/packages.nix;
  };
}
```

## Building from Source

Clone and build the repository:

```bash
# Clone the repository
git clone https://github.com/ekapkgs/ekapkgs-update.git
cd ekapkgs-update

# Build with nix
nix build

# Or use cargo if you have Rust toolchain
cargo build --release

# Binary will be at ./target/release/ekapkgs-update
```

## Development Environment

For development, use the provided devShell:

```bash
# Enter development shell
nix develop

# Or with direnv
echo "use flake" > .envrc
direnv allow

# Now you have cargo, rust-analyzer, and all dependencies
cargo build
cargo test
```

## Dependencies

ekapkgs-update requires the following tools at runtime:

- **nix** (2.4 or later, with flakes support recommended)
- **git** (for PR creation and repository management)
- **nix-eval-jobs** (for parallel package evaluation)
- **gh** (GitHub CLI, for PR creation if targeting GitHub)
- **cachix** (optional, for build artifact caching)

When installed via Nix, these dependencies are automatically included in the wrapper.

## Verifying Installation

Check that ekapkgs-update is correctly installed:

```bash
# Check version
ekapkgs-update --help

# Verify dependencies are accessible
which nix git gh nix-eval-jobs

# Test basic functionality
ekapkgs-update update --help
```

## Next Steps

Now that you have ekapkgs-update installed:

- [Quick Start](./quick-start.md) - Update your first package
- [Configuration](./configuration.md) - Set up for your repository
- [NixOS Module](./nixos-module.md) - Deploy as a system service
