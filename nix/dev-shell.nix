{
  stdenv,
  cargo,
  clippy,
  rustc,
  rustfmt,
  pkg-config,
  openssl,
  sqlite,
  iets,
  cachix,
}:

stdenv.mkDerivation {
  name = "dev";

  nativeBuildInputs = [
    iets
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
