final: prev:
with final;
let
  # ekapkgs exposes `elm` at the top level; nixpkgs only as elmPackages.elm.
  prevElm = prev.elm or prev.elmPackages.elm;
in
{
  dev-shell = callPackage ./dev-shell.nix { };

  # Not packaged in ekapkgs; used by the dev shell and `nix fmt`.
  elm-format = callPackage ./elm-format.nix { };

  ekapkgs-update = callPackage ./package.nix { };

  ekapkgs-update-web-frontend = callPackage ./package-web-frontend.nix { };

  ekapkgs-update-web = callPackage ./package-web.nix { };

  # ekapkgs packages elm as the upstream release binary but only pins the
  # x86_64-linux one. The same official release (github.com/elm/compiler,
  # 0.19.2) also ships a statically linked aarch64-linux binary; use it where
  # ekapkgs' elm is unavailable. elm.js output is platform-independent.
  # Tied to 0.19.2 so an ekapkgs bump falls back to its own definition.
  elm =
    if
      stdenv.hostPlatform.system == "aarch64-linux"
      && prevElm.version == "0.19.2"
      && !(lib.meta.availableOn stdenv.hostPlatform prevElm)
    then
      prevElm.overrideAttrs (old: {
        src = fetchurl {
          url = "https://github.com/elm/compiler/releases/download/0.19.2/elm-0.19.2-linux-arm.gz";
          hash = "sha256-orsj6K+MmHTtH3AP26TSVjZpkxPf8EaVWYxBBVAoyPU=";
        };
        meta = old.meta // {
          platforms = old.meta.platforms ++ [ "aarch64-linux" ];
        };
      })
    else
      prevElm;
}
