{
  fetchurl,
  lib,
  stdenv,
  cmake,
}:

stdenv.mkDerivation rec {
  pname = "cmocka";
  version = "1.1.8";

  src = fetchurl {
    url = "https://cmocka.org/files/${lib.versions.majorMinor version}/cmocka-${version}.tar.xz";
    hash = "sha256-WENbVYdm1/THKboWO9867Di+07x2batoTjUm7Qqnx4A=";
  };

  patches = [
    ./uintptr_t.patch
  ];

  nativeBuildInputs = [
    cmake
    cmake.configurePhaseHook
  ];

  cmakeFlags =
    lib.optional doCheck "-DUNIT_TESTING=ON"
    ++ lib.optional stdenv.hostPlatform.isStatic "-DBUILD_SHARED_LIBS=OFF";

  doCheck = true;
}
