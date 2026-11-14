//! Upstream index builder: enumerates packages with a single `nix-eval-jobs`
//! pass, determines each package's upstream source and version policy, and
//! stores the mapping in the `upstream_index` database table.
//!
//! All per-package data is extracted by an `--apply` function during the same
//! evaluation, so indexing costs one evaluation of the package set instead of
//! ~20 `nix-instantiate` calls per package (which took hours on corepkgs).
//!
//! Every indexed package is also registered in the `updates` table (with its
//! pname, description, homepage and upstream) so the dashboard shows it
//! before the first poll.
//!
//! The repo's entries are written in one transaction. Entries for packages
//! that disappeared from the repo are pruned in the same transaction, unless
//! the evaluation looks incomplete (stream errors, nothing evaluated, or far
//! fewer packages than before), in which case the previous entries are kept.

use std::path::{Path, PathBuf};

use futures::{StreamExt, pin_mut};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::database::{Database, NewUpstreamIndexEntry, PackageInfo};
use crate::nix::nix_eval_jobs::{NixEvalDrv, NixEvalItem};
use crate::nix::run_eval::{EvalJobsOptions, run_nix_eval_jobs_with_options};
use crate::package::PackageMetadata;
use crate::variant_strategy::infer_update_strategy;
use crate::vcs_sources::{SemverStrategy, VersionPolicy};

/// `--apply` function extracting everything the index needs from a
/// derivation. Every field is guarded so a single odd package cannot fail the
/// job: values are type-checked and evaluation errors become `null`.
///
/// Mirrors the queries in [`PackageMetadata::from_attr_path`].
const INDEX_APPLY_EXPR: &str = r#"drv:
let
  try = e: let r = builtins.tryEval e; in if r.success then r.value else null;
  str = v: if builtins.isString v then v else null;
  bool = v: if builtins.isBool v then v else null;
  cfg = let v = try (drv.passthru.ekapkgs-update or null); in if builtins.isAttrs v then v else { };
  opt = name: try (cfg.${name} or null);
  src = try (drv.src or null);
  # fetchurl may set `url = ""` and list the real mirrors in `urls`.
  srcUrl = try (
    if !(builtins.isAttrs src) then null
    else
      let
        u = src.url or null;
        us = src.urls or [ ];
        all =
          (if builtins.isString u && u != "" then [ u ] else [ ])
          ++ (if builtins.isList us then builtins.filter (x: builtins.isString x && x != "") us else [ ]);
      in
      if all == [ ] then null else builtins.concatStringsSep " " all);
  script = opt "update-script";
in
{
  version = try (str (drv.version or (builtins.parseDrvName drv.name).version));
  pname = try (str (drv.pname or null));
  description = try (str (drv.meta.description or null));
  homepage = try (str (drv.meta.homepage or null));
  inherit srcUrl;
  position = try (str (drv.meta.position or null));
  hasVariants = try (drv ? variants);
  isPython = try (drv ? pythonModule);
  skip = bool (opt "skip");
  semverStrategy = str (opt "semver-strategy");
  includePrereleases = bool (opt "include-prereleases");
  versionRegex = str (opt "version-regex");
  githubRepo = str (opt "github-repo");
  updateScript = try (
    if builtins.isString script || builtins.isPath script then toString script
    else if builtins.isAttrs script && script ? outPath then toString script.outPath
    else null);
}"#;

/// Per-derivation data produced by [`INDEX_APPLY_EXPR`].
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct IndexedAttrs {
    version: Option<String>,
    pname: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    src_url: Option<String>,
    position: Option<String>,
    has_variants: Option<bool>,
    is_python: Option<bool>,
    skip: Option<bool>,
    semver_strategy: Option<String>,
    include_prereleases: Option<bool>,
    version_regex: Option<String>,
    github_repo: Option<String>,
    update_script: Option<String>,
}

/// What to index.
#[derive(Debug, Clone, Copy)]
pub struct IndexTarget<'a> {
    /// Repository name stored with every entry (empty in single-repo mode).
    pub repo_name: &'a str,
    /// File passed to nix-eval-jobs to enumerate packages.
    pub enumerate_file: &'a str,
    /// Optional nix-eval-jobs `--select` applied to the enumeration root.
    pub select: Option<&'a str>,
    /// Only index packages whose `meta.position` lies inside this directory:
    /// anything defined elsewhere (e.g. a pinned dependency in the Nix store)
    /// cannot be edited by an update.
    pub repo_dir: Option<&'a Path>,
}

/// Why a derivation was not indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipReason {
    NoMetadata,
    NoVersion,
    OptedOut,
    NoPosition,
    OutsideRepo,
    NoUpstream,
    Pinned,
}

/// Result statistics from an index build run.
#[derive(Debug, Default)]
pub struct IndexStats {
    /// Derivations and evaluation errors reported by nix-eval-jobs.
    pub evaluated: usize,
    /// Entries stored in the index.
    pub indexed: usize,
    /// Derivations deliberately not indexed (see the breakdown counters).
    pub skipped: usize,
    /// Evaluation errors (excluding uncallable function attributes).
    pub errors: usize,
    /// Stale index rows removed.
    pub pruned: u64,
    /// The evaluation stream itself failed (or produced nothing), or the
    /// index could not be stored; the index may be incomplete.
    pub stream_error: Option<String>,
    skipped_outside_repo: usize,
    skipped_no_upstream: usize,
    skipped_pinned: usize,
    skipped_opted_out: usize,
    skipped_other: usize,
}

impl IndexStats {
    fn record_skip(&mut self, reason: SkipReason) {
        self.skipped += 1;
        match reason {
            SkipReason::OutsideRepo => self.skipped_outside_repo += 1,
            SkipReason::NoUpstream => self.skipped_no_upstream += 1,
            SkipReason::Pinned => self.skipped_pinned += 1,
            SkipReason::OptedOut => self.skipped_opted_out += 1,
            SkipReason::NoMetadata | SkipReason::NoVersion | SkipReason::NoPosition => {
                self.skipped_other += 1;
            },
        }
    }
}

/// Build (or rebuild) the upstream index for one repository.
///
/// Entries are stored atomically. Stale entries are only pruned when the
/// evaluation looks complete (see [`prune_decision`]); failures are reported
/// in [`IndexStats::stream_error`].
pub async fn build_upstream_index(
    target: IndexTarget<'_>,
    db: &Database,
    max_eval_workers: Option<usize>,
) -> IndexStats {
    let label = if target.repo_name.is_empty() {
        "default"
    } else {
        target.repo_name
    };
    info!(
        "[{}] Building upstream index from {}{}",
        label,
        target.enumerate_file,
        target
            .select
            .map(|s| format!(" (select: {s})"))
            .unwrap_or_default()
    );

    let previous = db
        .upstream_index_count(Some(target.repo_name))
        .await
        .unwrap_or(0);
    let canonical_repo_dir = target
        .repo_dir
        .map(|dir| std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()));

    let stream = run_nix_eval_jobs_with_options(
        target.enumerate_file.to_owned(),
        EvalJobsOptions {
            max_workers: max_eval_workers,
            select: target.select.map(str::to_owned),
            apply: Some(INDEX_APPLY_EXPR.to_owned()),
        },
    );
    pin_mut!(stream);

    let mut stats = IndexStats::default();
    let mut indexed: Vec<(NewUpstreamIndexEntry, PackageInfo)> = Vec::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(NixEvalItem::Drv(drv)) => {
                stats.evaluated += 1;
                match index_entry_for(&drv, canonical_repo_dir.as_deref()) {
                    Ok(entry) => indexed.push(entry),
                    Err(reason) => {
                        debug!("{}: not indexed ({:?})", drv.attr, reason);
                        stats.record_skip(reason);
                    },
                }
            },
            Ok(NixEvalItem::Error(e)) => {
                stats.evaluated += 1;
                if e.is_function_error() {
                    stats.skipped += 1;
                    stats.skipped_other += 1;
                } else {
                    debug!(
                        "Evaluation error for {}: {}",
                        e.attr,
                        e.error.lines().last().unwrap_or(&e.error)
                    );
                    stats.errors += 1;
                }
            },
            Err(e) => {
                warn!("[{}] Stream error during indexing: {:#}", label, e);
                stats.stream_error.get_or_insert_with(|| format!("{e:#}"));
            },
        }
    }

    if stats.stream_error.is_none() && stats.evaluated == 0 {
        stats.stream_error = Some("nix-eval-jobs produced no derivations".to_owned());
    }
    stats.indexed = indexed.len();

    let prune = match prune_decision(&stats, previous) {
        Ok(()) => true,
        Err(reason) => {
            warn!("[{}] Keeping stale index entries: {}", label, reason);
            false
        },
    };
    let (entries, infos): (Vec<_>, Vec<_>) = indexed.into_iter().unzip();
    match db
        .replace_upstream_index(target.repo_name, &entries, prune)
        .await
    {
        Ok(pruned) => stats.pruned = pruned,
        Err(e) => {
            warn!("[{}] Failed to store upstream index: {:#}", label, e);
            stats
                .stream_error
                .get_or_insert_with(|| format!("store upstream index: {e:#}"));
        },
    }

    // Register every indexed package for the dashboard.
    for (entry, info) in entries.iter().zip(&infos) {
        if let Err(e) = db
            .upsert_package_info(
                target.repo_name,
                &entry.attr_path,
                &entry.current_version,
                info,
            )
            .await
        {
            debug!("{}: failed to register package: {:#}", entry.attr_path, e);
        }
    }

    info!(
        "[{}] Upstream index built: {} evaluated, {} indexed, {} skipped ({} outside repo, {} \
         without upstream, {} pinned, {} opted out, {} other), {} errors, {} pruned",
        label,
        stats.evaluated,
        stats.indexed,
        stats.skipped,
        stats.skipped_outside_repo,
        stats.skipped_no_upstream,
        stats.skipped_pinned,
        stats.skipped_opted_out,
        stats.skipped_other,
        stats.errors,
        stats.pruned
    );

    stats
}

/// Whether rows not refreshed by this run may be deleted. A run that looks
/// incomplete keeps the previous index rather than forgetting packages.
fn prune_decision(stats: &IndexStats, previous: i64) -> Result<(), String> {
    if let Some(e) = &stats.stream_error {
        return Err(format!("evaluation incomplete ({e})"));
    }
    let indexed = i64::try_from(stats.indexed).unwrap_or(i64::MAX);
    if previous >= 20 && indexed * 2 < previous {
        return Err(format!(
            "only {indexed} packages indexed (previously {previous})"
        ));
    }
    Ok(())
}

/// Turn one evaluated derivation into an index entry, or explain why not.
fn index_entry_for(
    drv: &NixEvalDrv,
    repo_dir: Option<&Path>,
) -> Result<(NewUpstreamIndexEntry, PackageInfo), SkipReason> {
    let attrs: IndexedAttrs = drv
        .extra_value
        .clone()
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or(SkipReason::NoMetadata)?;

    let version = attrs
        .version
        .clone()
        .filter(|v| !v.is_empty())
        .ok_or(SkipReason::NoVersion)?;

    if attrs.skip == Some(true) {
        return Err(SkipReason::OptedOut);
    }

    if let Some(repo_dir) = repo_dir {
        let file = attrs
            .position
            .as_deref()
            .and_then(|p| p.rsplit_once(':').map(|(file, _line)| file))
            .ok_or(SkipReason::NoPosition)?;
        if !is_within(Path::new(file), repo_dir) {
            return Err(SkipReason::OutsideRepo);
        }
    }

    let metadata = PackageMetadata {
        version: version.clone(),
        src_url: attrs.src_url.clone().filter(|u| !u.is_empty()),
        output_hash: None,
        cargo_hash: None,
        vendor_hash: None,
        npm_deps_hash: None,
        nuget_deps_hash: None,
        composer_deps_hash: None,
        pname: attrs.pname.clone(),
        description: attrs.description.clone(),
        homepage: attrs.homepage.clone(),
        changelog: None,
        skip: attrs.skip,
        semver_strategy: None,
        include_prereleases: attrs.include_prereleases,
        version_regex: attrs.version_regex.clone(),
        platform_hashes: None,
        github_repo: attrs.github_repo.clone(),
        keep: None,
        update_script: attrs.update_script.clone(),
    };
    // Guessing a PyPI project from the pname is only sound for Python
    // packages; otherwise e.g. `binutils` would track an unrelated PyPI project.
    let source = metadata
        .resolve_upstream_source_with(attrs.is_python == Some(true))
        .map_err(|_| SkipReason::NoUpstream)?;

    let policy = version_policy(&drv.attr, &attrs).ok_or(SkipReason::Pinned)?;

    let info = PackageInfo::from_metadata(&metadata, Some(&source), Some(policy.strategy));
    let entry = NewUpstreamIndexEntry {
        attr_path: drv.attr.clone(),
        pname: attrs.pname.clone(),
        upstream_type: source.source_type().to_owned(),
        upstream_key: source.source_key(),
        instance: source.instance().map(str::to_owned),
        current_version: version,
        src_url: metadata.src_url,
        drv_path: Some(drv.drv_path.clone()),
        policy,
    };
    Ok((entry, info))
}

/// Version policy for an indexed package; `None` when it is pinned.
fn version_policy(attr_path: &str, attrs: &IndexedAttrs) -> Option<VersionPolicy> {
    let explicit = attrs
        .semver_strategy
        .as_deref()
        .and_then(|s| s.parse::<SemverStrategy>().ok());
    let strategy =
        infer_update_strategy(explicit, attr_path).resolve(attrs.has_variants.unwrap_or(false))?;
    Some(VersionPolicy {
        strategy,
        include_prereleases: attrs.include_prereleases.unwrap_or(false),
        version_regex: attrs.version_regex.clone(),
    })
}

/// Whether `file` is inside `dir` (both compared as given, then canonicalized).
fn is_within(file: &Path, dir: &Path) -> bool {
    if file.starts_with(dir) {
        return true;
    }
    let canonical: Option<PathBuf> = std::fs::canonicalize(file).ok();
    canonical.is_some_and(|f| f.starts_with(dir))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn drv(attr: &str, extra: serde_json::Value) -> NixEvalDrv {
        NixEvalDrv {
            attr: attr.to_owned(),
            attr_path: attr.split('.').map(str::to_owned).collect(),
            drv_path: format!("/nix/store/aaaa-{attr}.drv"),
            input_drvs: None,
            name: attr.to_owned(),
            outputs: HashMap::new(),
            system: "x86_64-linux".to_owned(),
            meta: None,
            extra_value: Some(extra),
        }
    }

    #[test]
    fn indexes_github_package_inside_repo() {
        let d = drv(
            "zlib",
            serde_json::json!({
                "version": "1.3.2",
                "pname": "zlib",
                "srcUrl": "https://github.com/madler/zlib/releases/download/v1.3.2/zlib-1.3.2.tar.xz",
                "position": "/repos/corepkgs/pkgs/zlib/default.nix:159",
                "hasVariants": false,
            }),
        );
        let entry = index_entry_for(&d, Some(Path::new("/repos/corepkgs")))
            .unwrap()
            .0;
        assert_eq!(entry.upstream_type, "github");
        assert_eq!(entry.upstream_key, "madler/zlib");
        assert_eq!(entry.current_version, "1.3.2");
        assert_eq!(entry.drv_path.as_deref(), Some("/nix/store/aaaa-zlib.drv"));
        assert_eq!(entry.policy.strategy, SemverStrategy::Latest);
    }

    #[test]
    fn skips_packages_defined_outside_repo() {
        let d = drv(
            "zlib",
            serde_json::json!({
                "version": "1.3.2",
                "srcUrl": "https://github.com/madler/zlib/archive/v1.3.2.tar.gz",
                "position": "/nix/store/xxxx-source/pkgs/zlib/default.nix:159",
            }),
        );
        assert_eq!(
            index_entry_for(&d, Some(Path::new("/repos/ekapkgs"))).unwrap_err(),
            SkipReason::OutsideRepo
        );
        // Without a repo restriction the package is indexed.
        assert!(index_entry_for(&d, None).is_ok());
    }

    #[test]
    fn registers_package_info() {
        let d = drv(
            "zlib",
            serde_json::json!({
                "version": "1.3.2",
                "pname": "zlib",
                "description": "Lossless data-compression library",
                "homepage": "https://zlib.net",
                "srcUrl": "https://github.com/madler/zlib/archive/v1.3.2.tar.gz",
            }),
        );
        let (entry, info) = index_entry_for(&d, None).unwrap();
        assert_eq!(entry.pname.as_deref(), Some("zlib"));
        assert_eq!(info.pname.as_deref(), Some("zlib"));
        assert_eq!(
            info.description.as_deref(),
            Some("Lossless data-compression library")
        );
        assert_eq!(info.homepage.as_deref(), Some("https://zlib.net"));
        assert_eq!(info.upstream_type.as_deref(), Some("github"));
        assert_eq!(info.semver_strategy.as_deref(), Some("latest"));
    }

    #[test]
    fn prune_guard() {
        let stats = |indexed, err: Option<&str>| IndexStats {
            evaluated: indexed,
            indexed,
            stream_error: err.map(str::to_owned),
            ..IndexStats::default()
        };
        assert!(prune_decision(&stats(100, None), 120).is_ok());
        assert!(prune_decision(&stats(5, None), 10).is_ok());
        assert!(prune_decision(&stats(0, None), 0).is_ok());
        assert!(prune_decision(&stats(40, None), 100).is_err());
        assert!(prune_decision(&stats(100, Some("boom")), 100).is_err());
    }

    #[test]
    fn variant_policy_and_pinning() {
        let base = serde_json::json!({
            "version": "4.2.0",
            "srcUrl": "https://github.com/Kitware/CMake/releases/download/v4.2.0/cmake-4.2.0.tar.gz",
            "position": "/repos/corepkgs/pkgs-many/cmake/v4/package.nix:214",
            "hasVariants": true,
        });
        let repo = Some(Path::new("/repos/corepkgs"));

        let v4 = index_entry_for(&drv("cmake.v4", base.clone()), repo)
            .unwrap()
            .0;
        assert_eq!(v4.policy.strategy, SemverStrategy::Minor);

        // Default variant: no attr-path signal, but it has variants -> Minor.
        let default = index_entry_for(&drv("cmake", base.clone()), repo)
            .unwrap()
            .0;
        assert_eq!(default.policy.strategy, SemverStrategy::Minor);

        assert_eq!(
            index_entry_for(&drv("cmake.v4_2_0", base), repo).unwrap_err(),
            SkipReason::Pinned
        );
    }

    #[test]
    fn honours_passthru_settings() {
        let d = drv(
            "jq",
            serde_json::json!({
                "version": "1.7.1",
                "srcUrl": "https://example.invalid/jq.tar.gz",
                "position": "/repos/corepkgs/pkgs/jq/default.nix:10",
                "githubRepo": "jqlang/jq",
                "semverStrategy": "patch",
                "includePrereleases": true,
                "versionRegex": "jq-(.*)",
            }),
        );
        let entry = index_entry_for(&d, Some(Path::new("/repos/corepkgs")))
            .unwrap()
            .0;
        assert_eq!(entry.upstream_key, "jqlang/jq");
        assert_eq!(
            entry.policy,
            VersionPolicy {
                strategy: SemverStrategy::Patch,
                include_prereleases: true,
                version_regex: Some("jq-(.*)".to_owned()),
            }
        );

        let opted_out = drv(
            "jq",
            serde_json::json!({ "version": "1.7.1", "skip": true, "githubRepo": "jqlang/jq" }),
        );
        assert_eq!(
            index_entry_for(&opted_out, None).unwrap_err(),
            SkipReason::OptedOut
        );
    }

    #[test]
    fn pname_pypi_fallback_only_for_python_packages() {
        let repo = Some(Path::new("/repos/corepkgs"));
        let no_src = |is_python: bool| {
            drv(
                "binutils",
                serde_json::json!({
                    "version": "2.44",
                    "pname": "binutils",
                    "position": "/repos/corepkgs/pkgs/binutils/default.nix:1",
                    "isPython": is_python,
                }),
            )
        };
        assert_eq!(
            index_entry_for(&no_src(false), repo).unwrap_err(),
            SkipReason::NoUpstream
        );
        let python = index_entry_for(&no_src(true), repo).unwrap().0;
        assert_eq!(python.upstream_type, "pypi");
    }

    #[test]
    fn multiple_mirrors_use_first_recognized() {
        let d = drv(
            "curl",
            serde_json::json!({
                "version": "8.21.0",
                "pname": "curl",
                "srcUrl": "https://curl.haxx.se/download/curl-8.21.0.tar.xz https://github.com/curl/curl/releases/download/curl-8_21_0/curl-8.21.0.tar.xz",
                "position": "/repos/corepkgs/pkgs/curl/default.nix:1",
            }),
        );
        let entry = index_entry_for(&d, Some(Path::new("/repos/corepkgs")))
            .unwrap()
            .0;
        assert_eq!(entry.upstream_type, "github");
        assert_eq!(entry.upstream_key, "curl/curl");
    }

    #[test]
    fn missing_metadata_or_version_is_skipped() {
        let mut no_extra = drv("foo", serde_json::json!({}));
        no_extra.extra_value = None;
        assert_eq!(
            index_entry_for(&no_extra, None).unwrap_err(),
            SkipReason::NoMetadata
        );
        assert_eq!(
            index_entry_for(&drv("foo", serde_json::json!({ "version": "" })), None).unwrap_err(),
            SkipReason::NoVersion
        );
    }
}
