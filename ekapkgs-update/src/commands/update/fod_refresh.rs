//! Refresh the hashes of every fixed-output derivation a version bump changed.
//!
//! A package can fetch more than its `src`: docker fetches `moby-src` with
//! `tag = "docker-v${version}"`, opencv a `contribSrc`, mission-center two
//! `fetchCargoVendor`s under custom names. After the version bump these FODs
//! still carry the old hash.
//!
//! They are found without annotations by comparing the package's `.drv`
//! closure before the bump with the closure after it: an FOD whose `.drv` is
//! new has inputs (URL, tag, source) that the bump changed. Its hash is located
//! by value in the package's directory, replaced with a fake hash, the FOD is
//! realised, and the hash Nix reports is written back. FODs that depend on
//! another changed FOD (vendored dependencies of a source) are handled after
//! it, so they are rebuilt from the corrected source.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::get_extra_nix_build_args;
use crate::hash_discovery;

/// The fake hash written while discovering the real one.
const FAKE_HASH: &str = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

/// Upper bound on refresh rounds (one FOD is refreshed per round).
const MAX_ROUNDS: usize = 32;

/// The `.drv` closure of a package, taken before any file is edited.
#[derive(Debug, Clone, Default)]
pub struct ClosureSnapshot {
    drvs: HashSet<String>,
}

impl ClosureSnapshot {
    /// Instantiate `attr_path` and record the `.drv` files of its closure.
    pub async fn capture(eval_entry_point: &str, attr_path: &str) -> anyhow::Result<Self> {
        let drv = instantiate(eval_entry_point, attr_path).await?;
        Ok(Self {
            drvs: closure_drvs(&drv).await?,
        })
    }
}

/// Which changed FODs a refresh pass handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FodScope {
    /// Only FODs that depend on no other changed FOD: additional sources
    /// such as `moby-src`.
    Sources,
    /// Every changed FOD, including vendored dependencies of a source.
    All,
}

/// A fixed-output derivation from `nix derivation show`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Fod {
    /// Store path basename of the `.drv`
    drv: String,
    /// Output hash (SRI)
    pub hash: String,
    /// Basenames of the `.drv`s it depends on
    inputs: Vec<String>,
    /// The URL, when the FOD is a plain download (`fetchurl` without
    /// `postFetch`) whose hash `nix store prefetch-file` reproduces
    pub download: Option<String>,
}

impl Fod {
    /// Derivation name without the store hash, for messages.
    pub fn name(&self) -> &str {
        self.drv
            .split_once('-')
            .map_or(self.drv.as_str(), |(_, name)| name)
            .trim_end_matches(".drv")
    }
}

/// Refresh the hashes of FODs whose `.drv` changed since `before`.
///
/// `confirmed` holds hashes this update already discovered (the main `src`,
/// `cargoHash`, …); FODs carrying one of them are up to date. Every hash
/// discovered here is added to it. Hashes are searched for in the files of
/// `package_dir`. Returns the names of the refreshed FODs.
pub async fn refresh_changed_fods(
    eval_entry_point: &str,
    attr_path: &str,
    package_dir: &Path,
    before: &ClosureSnapshot,
    confirmed: &mut HashSet<String>,
    scope: FodScope,
) -> anyhow::Result<Vec<String>> {
    let mut refreshed = Vec::new();

    for _ in 0..MAX_ROUNDS {
        let changed = changed_fods(eval_entry_point, attr_path, before).await?;
        let Some(fod) = next_stale_fod(&changed, confirmed, scope) else {
            if scope == FodScope::All {
                if let Some(stuck) = changed.iter().find(|f| !confirmed.contains(&f.hash)) {
                    anyhow::bail!(
                        "Fixed-output derivation {} changed with the update, but the \
                         FODs it depends on could not be refreshed first",
                        stuck.name()
                    );
                }
            }
            return Ok(refreshed);
        };

        info!(
            "{}: {} changed with the update; refreshing its hash",
            attr_path,
            fod.name()
        );
        let hash = refresh_fod(eval_entry_point, attr_path, package_dir, before, fod).await?;
        info!("{}: {} hash is now {}", attr_path, fod.name(), hash);
        confirmed.insert(hash);
        refreshed.push(fod.name().to_owned());
    }

    anyhow::bail!("Gave up refreshing fixed-output derivation hashes after {MAX_ROUNDS} rounds")
}

/// FODs in the closure of `attr_path` that are not in `before`.
async fn changed_fods(
    eval_entry_point: &str,
    attr_path: &str,
    before: &ClosureSnapshot,
) -> anyhow::Result<Vec<Fod>> {
    let drv = instantiate(eval_entry_point, attr_path).await?;
    let mut new: Vec<String> = closure_drvs(&drv)
        .await?
        .into_iter()
        .filter(|d| !before.drvs.contains(d))
        .collect();
    if new.is_empty() {
        return Ok(Vec::new());
    }
    new.sort();
    show_fods(&new).await
}

/// The next FOD to refresh: one with a stale hash whose changed FOD inputs
/// are all up to date (for [`FodScope::Sources`]: one with no changed FOD
/// inputs at all).
fn next_stale_fod<'a>(
    changed: &'a [Fod],
    confirmed: &HashSet<String>,
    scope: FodScope,
) -> Option<&'a Fod> {
    let by_drv: HashMap<&str, &Fod> = changed.iter().map(|f| (f.drv.as_str(), f)).collect();
    changed
        .iter()
        .filter(|f| !confirmed.contains(&f.hash))
        .find(|f| {
            let mut changed_inputs = f.inputs.iter().filter_map(|i| by_drv.get(i.as_str()));
            match scope {
                FodScope::Sources => changed_inputs.next().is_none(),
                FodScope::All => changed_inputs.all(|input| confirmed.contains(&input.hash)),
            }
        })
}

/// Replace `fod`'s hash with a fake one, realise it, and write back the hash
/// Nix reports.
async fn refresh_fod(
    eval_entry_point: &str,
    attr_path: &str,
    package_dir: &Path,
    before: &ClosureSnapshot,
    fod: &Fod,
) -> anyhow::Result<String> {
    let site = locate_hash(package_dir, &fod.hash).await.with_context(|| {
        format!(
            "Fixed-output derivation {} changed with the update, but its hash could not \
             be located",
            fod.name()
        )
    })?;
    let file = site.file.clone();
    site.replace(FAKE_HASH).await?;
    match discover_with_fake_hash(eval_entry_point, attr_path, before, fod, &file).await {
        Ok(hash) => {
            replace_once(&file, FAKE_HASH, &hash).await?;
            let systems = super::fod_platforms::refresh_other_systems(
                eval_entry_point,
                attr_path,
                &file,
                &hash,
            )
            .await?;
            if !systems.is_empty() {
                info!(
                    "{attr_path}: also refreshed {} for {}",
                    fod.name(),
                    systems.join(", ")
                );
            }
            Ok(hash)
        },
        Err(e) => {
            // Never leave the fake hash behind.
            if let Err(restore) = site.restore().await {
                warn!(
                    "Could not restore {} in {}: {restore:#}",
                    site.text,
                    file.display()
                );
            }
            Err(e)
        },
    }
}

/// With the fake hash written for `fod`, find the FOD's new `.drv` and
/// realise it to learn the real hash.
async fn discover_with_fake_hash(
    eval_entry_point: &str,
    attr_path: &str,
    before: &ClosureSnapshot,
    fod: &Fod,
    file: &Path,
) -> anyhow::Result<String> {
    let changed = changed_fods(eval_entry_point, attr_path, before).await?;
    let Some(fake) = changed.iter().find(|f| f.hash == FAKE_HASH) else {
        anyhow::bail!(
            "After setting a fake hash for {} in {}, no fixed-output derivation in {} uses it",
            fod.name(),
            file.display(),
            attr_path
        );
    };

    let (success, stderr) = realise(&fake.drv).await?;
    (!success)
        .then(|| hash_discovery::extract_hash_for(&stderr, FAKE_HASH))
        .flatten()
        .with_context(|| {
            format!(
                "Could not discover the hash of {} (expected a hash mismatch):\n{stderr}",
                fod.name()
            )
        })
}

/// Where a hash is written: `text` (a string literal with its quotes, or a
/// placeholder such as `lib.fakeHash`) at byte `offset` of `file`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HashSite {
    file: PathBuf,
    text: String,
    offset: usize,
}

/// Placeholders evaluating to the fake hash, left in by an unfinished
/// package (heroic's, element's `hash = lib.fakeHash;`).
const FAKE_HASH_PLACEHOLDERS: [&str; 3] = ["lib.fakeHash", "lib.fakeSha256", "lib.fakeSri"];

impl HashSite {
    /// Replace the text at this site with the string literal `new`.
    async fn replace(&self, new: &str) -> anyhow::Result<()> {
        let content = tokio::fs::read_to_string(&self.file)
            .await
            .with_context(|| format!("read {}", self.file.display()))?;
        let end = self.offset + self.text.len();
        anyhow::ensure!(
            content.get(self.offset..end) == Some(self.text.as_str()),
            "{} changed while its hashes were updated",
            self.file.display()
        );
        let updated = format!("{}\"{new}\"{}", &content[..self.offset], &content[end..]);
        tokio::fs::write(&self.file, updated)
            .await
            .with_context(|| format!("write {}", self.file.display()))
    }

    /// Put the original text back where [`Self::replace`] wrote the fake hash.
    async fn restore(&self) -> anyhow::Result<()> {
        let content = tokio::fs::read_to_string(&self.file)
            .await
            .with_context(|| format!("read {}", self.file.display()))?;
        let fake = format!("\"{FAKE_HASH}\"");
        anyhow::ensure!(
            content.get(self.offset..self.offset + fake.len()) == Some(fake.as_str()),
            "the fake hash is no longer at its place in {}",
            self.file.display()
        );
        let updated = format!(
            "{}{}{}",
            &content[..self.offset],
            self.text,
            &content[self.offset + fake.len()..]
        );
        tokio::fs::write(&self.file, updated)
            .await
            .with_context(|| format!("write {}", self.file.display()))
    }
}

/// Find where `hash` is written in `package_dir`, in any of the notations
/// Nix accepts. It must appear once, or once on a line keyed by the build
/// machine's system (`x86_64-linux = "…";`, when other systems share it).
async fn locate_hash(package_dir: &Path, hash: &str) -> anyhow::Result<HashSite> {
    let forms = hash_notations(hash);
    let mut found = Vec::new();

    let mut entries = tokio::fs::read_dir(package_dir)
        .await
        .with_context(|| format!("read directory {}", package_dir.display()))?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let searchable = path
            .extension()
            .is_some_and(|ext| ext == "nix" || ext == "json");
        if !searchable || !entry.file_type().await?.is_file() {
            continue;
        }
        let content = tokio::fs::read_to_string(&path).await?;
        let quoted: Vec<String> = forms.iter().map(|form| format!("\"{form}\"")).collect();
        let placeholders = (hash == FAKE_HASH).then_some(FAKE_HASH_PLACEHOLDERS);
        let texts = quoted
            .iter()
            .map(String::as_str)
            .chain(placeholders.into_iter().flatten());
        for text in texts {
            for offset in text_offsets(&content, text) {
                let host_keyed = line_at(&content, offset)
                    .trim_start()
                    .trim_start_matches('"')
                    .starts_with(&super::fod_platforms::native_system());
                found.push((
                    HashSite {
                        file: path.clone(),
                        text: text.to_owned(),
                        offset,
                    },
                    host_keyed,
                ));
            }
        }
    }

    if found.len() > 1 && found.iter().filter(|(_, host)| *host).count() == 1 {
        found.retain(|(_, host)| *host);
    }
    match found.len() {
        1 => Ok(found.remove(0).0),
        0 => anyhow::bail!("{hash} does not appear in {}", package_dir.display()),
        n => anyhow::bail!(
            "{hash} appears {n} times in {}; refusing to guess which to replace",
            package_dir.display()
        ),
    }
}

/// Byte offsets of `text`, not as part of a longer identifier
/// (`lib.fakeHash` but not `lib.fakeHashes`).
fn text_offsets(content: &str, text: &str) -> Vec<usize> {
    let ident = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '\'' | '-' | '.');
    content
        .match_indices(text)
        .map(|(offset, _)| offset)
        .filter(|&offset| {
            text.starts_with('"')
                || (!content[..offset].ends_with(ident)
                    && !content[offset + text.len()..].starts_with(ident))
        })
        .collect()
}

/// The line containing byte `offset`.
fn line_at(content: &str, offset: usize) -> &str {
    let start = content[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = content[offset..]
        .find('\n')
        .map_or(content.len(), |i| offset + i);
    &content[start..end]
}

/// Replace the string literal `old` with `new` in `file`; `old` must occur
/// exactly once.
async fn replace_once(file: &Path, old: &str, new: &str) -> anyhow::Result<()> {
    let content = tokio::fs::read_to_string(file)
        .await
        .with_context(|| format!("read {}", file.display()))?;
    let quoted = format!("\"{old}\"");
    anyhow::ensure!(
        content.matches(&quoted).count() == 1,
        "expected {quoted} exactly once in {}",
        file.display()
    );
    let updated = content.replacen(&quoted, &format!("\"{new}\""), 1);
    tokio::fs::write(file, updated)
        .await
        .with_context(|| format!("write {}", file.display()))
}

/// Whether `other` (SRI, nix32 or base16, optionally `sha256:`-prefixed)
/// is the same SHA-256 hash as `sri`.
pub(super) fn same_hash(sri: &str, other: &str) -> bool {
    let other = other.strip_prefix("sha256:").unwrap_or(other);
    hash_notations(sri).iter().any(|form| form == other)
}

/// The spellings of an SRI hash a package may use: SRI itself and, for
/// sha256, the bare nix32 and base16 forms (`sha256 = "…";`).
fn hash_notations(sri: &str) -> Vec<String> {
    let mut forms = vec![sri.to_owned()];
    if let Some(bytes) = sri
        .strip_prefix("sha256-")
        .and_then(base64_decode)
        .filter(|b| b.len() == 32)
    {
        forms.push(nix32_encode(&bytes));
        forms.push(bytes.iter().map(|b| format!("{b:02x}")).collect());
    }
    forms
}

/// Decode standard (padded) base64.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = input.trim_end_matches('=');
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in input.bytes() {
        let value = ALPHABET.iter().position(|&a| a == c)?;
        buf = (buf << 6) | u32::try_from(value).ok()?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buf >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// Nix's base-32 encoding (`nix hash convert --to nix32`).
fn nix32_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";
    let len = (bytes.len() * 8).div_ceil(5);
    (0..len)
        .rev()
        .map(|n| {
            let b = n * 5;
            let (i, j) = (b / 8, b % 8);
            let low = u16::from(bytes[i]) >> j;
            let high = bytes
                .get(i + 1)
                .map_or(0, |&next| u16::from(next) << (8 - j));
            char::from(ALPHABET[usize::from((low | high) & 0x1f)])
        })
        .collect()
}

/// `nix-instantiate <entry> -A <attr>`, returning the `.drv` path.
async fn instantiate(eval_entry_point: &str, attr_path: &str) -> anyhow::Result<String> {
    let output = Command::new("nix-instantiate")
        .arg(eval_entry_point)
        .arg("-A")
        .arg(attr_path)
        .output()
        .await
        .context("Failed to execute nix-instantiate")?;
    if !output.status.success() {
        anyhow::bail!(
            "nix-instantiate -A {attr_path} failed: {}",
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

/// Basenames of the `.drv` files in the closure of `drv`.
pub(super) async fn closure_drvs(drv: &str) -> anyhow::Result<HashSet<String>> {
    let output = Command::new("nix-store")
        .args(["--query", "--requisites", drv])
        .output()
        .await
        .context("Failed to execute nix-store --query")?;
    if !output.status.success() {
        anyhow::bail!(
            "nix-store --query --requisites {drv} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| l.ends_with(".drv"))
        .map(|l| store_basename(l.trim()).to_owned())
        .collect())
}

/// The fixed-output derivations among `drvs` (basenames).
pub(super) async fn show_fods(drvs: &[String]) -> anyhow::Result<Vec<Fod>> {
    let output = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "derivation",
            "show",
        ])
        .args(drvs.iter().map(|d| format!("/nix/store/{d}")))
        .output()
        .await
        .context("Failed to execute nix derivation show")?;
    if !output.status.success() {
        anyhow::bail!(
            "nix derivation show failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_fods(&String::from_utf8_lossy(&output.stdout))
}

/// Parse `nix derivation show` output into its fixed-output derivations.
///
/// Handles both the flat `{ "<drv>": {…} }` layout and the newer
/// `{ "derivations": {…} }` one, and both the `inputDrvs` and the
/// `inputs.drvs` spelling of a derivation's inputs.
fn parse_fods(json: &str) -> anyhow::Result<Vec<Fod>> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("parse nix derivation show output")?;
    let drvs = value
        .get("derivations")
        .unwrap_or(&value)
        .as_object()
        .context("nix derivation show output is not an object")?;

    let mut fods: Vec<Fod> = drvs
        .iter()
        .filter_map(|(drv, info)| {
            let hash = info.pointer("/outputs/out/hash")?.as_str()?;
            let algo = info
                .pointer("/outputs/out/hashAlgo")
                .and_then(serde_json::Value::as_str);
            let inputs = info
                .pointer("/inputs/drvs")
                .or_else(|| info.get("inputDrvs"))
                .and_then(serde_json::Value::as_object)
                .map(|inputs| {
                    inputs
                        .keys()
                        .map(|i| store_basename(i).to_owned())
                        .collect()
                })
                .unwrap_or_default();
            Some(Fod {
                drv: store_basename(drv).to_owned(),
                hash: sri_hash(hash, algo),
                inputs,
                download: plain_download(info),
            })
        })
        .collect();
    fods.sort_by(|a, b| a.drv.cmp(&b.drv));
    Ok(fods)
}

/// A FOD's `outputs.out.hash` as SRI. Newer Nix emits SRI; older Nix and
/// Lix emit base16 with a separate `hashAlgo` (`sha256`, or `r:sha256` for
/// recursive/NAR hashing), which would never match the SRI hashes in files
/// and from `got:` lines.
fn sri_hash(hash: &str, algo: Option<&str>) -> String {
    let Some(algo) = algo else {
        return hash.to_owned();
    };
    let algo = algo.strip_prefix("r:").unwrap_or(algo);
    let digest_len = match algo {
        "sha1" => 20,
        "sha256" => 32,
        "sha512" => 64,
        _ => return hash.to_owned(),
    };
    if hash.len() != digest_len * 2 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return hash.to_owned();
    }
    let bytes: Option<Vec<u8>> = (0..hash.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hash[i..i + 2], 16).ok())
        .collect();
    bytes.map_or_else(
        || hash.to_owned(),
        |bytes| format!("{algo}-{}", base64_encode(&bytes)),
    )
}

/// Encode standard (padded) base64.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The URL of a FOD that is a plain download: a flat hash and a `url`/`urls`
/// (env or structured attrs) with no `postFetch` or `downloadToTemp`.
fn plain_download(info: &serde_json::Value) -> Option<String> {
    let attr = |name: &str| {
        info.pointer(&format!("/env/{name}"))
            .or_else(|| info.pointer(&format!("/structuredAttrs/{name}")))
            .filter(|v| !v.is_null())
    };
    let flat = info
        .pointer("/outputs/out/method")
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || {
                info.pointer("/outputs/out/hashAlgo")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|algo| !algo.starts_with("r:"))
            },
            |method| method == "flat",
        );
    let post_fetch = attr("postFetch").and_then(serde_json::Value::as_str);
    let to_temp = attr("downloadToTemp").is_some_and(|v| v == true || v == "1");
    if !flat || post_fetch.is_some_and(|p| !p.trim().is_empty()) || to_temp {
        return None;
    }
    let urls: Vec<String> = ["urls", "url"]
        .iter()
        .filter_map(|name| attr(name))
        .flat_map(|v| match v {
            serde_json::Value::String(s) => s.split_whitespace().map(str::to_owned).collect(),
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        })
        .collect();
    urls.into_iter()
        .find(|url| url.starts_with("https://") || url.starts_with("http://"))
}

/// `/nix/store/<hash>-<name>` → `<hash>-<name>`.
fn store_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Realise a `.drv`, returning (success, stderr).
async fn realise(drv: &str) -> anyhow::Result<(bool, String)> {
    debug!("Realising {}", drv);
    let output = Command::new("nix-build")
        .arg(format!("/nix/store/{drv}"))
        .arg("--no-out-link")
        .args(get_extra_nix_build_args())
        .output()
        .await
        .context("Failed to execute nix-build")?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOBY_SRI: &str = "sha256-k28c4cwt+ASVj8FTvM8dgIOL3yqR5wbI21r3LtrVamU=";

    #[test]
    fn same_hash_accepts_any_notation() {
        let forms = hash_notations(MOBY_SRI);
        assert!(same_hash(MOBY_SRI, MOBY_SRI));
        assert!(same_hash(MOBY_SRI, &forms[1]));
        assert!(same_hash(MOBY_SRI, &format!("sha256:{}", forms[2])));
        assert!(!same_hash(MOBY_SRI, FAKE_HASH));
        assert!(!same_hash(MOBY_SRI, ""));
    }

    #[test]
    fn hash_notations_match_nix() {
        // Expected values from `nix hash convert`.
        let cases = [
            (
                MOBY_SRI,
                "0rbaspd2xxssvg40drwi5bgqp0w03p7vqly1iyah9y1drkhiqvwk",
                "936f1ce1cc2df804958fc153bccf1d80838bdf2a91e706c8db5af72edad56a65",
            ),
            (
                "sha256-wlgtkixogqjlQLU1kOWdMWJatG6x1X513/C9DXt40Y8=",
                "13yig1xhvgghvxspxmdidss5lqiikpjr0ddm83jsi0k85j92sn62",
                "c2582d922c6882a8e540b53590e59d31625ab46eb1d57e75dff0bd0d7b78d18f",
            ),
            (FAKE_HASH, &"0".repeat(52), &"0".repeat(64)),
        ];
        for (sri, nix32, base16) in cases {
            assert_eq!(
                hash_notations(sri),
                vec![sri.to_owned(), nix32.to_owned(), base16.to_owned()],
                "{sri}"
            );
        }
        // Other algorithms are only searched for as SRI.
        assert_eq!(
            hash_notations("sha512-abc="),
            vec!["sha512-abc=".to_owned()]
        );
    }

    #[test]
    fn count_literal_needs_whole_string() {
        let content = r#"a = "v1.4.3"; b = "v1.4.30"; c = "xv1.4.3";"#;
        assert_eq!(text_offsets(content, "\"v1.4.3\""), vec![4]);
        assert!(text_offsets(content, "\"v1.4\"").is_empty());
        let placeholder = "a = lib.fakeHash; b = lib.fakeHashes; c = xlib.fakeHash;";
        assert_eq!(text_offsets(placeholder, "lib.fakeHash"), vec![4]);
    }

    fn fod(drv: &str, hash: &str, inputs: &[&str]) -> Fod {
        Fod {
            drv: drv.to_owned(),
            hash: hash.to_owned(),
            inputs: inputs.iter().map(|&i| i.to_owned()).collect(),
            download: None,
        }
    }

    #[test]
    fn parse_fods_both_layouts() {
        let nested = r#"{"derivations": {
            "aaa-moby-29.10.0-github-source.drv": {
                "outputs": {"out": {"hash": "sha256-moby", "method": "nar"}},
                "inputs": {"drvs": {"bbb-curl.drv": {}}}
            },
            "ccc-moby-29.10.0.drv": {"outputs": {"out": {}}}
        }}"#;
        assert_eq!(
            parse_fods(nested).unwrap(),
            vec![fod(
                "aaa-moby-29.10.0-github-source.drv",
                "sha256-moby",
                &["bbb-curl.drv"]
            )]
        );

        let flat = r#"{
            "/nix/store/ddd-x-vendor-staging.drv": {
                "outputs": {"out": {"hash": "sha256-v"}},
                "inputDrvs": {"/nix/store/eee-x-source.drv": {}}
            }
        }"#;
        assert_eq!(
            parse_fods(flat).unwrap(),
            vec![fod(
                "ddd-x-vendor-staging.drv",
                "sha256-v",
                &["eee-x-source.drv"]
            )]
        );
    }

    #[test]
    fn next_stale_fod_orders_by_dependencies() {
        // mission-center: two vendor FODs built from the (already refreshed) source.
        let changed = vec![
            fod("a-src.drv", "sha256-src-new", &[]),
            fod(
                "b-vendor.drv",
                "sha256-vendor-old",
                &["a-src.drv", "z-curl.drv"],
            ),
            fod("c-moby.drv", "sha256-moby-old", &["z-curl.drv"]),
        ];
        let mut confirmed = HashSet::from(["sha256-src-new".to_owned()]);

        let cases = [
            (FodScope::Sources, Some("c-moby.drv")),
            (FodScope::All, Some("b-vendor.drv")),
        ];
        for (scope, expected) in cases {
            assert_eq!(
                next_stale_fod(&changed, &confirmed, scope).map(|f| f.drv.as_str()),
                expected,
                "{scope:?}"
            );
        }

        // A dependency that is itself stale blocks its dependents.
        confirmed.clear();
        assert_eq!(
            next_stale_fod(&changed, &confirmed, FodScope::All).map(|f| f.drv.as_str()),
            Some("a-src.drv")
        );

        // Nothing left once every hash is confirmed.
        confirmed.extend(["sha256-vendor-old".to_owned(), "sha256-moby-old".to_owned()]);
        confirmed.insert("sha256-src-new".to_owned());
        assert_eq!(next_stale_fod(&changed, &confirmed, FodScope::All), None);
    }

    #[test]
    fn fod_name_strips_store_hash() {
        assert_eq!(
            fod("n49kz-moby-29.7.2-github-source.drv", "", &[]).name(),
            "moby-29.7.2-github-source"
        );
    }

    #[tokio::test]
    async fn locate_and_replace_hash_by_value() {
        let dir = tempfile::tempdir().unwrap();
        let nix32 = "0rbaspd2xxssvg40drwi5bgqp0w03p7vqly1iyah9y1drkhiqvwk";
        tokio::fs::write(
            dir.path().join("default.nix"),
            format!("{{ mobyHash = \"{MOBY_SRI}\"; }}"),
        )
        .await
        .unwrap();
        tokio::fs::write(
            dir.path().join("other.nix"),
            format!("{{ sha256 = \"{nix32}\"; }}"),
        )
        .await
        .unwrap();
        tokio::fs::write(dir.path().join("notes.txt"), MOBY_SRI)
            .await
            .unwrap();

        // Found in two notations, in two files: ambiguous.
        assert!(locate_hash(dir.path(), MOBY_SRI).await.is_err());

        tokio::fs::remove_file(dir.path().join("other.nix"))
            .await
            .unwrap();
        let site = locate_hash(dir.path(), MOBY_SRI).await.unwrap();
        assert_eq!(site.file, dir.path().join("default.nix"));
        assert_eq!(site.text, format!("\"{MOBY_SRI}\""));

        site.replace(FAKE_HASH).await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&site.file).await.unwrap(),
            format!("{{ mobyHash = \"{FAKE_HASH}\"; }}")
        );
        assert!(locate_hash(dir.path(), MOBY_SRI).await.is_err());
    }

    #[tokio::test]
    async fn locate_and_restore_a_fake_hash_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("default.nix");
        let original = "{\n  pnpmDeps = fetchPnpmDeps {\n    hash = lib.fakeHash;\n  };\n}\n";
        tokio::fs::write(&file, original).await.unwrap();

        let site = locate_hash(dir.path(), FAKE_HASH).await.unwrap();
        assert_eq!(site.text, "lib.fakeHash");
        site.replace(FAKE_HASH).await.unwrap();
        assert!(
            tokio::fs::read_to_string(&file)
                .await
                .unwrap()
                .contains(&format!("hash = \"{FAKE_HASH}\";"))
        );
        site.restore().await.unwrap();
        assert_eq!(tokio::fs::read_to_string(&file).await.unwrap(), original);
    }

    #[tokio::test]
    async fn locate_hash_prefers_the_build_systems_line() {
        // codex: two linux systems share a srcBinding hash.
        let dir = tempfile::tempdir().unwrap();
        let host = super::super::fod_platforms::native_system();
        let content = format!(
            "{{\n  srcBindingHashes = {{\n    zz-other = \"{MOBY_SRI}\";\n    \
             {host} = \"{MOBY_SRI}\";\n  }};\n}}\n"
        );
        let file = dir.path().join("default.nix");
        tokio::fs::write(&file, &content).await.unwrap();
        let site = locate_hash(dir.path(), MOBY_SRI).await.unwrap();
        site.replace(FAKE_HASH).await.unwrap();
        let updated = tokio::fs::read_to_string(&file).await.unwrap();
        assert!(
            updated.contains(&format!("zz-other = \"{MOBY_SRI}\"")),
            "{updated}"
        );
        assert!(
            updated.contains(&format!("{host} = \"{FAKE_HASH}\"")),
            "{updated}"
        );
    }

    #[test]
    fn base16_hashes_become_sri() {
        // sha256 of the empty string.
        let empty_hex = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let empty_sri = "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";
        for (hash, algo, expected) in [
            (empty_hex, Some("sha256"), empty_sri),
            (empty_hex, Some("r:sha256"), empty_sri),
            (empty_sri, Some("sha256"), empty_sri),
            (empty_sri, None, empty_sri),
            (empty_hex, None, empty_hex),
            (
                "da39a3ee5e6b4b0d3255bfef95601890afd80709",
                Some("sha1"),
                "sha1-2jmj7l5rSw0yVb/vlWAYkK/YBwk=",
            ),
            (
                "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
                 47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
                Some("sha512"),
                "sha512-z4PhNX7vuL3xVChQ1m2AB9Yg5AULVxXcg/SpIdNs6c5H0NE8XYXysP+DGNKHfuwvY7kxvUdBeoGlODJ6+SfaPg==",
            ),
            ("abc", Some("sha256"), "abc"),
        ] {
            assert_eq!(sri_hash(hash, algo), expected, "{hash} {algo:?}");
        }

        let json = format!(
            r#"{{"/nix/store/a-old.drv": {{
                "outputs": {{"out": {{"hash": "{empty_hex}", "hashAlgo": "r:sha256"}}}},
                "inputDrvs": {{}}
            }}}}"#
        );
        assert_eq!(
            parse_fods(&json).unwrap(),
            vec![fod("a-old.drv", empty_sri, &[])]
        );
    }

    #[test]
    fn plain_downloads() {
        let json = r#"{"derivations": {
            "a-librusty_v8-150.4.0.drv": {
                "outputs": {"out": {"hash": "sha256-a", "method": "flat"}},
                "structuredAttrs": {"urls": ["mirror://x", "https://example.org/a.gz"],
                                    "downloadToTemp": false, "postFetch": ""}
            },
            "b-wheel.drv": {
                "outputs": {"out": {"hash": "sha256-b", "method": "flat"}},
                "env": {"urls": "https://example.org/b.whl https://mirror.org/b.whl"}
            },
            "c-zip.drv": {
                "outputs": {"out": {"hash": "sha256-c", "method": "nar"}},
                "env": {"url": "https://example.org/c.zip"}
            },
            "d-post.drv": {
                "outputs": {"out": {"hash": "sha256-d", "method": "flat"}},
                "env": {"url": "https://example.org/d", "postFetch": "chmod +x $out"}
            },
            "e-old.drv": {
                "outputs": {"out": {
                    "hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "hashAlgo": "sha256"
                }},
                "env": {"url": "https://example.org/e"}
            }
        }}"#;
        let downloads: Vec<(String, Option<String>)> = parse_fods(json)
            .unwrap()
            .into_iter()
            .map(|f| (f.drv, f.download))
            .collect();
        let url = |u: &str| Some(u.to_owned());
        assert_eq!(
            downloads,
            vec![
                (
                    "a-librusty_v8-150.4.0.drv".to_owned(),
                    url("https://example.org/a.gz")
                ),
                ("b-wheel.drv".to_owned(), url("https://example.org/b.whl")),
                ("c-zip.drv".to_owned(), None),
                ("d-post.drv".to_owned(), None),
                ("e-old.drv".to_owned(), url("https://example.org/e")),
            ]
        );
    }
}
