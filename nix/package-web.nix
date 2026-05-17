{
  lib,
  rustPlatform,
  pkg-config,
  openssl,
  perl,
}:

rustPlatform.buildRustPackage {
  pname = "ekapkgs-update-web";
  version =
    let
      cargo_toml = builtins.readFile ../Cargo.toml;
      cargo_info = builtins.fromTOML cargo_toml;
    in
    cargo_info.workspace.package.version or "0.1.0";

  cargoLock.lockFile = ../Cargo.lock;
  src = ../.;

  # Build only the web portal binary from the workspace
  cargoBuildFlags = [
    "-p"
    "ekapkgs-update-web"
  ];
  cargoTestFlags = [
    "-p"
    "ekapkgs-update-web"
  ];

  nativeBuildInputs = [
    perl
    pkg-config
  ];

  buildInputs = [
    openssl
  ];

  # This causes the build to occur again, but in debug mode
  doCheck = false;

  # Copy static assets to the output
  postInstall = ''
    mkdir -p $out/share/ekapkgs-update-web
    cp -r ekapkgs-update-web/templates $out/share/ekapkgs-update-web/
    cp -r ekapkgs-update-web/static $out/share/ekapkgs-update-web/
  '';

  meta = with lib; {
    description = "Web monitoring portal for ekapkgs-update";
    mainProgram = "ekapkgs-update-web";
  };
}
