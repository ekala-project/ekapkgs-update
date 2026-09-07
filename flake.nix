{
  description = "EkaCI flake";

  inputs = {
    ekapkgs.url = "github:ekala-project/ekapkgs";
    treefmt-nix.follows = "ekapkgs/corepkgs/treefmt-nix";
  };

  outputs =
    {
      self,
      ekapkgs,
      treefmt-nix,
    }:
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

      formatter = pkgs:
        let
          fmt = treefmt-nix.lib.evalModule pkgs {
            programs.rustfmt.enable = true;
            programs.rustfmt.package = pkgs.nixfmt-rs;
            programs.nixfmt.enable = true;
          };
        in
        fmt.config.build.wrapper;
    }
    // {
      overlays.default = localOverlay;

      nixosModules.default = import ./nix/module.nix;
      nixosModules.ekapkgs-update = import ./nix/module.nix;
    };
}
