{
  stdenv,
  cargo,
  rustfmt,
}:

stdenv.mkDerivation {
  name = "dev";

  nativeBuildInputs = [
    rustfmt
    cargo
  ];
}
