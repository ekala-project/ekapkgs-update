{
  description = "Ekapkgs-update flake, a tool for updating and curating packages";

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

      overlays.default = localOverlay;

      packages = pkgs: {
        default = pkgs.ekapkgs-update;
        inherit (pkgs) ekapkgs-update ekapkgs-update-web;
      };

      ekaShell = { pkgs, ... }: {
        modules = [
          {
            languages.rust.enable = true;
          }
        ];
        packages = [
          pkgs.pkg-config
          pkgs.sqlite
          pkgs.nix-eval-jobs
          # pkgs.cachix  # TODO: ghc-binary-9.8.4 segfaults; re-enable when fixed upstream
        ];
        buildInputs = [
          pkgs.openssl
          pkgs.sqlite
        ];
      };

      treefmt = {
        programs.rustfmt.enable = true;
        programs.nixfmt.enable = true;
      };

      nixosModules = {
        default = import ./nix/module.nix;
        ekapkgs-update = import ./nix/module.nix;
      };
    };
}
