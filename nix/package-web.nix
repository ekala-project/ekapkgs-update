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

  # Templates (askama) and static assets (include_str!) are compiled into
  # the binary, so nothing needs to be installed alongside it.

  meta = with lib; {
    description = "Web monitoring portal for ekapkgs-update";
    mainProgram = "ekapkgs-update-web";
  };
}
