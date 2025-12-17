{
  stdenv,
  fenix,
  pkg-config,
  openssl,
  sqlite,
}:

stdenv.mkDerivation {
  name = "dev";

  nativeBuildInputs = [
    (fenix.default.withComponents [
      "cargo"
      "clippy"
      "rust-std"
      "rustc"
      "rustfmt-preview"
    ])
    sqlite
  ];
  buildInputs = [ sqlite ];
}
