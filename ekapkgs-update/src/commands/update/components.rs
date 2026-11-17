//! Independently pinned components of a package
//! (`passthru.ekapkgs-update.components`).
//!
//! Some packages bundle sources pinned separately from their own version:
//! docker builds runc, containerd and tini at their own tags. A component names
//! the FOD it is fetched by; its upstream is inferred from that FOD's URL (or
//! `github-repo`), and its new version is chosen by the same release selection
//! as a package's, with optional per-component settings:
//!
//! ```nix
//! passthru.ekapkgs-update.components = {
//!   runc = {
//!     src = "docker-runc.src";
//!     semver-strategy = "minor";
//!   };
//!   tini.src = "docker-tini.src";
//! };
//! ```
//!
//! With `pinned-by`, a component follows the pin in a file of the package's
//! own source instead (see [`super::pins`]).
//!
//! Components are only considered while their package is updated. The bump
//! rewrites the component's tag (or version) literal in the package file; the
//! FOD then has a new `.drv` and its hash is refreshed by
//! [`super::fod_refresh`].

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::build_nix_expr;
use super::pins::PinnedBy;
use crate::nix::normalize_entry_point;
use crate::rewrite::rev_update::is_likely_commit_sha;
use crate::vcs_sources::{SemverStrategy, UpstreamSource, extract_version_with_regex};

/// One entry of `passthru.ekapkgs-update.components`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
struct ComponentSpec {
    /// Attr path of the component's FOD, relative to the package
    src: String,
    semver_strategy: Option<String>,
    version_regex: Option<String>,
    include_prereleases: Option<bool>,
    /// Explicit `owner/repo` when the URL is not recognized
    github_repo: Option<String>,
    /// Follow the pin in a file of the package's source instead of picking a
    /// release (see [`super::pins`])
    pinned_by: Option<PinnedBy>,
}

/// What a component's FOD is currently fetching.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ComponentSrc {
    tag: Option<String>,
    rev: Option<String>,
    urls: Vec<String>,
    /// The version in the FOD's name (`librusty_v8-150.4.0`)
    #[serde(default)]
    version: Option<String>,
}

impl ComponentSrc {
    /// The git ref the FOD fetches: its `tag`, or its `rev` without a
    /// `refs/tags/` prefix.
    fn current_ref(&self) -> Option<&str> {
        self.tag
            .as_deref()
            .or_else(|| {
                self.rev
                    .as_deref()
                    .map(|rev| rev.strip_prefix("refs/tags/").unwrap_or(rev))
            })
            .filter(|r| !r.is_empty())
    }
}

/// A component bumped as part of an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentUpdate {
    pub name: String,
    pub old: String,
    pub new: String,
}

impl std::fmt::Display for ComponentUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} -> {}", self.name, self.old, self.new)
    }
}

/// Bump every component of `attr_path` that has a newer compatible release,
/// rewriting its ref in `file`.
///
/// Problems with a single component (unrecognized upstream, no release,
/// a ref that can't be found in the file) are logged and skip it: the
/// package update itself does not depend on them.
pub async fn update_components(
    eval_entry_point: &str,
    attr_path: &str,
    file: &Path,
) -> anyhow::Result<Vec<ComponentUpdate>> {
    let entry = normalize_entry_point(eval_entry_point);
    let specs: BTreeMap<String, ComponentSpec> = eval_json(&format!(
        "let pkgs = import {entry} {{ }}; in pkgs.{attr_path}.passthru.ekapkgs-update.components \
         or {{ }}"
    ))
    .await
    .context("evaluate passthru.ekapkgs-update.components")?;

    let mut updates = Vec::new();
    for (name, spec) in specs {
        match update_component(eval_entry_point, attr_path, file, &name, &spec).await {
            Ok(Some(update)) => {
                info!("{attr_path}: component {update}");
                updates.push(update);
            },
            Ok(None) => {},
            Err(e) => warn!("{attr_path}: component {name} not updated: {e:#}"),
        }
    }
    Ok(updates)
}

async fn update_component(
    eval_entry_point: &str,
    attr_path: &str,
    file: &Path,
    name: &str,
    spec: &ComponentSpec,
) -> anyhow::Result<Option<ComponentUpdate>> {
    let entry = normalize_entry_point(eval_entry_point);
    let src: ComponentSrc = eval_json(&format!(
        "let pkgs = import {entry} {{ }}; s = pkgs.{attr_path}.{}; u = s.url or \"\"; \
         v = (builtins.parseDrvName (s.name or \"\")).version; in {{ \
         tag = s.tag or null; rev = s.rev or null; \
         version = if v == \"\" then null else v; \
         urls = (if builtins.isString u && u != \"\" then [ u ] else [ ]) \
           ++ builtins.filter builtins.isString (s.urls or [ ]); }}",
        spec.src
    ))
    .await
    .with_context(|| format!("evaluate {}", spec.src))?;

    if let Some(pinned_by) = &spec.pinned_by {
        let pin = read_pin(eval_entry_point, attr_path, pinned_by).await?;
        return update_pinned(file, name, spec, &src, &pin).await;
    }

    let current_ref = src
        .current_ref()
        .with_context(|| format!("{} has no tag or rev", spec.src))?;
    anyhow::ensure!(
        !is_likely_commit_sha(current_ref),
        "{} is pinned to commit {current_ref}; use a tag to let it be updated",
        spec.src
    );
    let current_version = match &spec.version_regex {
        Some(regex) => extract_version_with_regex(current_ref, regex)?,
        None => crate::vcs_sources::extract_version_from_tag(current_ref).to_owned(),
    };

    let upstream = component_upstream(spec, &src)?;
    let strategy = spec
        .semver_strategy
        .as_deref()
        .map(str::parse)
        .transpose()?
        .unwrap_or(SemverStrategy::Latest);

    let release = match upstream
        .get_compatible_release(
            &current_version,
            strategy,
            None,
            None,
            spec.version_regex.as_deref(),
            spec.include_prereleases.unwrap_or(false),
        )
        .await
    {
        Ok(release) => release,
        Err(e) => {
            debug!("{attr_path}: component {name}: no newer release ({e:#})");
            return Ok(None);
        },
    };
    let new_version = release.resolved_version(spec.version_regex.as_deref())?;
    if release.tag_name == current_ref
        || !crate::database::version_at_least(&new_version, &current_version)
        || new_version == current_version
    {
        debug!("{attr_path}: component {name} is up to date at {current_ref}");
        return Ok(None);
    }

    let content = tokio::fs::read_to_string(file)
        .await
        .with_context(|| format!("read {}", file.display()))?;
    let updated = rewrite_ref(
        &content,
        (current_ref, &release.tag_name),
        (&current_version, &new_version),
    )
    .with_context(|| {
        format!(
            "neither \"{current_ref}\" nor \"{current_version}\" appears exactly once in {}",
            file.display()
        )
    })?;
    tokio::fs::write(file, updated)
        .await
        .with_context(|| format!("write {}", file.display()))?;

    Ok(Some(ComponentUpdate {
        name: name.to_owned(),
        old: current_ref.to_owned(),
        new: release.tag_name.clone(),
    }))
}

/// Read the pin of a `pinned-by` component from the (realised) source
/// holding its pin file.
async fn read_pin(
    eval_entry_point: &str,
    attr_path: &str,
    pinned_by: &PinnedBy,
) -> anyhow::Result<String> {
    let (success, stdout, stderr) =
        build_nix_expr(eval_entry_point, attr_path, Some(pinned_by.source())).await?;
    anyhow::ensure!(
        success,
        "could not realise {attr_path}.{}:\n{stderr}",
        pinned_by.source()
    );
    let out = stdout
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .with_context(|| format!("nix-build printed no path for {}", pinned_by.source()))?;
    let path = Path::new(out.trim()).join(&pinned_by.file);
    let content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("read {} in {}", pinned_by.file, pinned_by.source()))?;
    pinned_by.extract(&content)
}

/// Point a `pinned-by` component at `pin`: a commit replaces the fetched
/// commit, a version or tag the version in the component's tag (keeping its
/// scheme) or, for URL-only fetchers, in its name.
async fn update_pinned(
    file: &Path,
    name: &str,
    spec: &ComponentSpec,
    src: &ComponentSrc,
    pin: &str,
) -> anyhow::Result<Option<ComponentUpdate>> {
    let content = tokio::fs::read_to_string(file)
        .await
        .with_context(|| format!("read {}", file.display()))?;
    let Some((updated, update)) = plan_pinned(&content, name, spec, src, pin)? else {
        debug!("component {name} already follows its pin {pin}");
        return Ok(None);
    };
    tokio::fs::write(file, updated)
        .await
        .with_context(|| format!("write {}", file.display()))?;
    Ok(Some(update))
}

/// The rewritten file for [`update_pinned`], or `None` when the component
/// already follows `pin`.
fn plan_pinned(
    content: &str,
    name: &str,
    spec: &ComponentSpec,
    src: &ComponentSrc,
    pin: &str,
) -> anyhow::Result<Option<(String, ComponentUpdate)>> {
    let current_ref = src.current_ref();
    let not_found = |old: &str| {
        format!(
            "the pin of {} (\"{old}\") does not appear exactly once in the package file",
            spec.src
        )
    };
    let update = |old: &str, new: &str| ComponentUpdate {
        name: name.to_owned(),
        old: old.to_owned(),
        new: new.to_owned(),
    };

    if is_likely_commit_sha(pin) {
        let current = current_ref
            .filter(|r| is_likely_commit_sha(r))
            .with_context(|| {
                format!(
                    "pinned to commit {pin}, but {} is not fetched by commit",
                    spec.src
                )
            })?;
        if current == pin {
            return Ok(None);
        }
        let updated = rewrite_ref(content, (current, pin), (current, pin))
            .with_context(|| not_found(current))?;
        return Ok(Some((updated, update(current, pin))));
    }

    let version_of = |text: &str| match &spec.version_regex {
        Some(regex) => extract_version_with_regex(text, regex),
        None => Ok(crate::vcs_sources::extract_version_from_tag(text).to_owned()),
    };
    let new_version = version_of(pin)?;
    match current_ref {
        Some(current) if is_likely_commit_sha(current) => anyhow::bail!(
            "pinned to {pin}, but {} is fetched by commit {current}",
            spec.src
        ),
        Some(current) => {
            let current_version = version_of(current)?;
            if current_version == new_version {
                return Ok(None);
            }
            let new_ref = current.replacen(&current_version, &new_version, 1);
            let updated = rewrite_ref(
                content,
                (current, &new_ref),
                (&current_version, &new_version),
            )
            .with_context(|| not_found(current))?;
            Ok(Some((updated, update(current, &new_ref))))
        },
        None => {
            let current_version = src
                .version
                .as_deref()
                .with_context(|| format!("{} has no tag, rev or versioned name", spec.src))?;
            if current_version == new_version {
                return Ok(None);
            }
            let updated = replace_version(content, current_version, &new_version)
                .with_context(|| not_found(current_version))?;
            Ok(Some((updated, update(current_version, &new_version))))
        },
    }
}

/// Replace every occurrence of `old` not embedded in a longer version
/// (`version = "150.4.0"`, `…/rusty-v8-v150.4.0`), or `None` if there is none.
/// URL-only fetchers repeat their version (codex's `baseUrl`), so all of them
/// move together.
fn replace_version(content: &str, old: &str, new: &str) -> Option<String> {
    let part_of_version = |c: char| c.is_ascii_digit() || c == '.';
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    let mut replaced = false;
    while let Some(idx) = rest.find(old) {
        let before = rest[..idx]
            .chars()
            .next_back()
            .or_else(|| out.chars().next_back());
        let after = rest[idx + old.len()..].chars().next();
        out.push_str(&rest[..idx]);
        if before.is_some_and(part_of_version) || after.is_some_and(part_of_version) {
            out.push_str(old);
        } else {
            out.push_str(new);
            replaced = true;
        }
        rest = &rest[idx + old.len()..];
    }
    out.push_str(rest);
    replaced.then_some(out)
}

/// The upstream a component is released from: its `github-repo`, else the
/// first recognizable URL of its FOD.
fn component_upstream(spec: &ComponentSpec, src: &ComponentSrc) -> anyhow::Result<UpstreamSource> {
    if let Some(repo) = &spec.github_repo {
        let (owner, repo) = repo
            .split_once('/')
            .with_context(|| format!("invalid github-repo '{repo}': expected 'owner/repo'"))?;
        return Ok(UpstreamSource::GitHub {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        });
    }
    src.urls
        .iter()
        .find_map(|url| UpstreamSource::from_url(url))
        .with_context(|| {
            format!(
                "no recognizable upstream in {} URL(s) {:?}; set github-repo",
                spec.src, src.urls
            )
        })
}

/// Rewrite a component's pin in `content`: the string literal of the tag
/// (`runcRev = "v1.4.3";`), or failing that of the version
/// (`runcVersion = "1.4.3";` with `tag = "v${runcVersion}"`). The literal must
/// appear exactly once.
fn rewrite_ref(
    content: &str,
    (old_ref, new_ref): (&str, &str),
    (old_version, new_version): (&str, &str),
) -> Option<String> {
    [(old_ref, new_ref), (old_version, new_version)]
        .into_iter()
        .find_map(|(old, new)| {
            let quoted = format!("\"{old}\"");
            (content.matches(&quoted).count() == 1)
                .then(|| content.replacen(&quoted, &format!("\"{new}\""), 1))
        })
}

/// `nix-instantiate --eval --strict --json`, deserialized.
async fn eval_json<T: serde::de::DeserializeOwned>(expr: &str) -> anyhow::Result<T> {
    let output = Command::new("nix-instantiate")
        .args(["--eval", "--strict", "--json", "-E", expr])
        .output()
        .await
        .context("Failed to execute nix-instantiate")?;
    if !output.status.success() {
        anyhow::bail!(
            "nix-instantiate evaluation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parse nix-instantiate JSON output")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_from_passthru_json() {
        let json = r#"{
            "runc": {"src": "docker-runc.src", "semver-strategy": "minor"},
            "tini": {"src": "docker-tini.src", "github-repo": "krallin/tini",
                     "include-prereleases": false, "version-regex": "v(.*)"}
        }"#;
        let specs: BTreeMap<String, ComponentSpec> = serde_json::from_str(json).unwrap();
        assert_eq!(
            specs["runc"],
            ComponentSpec {
                src: "docker-runc.src".to_owned(),
                semver_strategy: Some("minor".to_owned()),
                version_regex: None,
                include_prereleases: None,
                github_repo: None,
                pinned_by: None,
            }
        );
        assert_eq!(specs["tini"].github_repo.as_deref(), Some("krallin/tini"));
        assert_eq!(specs["tini"].version_regex.as_deref(), Some("v(.*)"));
    }

    #[test]
    fn current_ref_cases() {
        let cases = [
            (Some("v1.4.3"), Some("refs/tags/v1.4.3"), Some("v1.4.3")),
            (None, Some("refs/tags/v2.3.3"), Some("v2.3.3")),
            (
                None,
                Some("369448a167e8b3da4ca5bca0b3307500c3371828"),
                Some("369448a167e8b3da4ca5bca0b3307500c3371828"),
            ),
            (None, None, None),
            (Some(""), None, None),
        ];
        for (tag, rev, expected) in cases {
            let src = ComponentSrc {
                tag: tag.map(str::to_owned),
                rev: rev.map(str::to_owned),
                urls: vec![],
                version: None,
            };
            assert_eq!(src.current_ref(), expected, "{tag:?} {rev:?}");
        }
    }

    #[test]
    fn upstream_from_url_or_override() {
        let src = ComponentSrc {
            tag: Some("v1.4.3".to_owned()),
            rev: None,
            urls: vec![
                "https://github.com/opencontainers/runc/archive/refs/tags/v1.4.3.tar.gz".to_owned(),
            ],
            version: None,
        };
        let mut spec = ComponentSpec {
            src: "docker-runc.src".to_owned(),
            semver_strategy: None,
            version_regex: None,
            include_prereleases: None,
            github_repo: None,
            pinned_by: None,
        };
        let upstream = component_upstream(&spec, &src).unwrap();
        assert_eq!(upstream.source_key(), "opencontainers/runc");

        spec.github_repo = Some("other/runc".to_owned());
        assert_eq!(
            component_upstream(&spec, &src).unwrap().source_key(),
            "other/runc"
        );

        spec.github_repo = None;
        let unknown = ComponentSrc {
            urls: vec!["https://example.org/runc.tar.gz".to_owned()],
            ..src
        };
        assert!(component_upstream(&spec, &unknown).is_err());
    }

    fn pinned_spec(src: &str) -> ComponentSpec {
        ComponentSpec {
            src: src.to_owned(),
            semver_strategy: None,
            version_regex: None,
            include_prereleases: None,
            github_repo: None,
            pinned_by: Some(serde_json::from_str(r#"{"file": "x.wrap"}"#).unwrap()),
        }
    }

    fn fetched(tag: Option<&str>, rev: Option<&str>, version: Option<&str>) -> ComponentSrc {
        ComponentSrc {
            tag: tag.map(str::to_owned),
            rev: rev.map(str::to_owned),
            urls: vec![],
            version: version.map(str::to_owned),
        }
    }

    #[test]
    fn plan_pinned_cases() {
        const QJS_OLD: &str = "9d15fb60b67c45fd0de413bb49e48f8dacebac16";
        const QJS_NEW: &str = "b51e5278439b2fa0510fbb1f3b3dff9f0064d5d7";
        let radare2 = format!(
            "sdb = fetchFromGitHub {{ tag = \"2.5.0\"; }};\nqjs = fetchFromGitHub {{ rev = \"{QJS_OLD}\"; }};\n"
        );
        let codex = "version = \"150.4.0\";\nbaseUrl = \"https://x/rusty-v8-v150.4.0\";\n\
                     other = \"1150.4.0\";\n";
        let kapacitor = "libflux_version = \"0.171.0\";\ntag = \"v${libflux_version}\";\n";
        // (file, component, pin, expected (old, new, text in the result))
        type Case<'a> = (
            &'a str,
            ComponentSrc,
            &'a str,
            Option<(&'a str, &'a str, &'a str)>,
        );
        let cases: [Case<'_>; 6] = [
            // tag literal follows a wrap's revision
            (
                &radare2,
                fetched(Some("2.5.0"), None, None),
                "2.6.2",
                Some(("2.5.0", "2.6.2", "tag = \"2.6.2\"")),
            ),
            // commit follows a commit
            (
                &radare2,
                fetched(None, Some(QJS_OLD), None),
                QJS_NEW,
                Some((QJS_OLD, QJS_NEW, QJS_NEW)),
            ),
            // version literal behind an interpolated tag, `v` scheme kept
            (
                kapacitor,
                fetched(Some("v0.171.0"), None, None),
                "v0.201.0",
                Some(("v0.171.0", "v0.201.0", "libflux_version = \"0.201.0\"")),
            ),
            // URL-only fetcher: every standalone occurrence moves
            (
                codex,
                fetched(None, None, Some("150.4.0")),
                "152.2.0",
                Some(("150.4.0", "152.2.0", "rusty-v8-v152.2.0")),
            ),
            // already at the pin
            (&radare2, fetched(Some("2.6.2"), None, None), "v2.6.2", None),
            (&radare2, fetched(None, Some(QJS_NEW), None), QJS_NEW, None),
        ];
        for (content, src, pin, expected) in cases {
            let planned = plan_pinned(content, "c", &pinned_spec("c"), &src, pin)
                .unwrap_or_else(|e| panic!("{pin}: {e:#}"));
            match (planned, expected) {
                (None, None) => {},
                (Some((updated, update)), Some((old, new, contains))) => {
                    assert_eq!((update.old.as_str(), update.new.as_str()), (old, new));
                    assert!(updated.contains(contains), "{pin}: {updated}");
                    assert!(!updated.contains(&format!("\"{old}\"")), "{pin}: {updated}");
                },
                (planned, expected) => panic!("{pin}: {planned:?} vs {expected:?}"),
            }
        }
        // codex: a longer version containing the old one is left alone
        let (updated, _) = plan_pinned(
            codex,
            "c",
            &pinned_spec("c"),
            &fetched(None, None, Some("150.4.0")),
            "152.2.0",
        )
        .unwrap()
        .unwrap();
        assert!(updated.contains("\"1150.4.0\""), "{updated}");
    }

    #[test]
    fn plan_pinned_refuses() {
        let cases = [
            // commit pin for a tag-fetched component
            (
                fetched(Some("2.5.0"), None, None),
                "b51e5278439b2fa0510fbb1f3b3dff9f0064d5d7",
            ),
            // version pin for a commit-fetched component
            (
                fetched(None, Some("9d15fb60b67c45fd0de413bb49e48f8dacebac16"), None),
                "2.6.2",
            ),
            // nothing to derive the current version from
            (fetched(None, None, None), "2.6.2"),
            // shared version literal (mangohud's Vulkan headers/utility libraries)
            (fetched(Some("v1.4.346"), None, None), "v1.4.350"),
        ];
        let content = "a = \"1.4.346\"; b = \"1.4.346\"; tag = \"2.5.0\";";
        for (src, pin) in cases {
            assert!(
                plan_pinned(content, "c", &pinned_spec("c"), &src, pin).is_err(),
                "{src:?} {pin}"
            );
        }
    }

    #[test]
    fn rewrite_ref_cases() {
        let docker = r#"runcRev = "v1.4.3"; containerdRev = "v2.3.3"; x = "v1.4.30";"#;
        let cases = [
            // Tag literal (docker's runcRev).
            (
                docker,
                ("v1.4.3", "v1.5.3"),
                ("1.4.3", "1.5.3"),
                Some(r#"runcRev = "v1.5.3"; containerdRev = "v2.3.3"; x = "v1.4.30";"#),
            ),
            // Version literal behind an interpolated tag.
            (
                r#"runcVersion = "1.4.3"; tag = "v${runcVersion}";"#,
                ("v1.4.3", "v1.5.3"),
                ("1.4.3", "1.5.3"),
                Some(r#"runcVersion = "1.5.3"; tag = "v${runcVersion}";"#),
            ),
            // Ambiguous: the same literal twice.
            (
                r#"a = "v1.4.3"; b = "v1.4.3";"#,
                ("v1.4.3", "v1.5.3"),
                ("1.4.3", "1.5.3"),
                None,
            ),
            // Not present at all.
            (docker, ("v9.9.9", "v9.9.10"), ("9.9.9", "9.9.10"), None),
        ];
        for (content, refs, versions, expected) in cases {
            assert_eq!(
                rewrite_ref(content, refs, versions).as_deref(),
                expected,
                "{content}"
            );
        }
    }
}
