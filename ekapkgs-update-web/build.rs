//! Compiles the Elm frontend (`frontend/`) into `$OUT_DIR/elm.js`, which
//! `src/assets.rs` embeds with `include_str!`.
//!
//! When `EKAPKGS_WEB_ELM_JS` names an already compiled file (the Nix build
//! compiles the frontend in its own derivation, see
//! `nix/package-web-frontend.nix`), it is copied instead and `elm` is not
//! needed. Otherwise `elm make --optimize` runs; `elm` comes from the dev
//! shell (`nix develop`).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const PREBUILT_ENV: &str = "EKAPKGS_WEB_ELM_JS";

fn main() {
    let out =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo")).join("elm.js");
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let frontend = manifest.join("frontend");

    println!("cargo:rerun-if-env-changed={PREBUILT_ENV}");
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
        Ok(s) if s.success() => {},
        Ok(s) => fail(&format!(
            "`elm make` failed ({s}); see the compiler output above"
        )),
        Err(e) => fail(&format!(
            "could not run `elm` ({e}). Build inside `nix develop`, which provides it, or set \
             {PREBUILT_ENV} to a compiled elm.js"
        )),
    }
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
