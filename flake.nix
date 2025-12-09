{
  description = "EkaCI flake";

  inputs = {
    utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    treefmt-nix.url = "github:numtide/treefmt-nix";
  };

  outputs =
    {
      self,
      nixpkgs,
      utils,
      treefmt-nix,
    }:
    let
      localOverlay = import ./nix/overlay.nix;
    in
    utils.lib.eachDefaultSystem (system: rec {
      legacyPackages = import nixpkgs {
        inherit system;
        overlays = [
          localOverlay
        ];
      };

      packages.default = legacyPackages.ekapkgs-update;
      devShells.default = legacyPackages.dev-shell;
      formatter =
        let
          fmt = treefmt-nix.lib.evalModule legacyPackages {
            programs.rustfmt.enable = true;
            programs.nixfmt.enable = true;
          };
        in
        fmt.config.build.wrapper;
    })
    // {
      overlays.default = localOverlay;
    };
}
