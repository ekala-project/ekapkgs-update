# Runs the Elm unit tests (ekapkgs-update-web/frontend/tests) in the
# sandbox. It reuses the frontend build's offline $ELM_HOME, which also
# holds the pinned test-dependencies, and runs test-runner/run.sh: elm
# compiles a generated test Main and QuickJS executes it.
{
  lib,
  ekapkgs-update-web-frontend,
  jq,
  quickjs,
}:

ekapkgs-update-web-frontend.overrideAttrs (old: {
  pname = "ekapkgs-update-web-frontend-tests";

  src = lib.fileset.toSource {
    root = ../ekapkgs-update-web/frontend;
    fileset = lib.fileset.unions [
      ../ekapkgs-update-web/frontend/elm.json
      ../ekapkgs-update-web/frontend/src
      ../ekapkgs-update-web/frontend/tests
      ../ekapkgs-update-web/frontend/test-runner
    ];
  };

  nativeBuildInputs = old.nativeBuildInputs ++ [
    jq
    quickjs
  ];

  postPatch = ''
    patchShebangs test-runner/run.sh
  '';

  buildPhase = ''
    runHook preBuild
    test-runner/run.sh >test-results.txt || { cat test-results.txt; exit 1; }
    cat test-results.txt
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    install -Dm644 test-results.txt "$out/test-results.txt"
    runHook postInstall
  '';

  meta = old.meta // {
    description = "Elm unit tests of the ekapkgs-update web dashboard";
  };
})
