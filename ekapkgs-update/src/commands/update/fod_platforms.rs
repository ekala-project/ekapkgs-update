//! Hashes kept per system.
//!
//! Prebuilt artifacts are fetched per platform, with one hash per system:
//!
//! ```nix
//! wheelHashes = {
//!   aarch64-darwin = "sha256-…";
//!   x86_64-linux = "sha256-…";
//!   aarch64-linux = "sha256-…";
//! };
//! ```
//!
//! Hash discovery only sees the build machine's FOD. Once its hash is
//! written, [`refresh_other_systems`] updates the hashes on the neighbouring
//! system-keyed lines: the package is instantiated for each of those systems,
//! the FOD still carrying that system's old hash is found in its closure, and
//! its URL is prefetched (`nix store prefetch-file`). Only plain downloads
//! (flat hash, no `postFetch`) can be prefetched; others are reported.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::Context;
use regex::Regex;
use tracing::{info, warn};

use super::fod_refresh::{closure_drvs, same_hash, show_fods};

/// The system Nix builds for on this machine (`x86_64-linux`).
pub fn native_system() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    format!("{}-{os}", std::env::consts::ARCH)
}

/// `<system> = "<hash>";` on a line of its own.
fn system_line() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"^\s*"?((?:x86_64|i686|aarch64|armv[5-7]l|riscv64|powerpc64le|loongarch64)-(?:linux|darwin|freebsd))"?\s*=\s*"([^"]+)"\s*;\s*$"#,
        )
        .expect("valid regex")
    })
}

/// One `<system> = "<hash>";` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SystemHash {
    /// Zero-based line number
    line: usize,
    pub system: String,
    pub hash: String,
}

/// The system-keyed lines around the line binding `host` to `hash`: the run
/// of consecutive `<system> = "<hash>";` lines it is part of, without the
/// host's own. Empty when `hash` is not bound to `host` that way.
pub(super) fn sibling_systems(content: &str, host: &str, hash: &str) -> Vec<SystemHash> {
    let lines: Vec<Option<SystemHash>> = content
        .lines()
        .enumerate()
        .map(|(line, text)| {
            system_line().captures(text).map(|caps| SystemHash {
                line,
                system: caps[1].to_owned(),
                hash: caps[2].to_owned(),
            })
        })
        .collect();
    let Some(own) = lines
        .iter()
        .flatten()
        .find(|entry| entry.system == host && entry.hash == hash)
        .map(|entry| entry.line)
    else {
        return Vec::new();
    };
    let start = lines[..own]
        .iter()
        .rposition(Option::is_none)
        .map_or(0, |i| i + 1);
    let end = lines[own..]
        .iter()
        .position(Option::is_none)
        .map_or(lines.len(), |i| own + i);
    lines[start..end]
        .iter()
        .flatten()
        .filter(|entry| entry.line != own)
        .cloned()
        .collect()
}

/// Replace the hash on each entry's line.
fn rewrite_lines(content: &str, updates: &[(SystemHash, String)]) -> String {
    let mut out: Vec<String> = content.lines().map(str::to_owned).collect();
    for (entry, new) in updates {
        if let Some(line) = out.get_mut(entry.line) {
            *line = line.replacen(&format!("\"{}\"", entry.hash), &format!("\"{new}\""), 1);
        }
    }
    let mut joined = out.join("\n");
    if content.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// After the build machine's hash in `file` became `host_hash`, update the
/// hashes of the other systems listed next to it. Returns the systems
/// updated; systems whose hash cannot be discovered are warned about and
/// left alone.
pub(super) async fn refresh_other_systems(
    eval_entry_point: &str,
    attr_path: &str,
    file: &Path,
    host_hash: &str,
) -> anyhow::Result<Vec<String>> {
    let content = tokio::fs::read_to_string(file)
        .await
        .with_context(|| format!("read {}", file.display()))?;
    let siblings = sibling_systems(&content, &native_system(), host_hash);
    if siblings.is_empty() {
        return Ok(Vec::new());
    }

    let mut updates = Vec::new();
    for entry in siblings {
        match discover_for_system(eval_entry_point, attr_path, &entry).await {
            Ok(Some(hash)) => {
                info!("{attr_path}: {} hash is now {hash}", entry.system);
                updates.push((entry, hash));
            },
            Ok(None) => {},
            Err(e) => warn!(
                "{attr_path}: the {} hash next to {host_hash} in {} was not updated: {e:#}",
                entry.system,
                file.display()
            ),
        }
    }
    if updates.is_empty() {
        return Ok(Vec::new());
    }

    let content = tokio::fs::read_to_string(file)
        .await
        .with_context(|| format!("read {}", file.display()))?;
    tokio::fs::write(file, rewrite_lines(&content, &updates))
        .await
        .with_context(|| format!("write {}", file.display()))?;
    Ok(updates.into_iter().map(|(entry, _)| entry.system).collect())
}

/// The new hash of the FOD that carries `entry`'s hash when the package is
/// instantiated for `entry.system`, or `None` if it is already up to date.
async fn discover_for_system(
    eval_entry_point: &str,
    attr_path: &str,
    entry: &SystemHash,
) -> anyhow::Result<Option<String>> {
    let drv = instantiate_for_system(eval_entry_point, attr_path, &entry.system).await?;
    let mut drvs: Vec<String> = closure_drvs(&drv).await?.into_iter().collect();
    drvs.sort();
    let fods = show_fods(&drvs).await?;
    let mut matching = fods.iter().filter(|fod| same_hash(&fod.hash, &entry.hash));
    let fod = matching.next().with_context(|| {
        format!(
            "no fixed-output derivation of {attr_path} for {} has hash {}",
            entry.system, entry.hash
        )
    })?;
    anyhow::ensure!(
        matching.all(|other| other.download == fod.download),
        "several fixed-output derivations of {attr_path} for {} have hash {}",
        entry.system,
        entry.hash
    );
    let url = fod.download.as_deref().with_context(|| {
        format!(
            "{} is not a plain download (flat hash, http(s) URL, no postFetch); \
             update its hash by hand",
            fod.name()
        )
    })?;
    let hash = super::hash_workflows::prefetch_url_hash(url)
        .await
        .with_context(|| format!("prefetch {url}"))?;
    Ok((!same_hash(&hash, &entry.hash)).then_some(hash))
}

/// `nix-instantiate --system <system> <entry> -A <attr>`, returning the
/// `.drv` path.
async fn instantiate_for_system(
    eval_entry_point: &str,
    attr_path: &str,
    system: &str,
) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("nix-instantiate")
        .args(["--system", system])
        .arg(eval_entry_point)
        .arg("-A")
        .arg(attr_path)
        .output()
        .await
        .context("Failed to execute nix-instantiate")?;
    if !output.status.success() {
        anyhow::bail!(
            "nix-instantiate --system {system} -A {attr_path} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|l| l.ends_with(".drv"))
        .map(str::to_owned)
        .with_context(|| format!("nix-instantiate -A {attr_path} printed no .drv path"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MISTRAL: &str = r#"let
  version = "3.1.2";
  wheelHashes = {
    aarch64-darwin = "sha256-myo";
    x86_64-linux = "sha256-new";
    aarch64-linux = "sha256-pzc";
  };
  other = {
    aarch64-linux = "sha256-unrelated";
  };
in
"#;

    fn entry(line: usize, system: &str, hash: &str) -> SystemHash {
        SystemHash {
            line,
            system: system.to_owned(),
            hash: hash.to_owned(),
        }
    }

    #[test]
    fn siblings_of_the_host_line() {
        assert_eq!(
            sibling_systems(MISTRAL, "x86_64-linux", "sha256-new"),
            vec![
                entry(3, "aarch64-darwin", "sha256-myo"),
                entry(5, "aarch64-linux", "sha256-pzc"),
            ]
        );
        // Quoted keys (`"x86_64-linux" = …`) and identical hashes on several
        // systems (codex's srcBindingHashes).
        let codex = "srcBindingHashes = {\n  \"x86_64-linux\" = \"sha256-dye\";\n  \
                     \"aarch64-linux\" = \"sha256-dye\";\n};\n";
        assert_eq!(
            sibling_systems(codex, "x86_64-linux", "sha256-dye"),
            vec![entry(2, "aarch64-linux", "sha256-dye")]
        );
        // Not bound to the host system: nothing to do.
        assert!(sibling_systems(MISTRAL, "x86_64-linux", "sha256-myo").is_empty());
        assert!(sibling_systems(MISTRAL, "x86_64-linux", "sha256-missing").is_empty());
        assert!(sibling_systems("hash = \"sha256-new\";", "x86_64-linux", "sha256-new").is_empty());
    }

    #[test]
    fn rewrite_only_the_sibling_lines() {
        let updates = vec![
            (
                entry(3, "aarch64-darwin", "sha256-myo"),
                "sha256-DARWIN".to_owned(),
            ),
            (
                entry(5, "aarch64-linux", "sha256-pzc"),
                "sha256-ARM".to_owned(),
            ),
        ];
        let expected = MISTRAL
            .replace("sha256-myo", "sha256-DARWIN")
            .replace("sha256-pzc", "sha256-ARM");
        assert_eq!(rewrite_lines(MISTRAL, &updates), expected);

        // Same old hash on the host's line is left alone.
        let codex = "  x86_64-linux = \"sha256-new\";\n  aarch64-linux = \"sha256-dye\";\n";
        let updates = vec![(
            entry(1, "aarch64-linux", "sha256-dye"),
            "sha256-x".to_owned(),
        )];
        assert_eq!(
            rewrite_lines(codex, &updates),
            "  x86_64-linux = \"sha256-new\";\n  aarch64-linux = \"sha256-x\";\n"
        );
    }

    #[test]
    fn native_system_shape() {
        let system = native_system();
        assert!(
            system_line().is_match(&format!("{system} = \"h\";")),
            "{system}"
        );
    }
}
