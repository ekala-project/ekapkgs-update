# Builds the packages from nix/overlay.nix on top of upstream nixpkgs
# (rather than ekapkgs), the way the NixOS module consumes them. Used by CI:
#
#   nix build --impure -f nix/nixpkgs-compat.nix
{
  nixpkgs ? builtins.getFlake "github:NixOS/nixpkgs/nixos-unstable",
  system ? builtins.currentSystem,
}:

let
  pkgs = import nixpkgs {
    inherit system;
    overlays = [ (import ./overlay.nix) ];
  };
in
{
  inherit (pkgs) ekapkgs-update ekapkgs-update-web;
}
