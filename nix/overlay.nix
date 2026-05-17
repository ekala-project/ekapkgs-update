final: prev: with final; {
  dev-shell = callPackage ./dev-shell.nix { };

  ekapkgs-update = callPackage ./package.nix { };

  ekapkgs-update-web = callPackage ./package-web.nix { };
}
