//! Compiles the Elm frontend (`frontend/`) into `$OUT_DIR/elm.js`, which
//! `src/assets.rs` embeds with `include_str!`.
//!
//! The file comes from the first of these that applies:
//!
//! 1. `EKAPKGS_WEB_ELM_JS` names an already compiled file. It is copied, and a
//!    missing or unreadable file is a hard error. The Nix build compiles the
//!    frontend in its own derivation (`nix/package-web-frontend.nix`) and
//!    always takes this path.
//! 2. `elm make --optimize` runs; `elm` comes from the dev shell
//!    (`nix develop`). A compile error is a hard error.
//! 3. `elm` is not on `PATH`: a small stub `elm.js` is written instead and a
//!    `cargo:warning` says so. The stub's `Elm.Main.init` renders a
//!    "frontend not built" notice that links to the JSON API, so the crate
//!    (and `cargo test --workspace`) builds without `elm`. Set
//!    `EKAPKGS_WEB_REQUIRE_ELM=1` to make a missing `elm` a hard error.
//!
//! The chosen source is exposed to the crate as
//! `EKAPKGS_WEB_FRONTEND` = `prebuilt` | `elm` | `stub`.

use std::env;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

const PREBUILT_ENV: &str = "EKAPKGS_WEB_ELM_JS";
const REQUIRE_ENV: &str = "EKAPKGS_WEB_REQUIRE_ELM";

/// Served as `elm.js` when `elm` is unavailable. It mimics the one call
/// `static/js/app.js` makes and returns an app without `ports`, which
/// `app.js` tolerates.
const STUB_ELM_JS: &str = r#"// ekapkgs-update-web: frontend not built (stub elm.js written by build.rs).
(function () {
  "use strict";
  function init() {
    var box = document.createElement("main");
    box.setAttribute("style", "max-width:40rem;margin:4rem auto;padding:0 1rem;font-family:system-ui,sans-serif;line-height:1.5");
    var h = document.createElement("h1");
    h.textContent = "ekapkgs-update: frontend not built";
    var p = document.createElement("p");
    p.textContent = "This binary was compiled without the Elm compiler, so the dashboard UI is a placeholder. " +
      "Rebuild with `elm` on PATH (inside `nix develop`) or with EKAPKGS_WEB_ELM_JS set to a compiled elm.js. " +
      "The data is still available from the JSON API: ";
    var a = document.createElement("a");
    a.href = "/api/dashboard";
    a.textContent = "/api/dashboard";
    p.appendChild(a);
    box.appendChild(h);
    box.appendChild(p);
    document.body.appendChild(box);
    return {};
  }
  window.Elm = { Main: { init: init } };
})();
"#;

fn main() {
    let out =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo")).join("elm.js");
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let frontend = manifest.join("frontend");

    println!("cargo:rerun-if-env-changed={PREBUILT_ENV}");
    println!("cargo:rerun-if-env-changed={REQUIRE_ENV}");
    println!("cargo:rerun-if-env-changed=ELM_HOME");

    if let Some(prebuilt) = env::var_os(PREBUILT_ENV).filter(|v| !v.is_empty()) {
        let prebuilt = PathBuf::from(prebuilt);
        println!("cargo:rerun-if-changed={}", prebuilt.display());
        if let Err(e) = std::fs::copy(&prebuilt, &out) {
            fail(&format!(
                "{PREBUILT_ENV}={} could not be copied: {e}",
                prebuilt.display()
            ));
        }
        set_frontend("prebuilt");
        return;
    }

    println!(
        "cargo:rerun-if-changed={}",
        frontend.join("elm.json").display()
    );
    rerun_if_tree_changed(&frontend.join("src"));

    let status = Command::new("elm")
        .current_dir(&frontend)
        .arg("make")
        .arg("src/Main.elm")
        .arg("--optimize")
        .arg(format!("--output={}", out.display()))
        .status();
    match status {
        Ok(s) if s.success() => set_frontend("elm"),
        Ok(s) => fail(&format!(
            "`elm make` failed ({s}); see the compiler output above"
        )),
        Err(e) if e.kind() == ErrorKind::NotFound && !require_elm() => {
            // Re-run once `elm` shows up on PATH (e.g. inside `nix develop`).
            println!("cargo:rerun-if-env-changed=PATH");
            println!(
                "cargo:warning=`elm` not found on PATH; embedding a placeholder dashboard that \
                 only links to /api. Build inside `nix develop` (x86_64-linux) or set \
                 {PREBUILT_ENV} to a compiled elm.js for the real frontend; set \
                 {REQUIRE_ENV}=1 to make this an error."
            );
            if let Err(e) = std::fs::write(&out, STUB_ELM_JS) {
                fail(&format!("could not write {}: {e}", out.display()));
            }
            set_frontend("stub");
        },
        Err(e) => fail(&format!(
            "could not run `elm` ({e}). Build inside `nix develop`, which provides it, or set \
             {PREBUILT_ENV} to a compiled elm.js"
        )),
    }
}

/// `EKAPKGS_WEB_REQUIRE_ELM` is set to anything but empty, `0` or `false`.
fn require_elm() -> bool {
    env::var(REQUIRE_ENV).is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false"))
}

fn set_frontend(kind: &str) {
    println!("cargo:rustc-env=EKAPKGS_WEB_FRONTEND={kind}");
}

fn rerun_if_tree_changed(dir: &Path) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rerun_if_tree_changed(&path);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn fail(message: &str) -> ! {
    eprintln!("error: ekapkgs-update-web frontend: {message}");
    std::process::exit(1);
}
