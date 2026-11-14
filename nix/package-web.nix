{
  lib,
  rustPlatform,
  pkg-config,
  openssl,
  sqlite,
  perl,
  ekapkgs-update-web-frontend,
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
    sqlite
  ];

  # This causes the build to occur again, but in debug mode
  doCheck = false;

  # build.rs embeds this pre-compiled Elm bundle instead of running `elm`
  # (which would need network access for its packages). It and the other
  # static assets are compiled into the binary (include_str!), so nothing
  # needs to be installed alongside it.
  env.EKAPKGS_WEB_ELM_JS = "${ekapkgs-update-web-frontend}/elm.js";

  meta = with lib; {
    description = "Web monitoring portal for ekapkgs-update";
    mainProgram = "ekapkgs-update-web";
  };
}
