{
  stdenv,
  fenix,
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
  ];
}
