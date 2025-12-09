{ stdenv,
  cargo,
}:

stdenv.mkDerivation {
  name = "dev";

  nativeBuildInputs = [
    cargo
  ];
}
