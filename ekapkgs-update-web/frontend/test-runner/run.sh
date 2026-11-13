#!/usr/bin/env bash
# Run the Elm unit tests in ekapkgs-update-web/frontend/tests.
#
# Every `tests/**/*Test.elm` module must expose `suite : Test`. This
# script builds an application elm.json in elm-stuff/tests with the
# test-dependencies promoted to dependencies, generates a TestMain that runs
# every suite, compiles it with `elm make` and runs the result with
# QuickJS. It needs elm, jq and qjs (all in the dev shell) and no network
# once the packages are in $ELM_HOME; nix/web-frontend-tests.nix runs it
# in the sandbox.
#
# Usage: run.sh [seed]   (default: $ELM_TEST_SEED or 20261112)
set -euo pipefail

frontend="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
build="$frontend/elm-stuff/tests"
seed="${1:-${ELM_TEST_SEED:-20261112}}"
mkdir -p "$build"

jq '
  ."test-dependencies" as $t
  | ."source-directories" |= map("../../" + .) + ["../../tests", "../../test-runner", "."]
  | .dependencies.direct += $t.direct
  | .dependencies.indirect = ((.dependencies.indirect + $t.indirect)
      | with_entries(select(.key as $k | $t.direct | has($k) | not)))
  | ."test-dependencies" = { direct: {}, indirect: {} }
' "$frontend/elm.json" >"$build/elm.json"

modules=()
while IFS= read -r file; do
  module="${file#./}"
  module="${module%.elm}"
  modules+=("${module//\//.}")
done < <(cd "$frontend/tests" && find . -name '*Test.elm' | LC_ALL=C sort)

if [ "${#modules[@]}" -eq 0 ]; then
  echo "No *Test.elm modules in $frontend/tests" >&2
  exit 1
fi

{
  echo "module TestMain exposing (main)"
  echo
  echo "import TestRunner"
  printf 'import %s\n' "${modules[@]}"
  echo
  echo
  echo "main ="
  echo "    TestRunner.program"
  sep="["
  for module in "${modules[@]}"; do
    echo "        $sep $module.suite"
    sep=","
  done
  echo "        ]"
} >"$build/TestMain.elm.new"
# Only replace TestMain.elm when it changes, so elm can reuse its cache.
if cmp -s "$build/TestMain.elm.new" "$build/TestMain.elm"; then
  rm "$build/TestMain.elm.new"
else
  mv "$build/TestMain.elm.new" "$build/TestMain.elm"
fi

(cd "$build" && elm make TestMain.elm --output=runner.js >/dev/null)
exec qjs --std "$frontend/test-runner/driver.js" "$build/runner.js" "$seed"
