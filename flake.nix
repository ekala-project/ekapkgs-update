{
  description = "EkaCI flake";

  inputs.ekapkgs.url = "github:ekala-project/ekapkgs";

  outputs =
    { self, ekapkgs }:
    let
      localOverlay = import ./nix/overlay.nix;
    in
    ekapkgs.lib.mkFlake {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      overlays = [ localOverlay ];

      packages = pkgs: {
        default = pkgs.ekapkgs-update;
        inherit (pkgs) ekapkgs-update ekapkgs-update-web;
      };

      devShells = pkgs: {
        default = pkgs.dev-shell;
      };

      treefmt = {
        programs.rustfmt.enable = true;
        programs.nixfmt.enable = true;
      };
    }
    // {
      overlays.default = localOverlay;

      nixosModules.default = import ./nix/module.nix;
      nixosModules.ekapkgs-update = import ./nix/module.nix;
    };
}
