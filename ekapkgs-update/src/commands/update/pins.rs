//! Component pins read from a file of the package's own (new) source.
//!
//! Upstreams usually pin what they bundle: radare2's `subprojects/sdb.wrap`,
//! kapacitor's `go.mod` (flux), codex's `Cargo.lock` (`v8`, which decides the
//! `librusty_v8` archive), moby's `Dockerfile` (`ARG RUNC_VERSION=…`). A
//! component with `pinned-by` follows that pin exactly instead of picking a
//! release:
//!
//! ```nix
//! sdb = {
//!   src = "sdb";
//!   pinned-by.file = "subprojects/sdb.wrap";
//! };
//! librusty_v8 = {
//!   src = "RUSTY_V8_ARCHIVE";
//!   pinned-by = { file = "codex-rs/Cargo.lock"; name = "v8"; };
//! };
//! runc = {
//!   src = "docker-runc.src";
//!   pinned-by = { source = "moby-src"; file = "Dockerfile"; regex = "ARG RUNC_VERSION=(.+)"; };
//! };
//! ```
//!
//! Built-in formats are tried first (`format`, or inferred from the file
//! name), then `regex`, whose first capture group is the pin.

use std::path::Path;

use anyhow::Context;
use regex::Regex;
use serde::Deserialize;

/// Where a component's pin is read from.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) struct PinnedBy {
    /// Attr path, relative to the package, of the source holding `file`
    /// (default: `src`)
    source: Option<String>,
    /// Path of the pin file inside that source
    pub file: String,
    format: Option<PinFormat>,
    /// Entry to look up: the crate (`Cargo.lock`) or module (`go.mod`)
    name: Option<String>,
    /// Fallback: the first capture group is the pin
    regex: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum PinFormat {
    /// A meson subproject (`[wrap-git]` revision, `[wrap-file]` version)
    MesonWrap,
    /// A crate's version in `Cargo.lock`
    CargoLock,
    /// A module's version in `go.mod`
    GoMod,
}

impl PinnedBy {
    /// Attr path of the source holding the pin file, relative to the package.
    pub fn source(&self) -> &str {
        self.source.as_deref().unwrap_or("src")
    }

    /// The explicit format, or one inferred from the file name.
    fn format(&self) -> Option<PinFormat> {
        self.format.or_else(|| {
            let name = Path::new(&self.file).file_name()?.to_str()?;
            match name {
                "Cargo.lock" => Some(PinFormat::CargoLock),
                "go.mod" => Some(PinFormat::GoMod),
                _ if name.ends_with(".wrap") => Some(PinFormat::MesonWrap),
                _ => None,
            }
        })
    }

    /// The pin (a version, tag or commit) in `content`, the pin file's text.
    pub fn extract(&self, content: &str) -> anyhow::Result<String> {
        let parsed = self
            .format()
            .map(|format| self.parse(format, content))
            .transpose();
        match (parsed, &self.regex) {
            (Ok(Some(pin)), _) => Ok(pin),
            (Ok(None), Some(regex)) => extract_with_regex(content, regex),
            (Ok(None), None) => anyhow::bail!(
                "{} has no known format; set pinned-by.format or pinned-by.regex",
                self.file
            ),
            (Err(e), Some(regex)) => extract_with_regex(content, regex)
                .with_context(|| format!("{e:#}; regex fallback also failed")),
            (Err(e), None) => Err(e),
        }
    }

    fn parse(&self, format: PinFormat, content: &str) -> anyhow::Result<String> {
        let name = || {
            self.name
                .as_deref()
                .with_context(|| format!("pinned-by.name is required for {}", self.file))
        };
        match format {
            PinFormat::MesonWrap => meson_wrap_pin(content),
            PinFormat::CargoLock => cargo_lock_version(content, name()?),
            PinFormat::GoMod => go_mod_version(content, name()?),
        }
        .with_context(|| format!("read the pin from {}", self.file))
    }
}

fn extract_with_regex(content: &str, regex: &str) -> anyhow::Result<String> {
    let re =
        Regex::new(&format!("(?m){regex}")).with_context(|| format!("invalid regex {regex}"))?;
    let caps = re
        .captures(content)
        .with_context(|| format!("regex {regex} does not match"))?;
    let pin = caps
        .get(1)
        .with_context(|| format!("regex {regex} has no capture group"))?
        .as_str()
        .trim();
    anyhow::ensure!(!pin.is_empty(), "regex {regex} captured an empty pin");
    Ok(pin.to_owned())
}

/// The pin of a meson wrap file: a `[wrap-git]` `revision` (tag or commit),
/// else a `[wrap-file]`'s version: `wrapdb_version` without its `-N` patch
/// revision, or the archive name of `source_url`.
fn meson_wrap_pin(content: &str) -> anyhow::Result<String> {
    let value = |key: &str| {
        content.lines().find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == key)
                .then(|| v.trim())
                .filter(|v| !v.is_empty())
        })
    };
    if let Some(revision) = value("revision") {
        anyhow::ensure!(
            !revision.eq_ignore_ascii_case("head"),
            "the wrap follows the branch head, not a pin"
        );
        return Ok(revision.to_owned());
    }
    if let Some(version) = value("wrapdb_version") {
        let version = version
            .rsplit_once('-')
            .filter(|(_, rev)| rev.chars().all(|c| c.is_ascii_digit()))
            .map_or(version, |(version, _)| version);
        return Ok(version.to_owned());
    }
    let url =
        value("source_url").context("the wrap has no revision, wrapdb_version or source_url")?;
    let archive = url.rsplit('/').next().unwrap_or(url);
    [".tar.gz", ".tar.xz", ".tar.bz2", ".tgz", ".zip"]
        .iter()
        .find_map(|ext| archive.strip_suffix(ext))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("no version in source_url {url}"))
}

/// The version `Cargo.lock` locks crate `name` at; several versions of it are
/// ambiguous.
fn cargo_lock_version(content: &str, name: &str) -> anyhow::Result<String> {
    let mut versions = Vec::new();
    for package in content.split("[[package]]").skip(1) {
        let field = |key: &str| {
            package.lines().find_map(|line| {
                let (k, v) = line.split_once('=')?;
                (k.trim() == key).then(|| v.trim().trim_matches('"'))
            })
        };
        if field("name") == Some(name) {
            if let Some(version) = field("version") {
                versions.push(version.to_owned());
            }
        }
    }
    match versions.as_slice() {
        [version] => Ok(version.clone()),
        [] => anyhow::bail!("crate {name} is not in Cargo.lock"),
        _ => anyhow::bail!("Cargo.lock has several versions of {name}: {versions:?}"),
    }
}

/// The version `go.mod` requires module `name` at (single-line or block
/// `require`).
fn go_mod_version(content: &str, name: &str) -> anyhow::Result<String> {
    content
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default().trim())
        .map(|line| line.strip_prefix("require ").map_or(line, str::trim))
        .find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next() == Some(name)).then(|| words.next()).flatten()
        })
        .map(str::to_owned)
        .with_context(|| format!("module {name} is not required in go.mod"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned(file: &str, name: Option<&str>, regex: Option<&str>) -> PinnedBy {
        PinnedBy {
            source: None,
            file: file.to_owned(),
            format: None,
            name: name.map(str::to_owned),
            regex: regex.map(str::to_owned),
        }
    }

    const SDB_WRAP: &str = "[wrap-git]\nurl = https://github.com/radareorg/sdb.git\n\
                            revision = 2.6.2\ndirectory = sdb\n";
    const QJS_WRAP: &str = "[wrap-git]\nurl = https://github.com/quickjs-ng/quickjs.git\n\
                            revision = b51e5278439b2fa0510fbb1f3b3dff9f0064d5d7\n";
    const IMPLOT_WRAP: &str = "[wrap-file]\ndirectory = implot-0.16\n\
        source_url = https://github.com/epezent/implot/archive/refs/tags/v0.16.zip\n\
        patch_url = https://wrapdb.mesonbuild.com/v2/implot_0.16-1/get_patch\n\
        wrapdb_version = 0.16-1\n";
    const IMGUI_WRAP: &str = "[wrap-file]\ndirectory = imgui-1.91.6\n\
        source_url = https://github.com/ocornut/imgui/archive/refs/tags/v1.91.6.tar.gz\n\
        source_filename = imgui-1.91.6.tar.gz\n";
    const CARGO_LOCK: &str = "version = 4\n\n[[package]]\nname = \"v8\"\nversion = \"152.2.0\"\n\
        source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n\
        [[package]]\nname = \"v8-sys\"\nversion = \"1.0.0\"\n\n\
        [[package]]\nname = \"syn\"\nversion = \"1.0.109\"\n\n\
        [[package]]\nname = \"syn\"\nversion = \"2.0.100\"\n";
    const GO_MOD: &str = "module github.com/influxdata/kapacitor\n\ngo 1.26.10\n\n\
        require github.com/BurntSushi/toml v1.4.0\n\nrequire (\n\
        \tgithub.com/influxdata/flux v0.201.0\n\
        \tgithub.com/influxdata/fluxx v9.9.9 // indirect\n)\n";
    const DOCKERFILE: &str = "ARG GO_VERSION=1.26\nARG CONTAINERD_VERSION=v2.4.2\n\
                              ARG RUNC_VERSION=v1.5.3\n";

    #[test]
    fn extract_pins() {
        let cases = [
            (
                pinned("subprojects/sdb.wrap", None, None),
                SDB_WRAP,
                "2.6.2",
            ),
            (
                pinned("subprojects/qjs.wrap", None, None),
                QJS_WRAP,
                "b51e5278439b2fa0510fbb1f3b3dff9f0064d5d7",
            ),
            (
                pinned("subprojects/implot.wrap", None, None),
                IMPLOT_WRAP,
                "0.16",
            ),
            (
                pinned("subprojects/imgui.wrap", None, None),
                IMGUI_WRAP,
                "v1.91.6",
            ),
            (
                pinned("codex-rs/Cargo.lock", Some("v8"), None),
                CARGO_LOCK,
                "152.2.0",
            ),
            (
                pinned("go.mod", Some("github.com/influxdata/flux"), None),
                GO_MOD,
                "v0.201.0",
            ),
            (
                pinned("go.mod", Some("github.com/BurntSushi/toml"), None),
                GO_MOD,
                "v1.4.0",
            ),
            (
                pinned("Dockerfile", None, Some("ARG RUNC_VERSION=(.+)")),
                DOCKERFILE,
                "v1.5.3",
            ),
            // The regex is the fallback when the format's lookup fails.
            (
                pinned(
                    "Cargo.lock",
                    Some("rusty_v8"),
                    Some(r#"name = "v8"\nversion = "(.*)""#),
                ),
                CARGO_LOCK,
                "152.2.0",
            ),
        ];
        for (pinned_by, content, expected) in cases {
            assert_eq!(
                pinned_by.extract(content).unwrap(),
                expected,
                "{}",
                pinned_by.file
            );
        }
    }

    #[test]
    fn extract_pin_errors() {
        let cases = [
            // ambiguous crate
            (pinned("Cargo.lock", Some("syn"), None), CARGO_LOCK),
            // missing crate / module
            (pinned("Cargo.lock", Some("serde"), None), CARGO_LOCK),
            (
                pinned("go.mod", Some("github.com/influxdata"), None),
                GO_MOD,
            ),
            // name required
            (pinned("Cargo.lock", None, None), CARGO_LOCK),
            // unknown format without regex
            (pinned("Dockerfile", None, None), DOCKERFILE),
            // regex that doesn't match
            (
                pinned("Dockerfile", None, Some("ARG TINI_VERSION=(.+)")),
                DOCKERFILE,
            ),
            // wrap following a branch
            (
                pinned("x.wrap", None, None),
                "[wrap-git]\nrevision = HEAD\n",
            ),
        ];
        for (pinned_by, content) in cases {
            assert!(pinned_by.extract(content).is_err(), "{pinned_by:?}");
        }
    }

    #[test]
    fn pinned_by_from_json() {
        let json = r#"{"source": "moby-src", "file": "Dockerfile", "format": "go-mod",
                       "name": "x", "regex": "(.*)"}"#;
        let pinned_by: PinnedBy = serde_json::from_str(json).unwrap();
        assert_eq!(pinned_by.source(), "moby-src");
        assert_eq!(pinned_by.format(), Some(PinFormat::GoMod));
        let default: PinnedBy = serde_json::from_str(r#"{"file": "a/b.wrap"}"#).unwrap();
        assert_eq!(default.source(), "src");
        assert_eq!(default.format(), Some(PinFormat::MesonWrap));
    }
}
