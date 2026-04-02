{
  stdenv,
  fenix,
  pkg-config,
  openssl,
  sqlite,
  nix-eval-jobs,
}:

stdenv.mkDerivation {
  name = "dev";

  nativeBuildInputs = [
    nix-eval-jobs
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
