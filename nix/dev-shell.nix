{
  stdenv,
  fenix,
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
    cachix
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
