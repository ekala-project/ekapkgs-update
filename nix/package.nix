{
  lib,
  rustPlatform,
  pkg-config,
  openssl,
  perl,
  makeWrapper,
  cachix,
  nix,
  git,
  gnutar,
  xz,
  gh,
  nix-eval-jobs,
  coreutils,
}:

rustPlatform.buildRustPackage {
  pname = "ekapkgs-update";
  version =
    let
      cargo_toml = builtins.readFile ../ekapkgs-update/Cargo.toml;
      cargo_info = builtins.fromTOML cargo_toml;
    in
    cargo_info.package.version;

  cargoLock.lockFile = ../Cargo.lock;
  src = ../.;

  # Build only the main binary from the workspace
  cargoBuildFlags = [
    "-p"
    "ekapkgs-update"
  ];
  cargoTestFlags = [
    "-p"
    "ekapkgs-update"
  ];

  nativeBuildInputs = [
    perl
    pkg-config
    makeWrapper
  ];

  buildInputs = [
    openssl
  ];

  # This causes the build to occur again, but in debug mode
  doCheck = false;

  # Wrap the binary so that the runtime tools it shells out to (`nix`,
  # `nix-build`, `git`, `gh`, `cachix`, etc.) are guaranteed to be on PATH
  # when the daemon is launched from systemd. Without this the service
  # would inherit only the system PATH and fail at the first subprocess.
  postFixup = ''
    wrapProgram $out/bin/ekapkgs-update \
      --prefix PATH : ${
        lib.makeBinPath [
          cachix
          nix
          git
          gh
          nix-eval-jobs
          gnutar
          xz
          coreutils
        ]
      }
  '';

  meta = with lib; {
    description = "Automated package update tool for ekapkgs (and similar Nix package sets)";
    mainProgram = "ekapkgs-update";
  };
}
