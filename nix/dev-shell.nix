{
  stdenv,
  cargo,
  clippy,
  rustc,
  rustfmt,
  pkg-config,
  openssl,
  sqlite,
  nix-eval-jobs,
  cachix,
}:

stdenv.mkDerivation {
  name = "dev";

  nativeBuildInputs = [
    nix-eval-jobs
    # cachix  # TODO: ghc-binary-9.8.4 segfaults; re-enable when fixed upstream
    cargo
    clippy
    rustc
    rustfmt
    pkg-config
    sqlite
  ];
  buildInputs = [
    openssl
    sqlite
  ];
}
