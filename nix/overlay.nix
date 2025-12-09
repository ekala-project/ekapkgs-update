final: prev: with final; {
  dev-shell = callPackage ./dev-shell.nix { };

  ekapkgs-update = callPackage ./package.nix { };
}
