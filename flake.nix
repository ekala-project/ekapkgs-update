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

      packages =
        pkgs:
        {
          default = pkgs.ekapkgs-update;
          inherit (pkgs) ekapkgs-update;
        }
        # The dashboard's Elm frontend needs elm, which ekapkgs only packages
        # for x86_64-linux.
        // pkgs.lib.optionalAttrs (pkgs.lib.meta.availableOn pkgs.stdenv.hostPlatform pkgs.elm) {
          inherit (pkgs) ekapkgs-update-web ekapkgs-update-web-frontend;
        };

      # The Elm unit tests, gated like the frontend package.
      checks =
        pkgs:
        pkgs.lib.optionalAttrs (pkgs.lib.meta.availableOn pkgs.stdenv.hostPlatform pkgs.elm) {
          web-frontend-tests = pkgs.callPackage ./nix/web-frontend-tests.nix { };
        };

      devShells = pkgs: {
        default = pkgs.mkDevShell {
          nativeBuildInputs =
            with pkgs;
            [
              cargo
              clippy
              rust-analyzer
              rustc
              rustfmt
              pkg-config
              sqlite
              nix-eval-jobs
              # cachix  # TODO: ghc-binary-9.8.4 segfaults; re-enable when fixed upstream
            ]
            # The web dashboard's Elm frontend (built by ekapkgs-update-web's
            # build.rs) and its test runner (jq, qjs). ekapkgs only packages
            # elm for x86_64-linux.
            ++ lib.optionals (lib.meta.availableOn stdenv.hostPlatform elm) [
              elm
              jq
              quickjs
            ];
          buildInputs = with pkgs; [
            openssl
            sqlite
          ];
        };
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
