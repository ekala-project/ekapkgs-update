//! Periodic CVE scan of tracked packages against OSV.dev.
//!
//! Every `updates` row with a recognised upstream is mapped to one or more
//! OSV packages ([`osv_packages`]):
//!
//! - GitHub / GitLab / SourceHut → the `GIT` ecosystem, named by the
//!   repository URL. OSV matches the version against the repository's tags
//!   (fuzzily, so `8.4.0` finds `curl-8_4_0`). This covers C/C++ projects
//!   whose CVEs OSV imports from NVD/cvelistV5 and from project feeds.
//! - PyPI upstreams and Python packages (`python3Packages.*`, even when
//!   fetched from a forge) → the `PyPI` ecosystem (GHSA / PYSEC advisories).
//!
//! The current version and, when it differs, the latest upstream version are
//! queried through `POST /v1/querybatch` (ids only), then the full records are
//! fetched from `GET /v1/vulns/{id}` and cached by `modified`
//! (`osv_vulnerabilities`). Records that alias each other are merged
//! ([`merge_vulns`]), and each vulnerability not reported for the latest
//! version is marked as fixed by the available update.

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::{debug, info, warn};

use super::osv::best_cvss_score;
use super::store;

const OSV_API: &str = "https://api.osv.dev/v1";
/// OSV accepts at most 1000 queries per batch.
const BATCH_SIZE: usize = 1000;
/// Concurrent `GET /v1/vulns/{id}` requests.
const FETCH_CONCURRENCY: usize = 8;
/// Pagination rounds for queries with more results than one page.
const MAX_PAGES: usize = 20;

/// Upstream types [`osv_packages`] can map to an OSV package.
pub const SCANNABLE_UPSTREAMS: [&str; 4] = ["github", "gitlab", "sourcehut", "pypi"];

/// How severe a vulnerability is. Ordered from least to most severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VulnSeverity {
    /// No usable CVSS score or severity label.
    Unknown,
    Low,
    Medium,
    High,
    Critical,
}

impl VulnSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    /// Parse a stored value or an advisory's severity label (`HIGH`,
    /// `MODERATE`, `Medium`, …).
    pub fn parse(label: &str) -> Self {
        match label.trim().to_ascii_lowercase().as_str() {
            "critical" => Self::Critical,
            "high" | "important" => Self::High,
            "medium" | "moderate" => Self::Medium,
            "low" | "negligible" => Self::Low,
            _ => Self::Unknown,
        }
    }

    /// CVSS v3 qualitative rating of a base score.
    pub fn from_score(score: f64) -> Self {
        match score {
            s if s >= 9.0 => Self::Critical,
            s if s >= 7.0 => Self::High,
            s if s >= 4.0 => Self::Medium,
            s if s > 0.0 => Self::Low,
            _ => Self::Unknown,
        }
    }
}

/// A package to scan: an `updates` row.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ScanTarget {
    pub repo_name: String,
    pub attr_path: String,
    pub pname: Option<String>,
    pub upstream_type: String,
    pub upstream_key: String,
    pub upstream_instance: Option<String>,
    pub current_version: String,
    pub latest_upstream_version: Option<String>,
}

impl ScanTarget {
    /// The latest upstream version, when it differs from the current one.
    fn newer_version(&self) -> Option<&str> {
        self.latest_upstream_version
            .as_deref()
            .filter(|v| !v.is_empty() && *v != self.current_version)
    }
}

/// An OSV package identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct OsvPackage {
    pub name: String,
    pub ecosystem: &'static str,
}

impl OsvPackage {
    fn git(url: String) -> Self {
        Self {
            name: url,
            ecosystem: "GIT",
        }
    }

    fn pypi(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ecosystem: "PyPI",
        }
    }
}

/// Whether an attribute path belongs to a Python package set
/// (`python3Packages.foo`, `python313Packages.foo`).
fn is_python_attr(attr_path: &str) -> bool {
    attr_path
        .split_once('.')
        .is_some_and(|(set, _)| set.starts_with("python") && set.ends_with("Packages"))
}

/// The OSV packages describing a target (empty when none is known).
///
/// Python packages are looked up on PyPI only, even when fetched from a
/// forge: their advisories are filed there (GHSA, PYSEC), and a repository
/// query matches the package's version against the tags of every project in
/// a monorepo (`langchain-ai/langchain` hosts dozens of separately
/// versioned packages).
pub fn osv_packages(target: &ScanTarget) -> Vec<OsvPackage> {
    let key = target.upstream_key.trim_matches('/');
    if key.is_empty() {
        return Vec::new();
    }
    if target.upstream_type == "pypi" {
        return vec![OsvPackage::pypi(key)];
    }
    if is_python_attr(&target.attr_path) {
        return target
            .pname
            .as_deref()
            .filter(|p| !p.is_empty())
            .map(OsvPackage::pypi)
            .into_iter()
            .collect();
    }
    let url = match target.upstream_type.as_str() {
        "github" => format!("https://github.com/{key}"),
        "gitlab" => {
            let instance = target
                .upstream_instance
                .as_deref()
                .filter(|i| !i.is_empty())
                .unwrap_or("gitlab.com");
            format!("https://{instance}/{key}")
        },
        "sourcehut" => format!("https://git.sr.ht/{key}"),
        _ => return Vec::new(),
    };
    vec![OsvPackage::git(url)]
}

/// A vulnerability record, as fetched from OSV and cached in
/// `osv_vulnerabilities`.
#[derive(Debug, Clone, PartialEq)]
pub struct VulnInfo {
    pub id: String,
    pub modified: String,
    pub aliases: Vec<String>,
    pub summary: Option<String>,
    pub severity: VulnSeverity,
    pub cvss_score: Option<f64>,
    pub cvss_vector: Option<String>,
    pub published: Option<String>,
    pub withdrawn: Option<String>,
}

impl VulnInfo {
    /// Placeholder for a record that could not be fetched (not cached).
    fn unfetched(id: &str, modified: &str) -> Self {
        Self {
            id: id.to_owned(),
            modified: modified.to_owned(),
            aliases: Vec::new(),
            summary: None,
            severity: VulnSeverity::Unknown,
            cvss_score: None,
            cvss_vector: None,
            published: None,
            withdrawn: None,
        }
    }

    /// The record's id and aliases.
    fn keys(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.id.as_str()).chain(self.aliases.iter().map(String::as_str))
    }
}

/// One OSV record (`GET /v1/vulns/{id}`), only the fields we use.
#[derive(Debug, Deserialize)]
struct OsvRecord {
    id: String,
    #[serde(default)]
    modified: String,
    #[serde(default)]
    aliases: Vec<String>,
    summary: Option<String>,
    details: Option<String>,
    #[serde(default)]
    severity: Vec<OsvSeverity>,
    #[serde(default)]
    affected: Vec<OsvAffected>,
    database_specific: Option<Value>,
    published: Option<String>,
    withdrawn: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OsvSeverity {
    #[serde(rename = "type")]
    kind: String,
    score: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OsvAffected {
    ecosystem_specific: Option<Value>,
    database_specific: Option<Value>,
}

/// The `severity` label of a `database_specific` / `ecosystem_specific` object.
fn severity_label(value: Option<&Value>) -> Option<VulnSeverity> {
    value?
        .get("severity")?
        .as_str()
        .map(VulnSeverity::parse)
        .filter(|s| *s != VulnSeverity::Unknown)
}

impl OsvRecord {
    /// Severity, CVSS score and the vector it came from.
    ///
    /// The highest CVSS v3 score wins, then the highest CVSS v4 score
    /// ([`best_cvss_score`]); `cvss_vector` says which version it is.
    /// Otherwise the highest severity label of the record or its affected
    /// entries is used (GHSA's `database_specific.severity`, curl's `Medium`,
    /// distro labels), which also rates a CVSS score of 0.0. Records with
    /// neither are `Unknown`.
    fn severity(&self) -> (VulnSeverity, Option<f64>, Option<String>) {
        let entries: Vec<(&str, &str)> = self
            .severity
            .iter()
            .filter_map(|s| Some((s.kind.as_str(), s.score.as_deref()?)))
            .collect();
        let label = std::iter::once(severity_label(self.database_specific.as_ref()))
            .chain(self.affected.iter().flat_map(|a| {
                [
                    severity_label(a.database_specific.as_ref()),
                    severity_label(a.ecosystem_specific.as_ref()),
                ]
            }))
            .flatten()
            .max()
            .unwrap_or(VulnSeverity::Unknown);
        match best_cvss_score(&entries) {
            // A 0.0 score (no C/I/A impact in the vector) has no rating; the
            // label, if any, still does (GHSA's `LOW`).
            Some((score, raw)) => {
                let rated = VulnSeverity::from_score(score);
                let severity = if rated == VulnSeverity::Unknown {
                    label
                } else {
                    rated
                };
                (severity, Some(score), Some(raw.to_owned()))
            },
            None => (label, None, None),
        }
    }

    fn into_info(self) -> VulnInfo {
        let (severity, cvss_score, cvss_vector) = self.severity();
        let summary = self
            .summary
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                self.details
                    .as_deref()
                    .and_then(|d| d.lines().map(str::trim).find(|l| !l.is_empty()))
                    .map(str::to_owned)
            })
            .map(|s| s.trim().to_owned());
        VulnInfo {
            id: self.id,
            modified: self.modified,
            aliases: self.aliases,
            summary,
            severity,
            cvss_score,
            cvss_vector,
            published: self.published,
            withdrawn: self.withdrawn,
        }
    }
}

/// Distinct vulnerability affecting a package: OSV records that alias each
/// other, merged.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedVuln {
    /// The CVE id when the group has one, else the preferred OSV id.
    pub vuln_id: String,
    /// The merged OSV record ids (sorted).
    pub osv_ids: Vec<String>,
    pub severity: VulnSeverity,
    pub cvss_score: Option<f64>,
    pub summary: Option<String>,
    /// `Some(true)` when the latest upstream version is not affected.
    pub fixed_in_latest: Option<bool>,
}

/// Preference for an id as the canonical one / as the summary source.
fn id_rank(id: &str) -> u8 {
    if id.starts_with("CVE-") {
        0
    } else if id.starts_with("GHSA-") {
        1
    } else {
        2
    }
}

/// Merge records sharing an id or alias into one vulnerability each. The
/// result is sorted by severity (most severe first), then id.
pub fn merge_vulns(records: &[VulnInfo]) -> Vec<MergedVuln> {
    // Union-find over records, joined through shared ids/aliases.
    let mut parent: Vec<usize> = (0..records.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut owner: HashMap<&str, usize> = HashMap::new();
    for (i, record) in records.iter().enumerate() {
        for key in record.keys() {
            if let Some(&j) = owner.get(key) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a] = b;
            } else {
                owner.insert(key, i);
            }
        }
    }
    let mut groups: HashMap<usize, Vec<&VulnInfo>> = HashMap::new();
    for (i, record) in records.iter().enumerate() {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(record);
    }

    let mut merged: Vec<MergedVuln> = groups
        .into_values()
        .map(|mut group| {
            group.sort_by(|a, b| (id_rank(&a.id), &a.id).cmp(&(id_rank(&b.id), &b.id)));
            let vuln_id = group
                .iter()
                .flat_map(|r| r.keys())
                .min_by(|a, b| (id_rank(a), *a).cmp(&(id_rank(b), *b)))
                .unwrap_or_default()
                .to_owned();
            // GHSA summaries are short titles; CVE records often only have
            // a long description, so prefer GHSA, then others, then CVE.
            let summary = group
                .iter()
                .filter(|r| r.summary.is_some())
                .min_by_key(|r| match id_rank(&r.id) {
                    1 => 0,
                    0 => 2,
                    _ => 1,
                })
                .and_then(|r| r.summary.clone());
            MergedVuln {
                vuln_id,
                osv_ids: group
                    .iter()
                    .map(|r| r.id.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                severity: group
                    .iter()
                    .map(|r| r.severity)
                    .max()
                    .unwrap_or(VulnSeverity::Unknown),
                cvss_score: group
                    .iter()
                    .filter_map(|r| r.cvss_score)
                    .max_by(f64::total_cmp),
                summary,
                fixed_in_latest: None,
            }
        })
        .collect();
    merged.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(
                b.cvss_score
                    .unwrap_or(0.0)
                    .total_cmp(&a.cvss_score.unwrap_or(0.0)),
            )
            .then_with(|| a.vuln_id.cmp(&b.vuln_id))
    });
    merged
}

/// Set `fixed_in_latest` from the records reported for the latest version:
/// a vulnerability is fixed when none of its ids or aliases is among them.
pub fn mark_fixed_in_latest(vulns: &mut [MergedVuln], latest: Option<&[VulnInfo]>) {
    let Some(latest) = latest else {
        return;
    };
    let keys: HashSet<&str> = latest.iter().flat_map(VulnInfo::keys).collect();
    for vuln in vulns {
        let still = std::iter::once(vuln.vuln_id.as_str())
            .chain(vuln.osv_ids.iter().map(String::as_str))
            .any(|id| keys.contains(id));
        vuln.fixed_in_latest = Some(!still);
    }
}

// ── OSV API ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct OsvQuery {
    package: OsvPackage,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct BatchRequest<'a> {
    queries: &'a [OsvQuery],
}

#[derive(Debug, Deserialize)]
struct BatchResponse {
    #[serde(default)]
    results: Vec<BatchResult>,
}

#[derive(Debug, Default, Deserialize)]
struct BatchResult {
    #[serde(default)]
    vulns: Vec<BatchVuln>,
    next_page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BatchVuln {
    id: String,
    #[serde(default)]
    modified: String,
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("ekapkgs-update/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("build HTTP client")
}

async fn post_batch(client: &reqwest::Client, queries: &[OsvQuery]) -> Result<Vec<BatchResult>> {
    let url = format!("{OSV_API}/querybatch");
    let response = client
        .post(&url)
        .json(&BatchRequest { queries })
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("POST {url}: HTTP {status}");
    }
    let body: BatchResponse = response
        .json()
        .await
        .with_context(|| format!("decode JSON from {url}"))?;
    if body.results.len() != queries.len() {
        anyhow::bail!(
            "POST {url}: {} results for {} queries",
            body.results.len(),
            queries.len()
        );
    }
    Ok(body.results)
}

/// Run one batch (≤ [`BATCH_SIZE`] queries), following pagination. Returns
/// the reported `(id, modified)` per query.
async fn query_batch(
    client: &reqwest::Client,
    queries: &[OsvQuery],
) -> Result<Vec<Vec<BatchVuln>>> {
    let first = post_batch(client, queries).await?;
    let mut tokens: Vec<Option<String>> = Vec::with_capacity(queries.len());
    let mut found: Vec<Vec<BatchVuln>> = Vec::with_capacity(queries.len());
    for result in first {
        tokens.push(result.next_page_token);
        found.push(result.vulns);
    }
    for _ in 0..MAX_PAGES {
        let pending: Vec<usize> = (0..queries.len())
            .filter(|&i| tokens[i].is_some())
            .collect();
        if pending.is_empty() {
            break;
        }
        let page: Vec<OsvQuery> = pending
            .iter()
            .map(|&i| OsvQuery {
                page_token: tokens[i].take(),
                ..queries[i].clone()
            })
            .collect();
        for (i, result) in pending.into_iter().zip(post_batch(client, &page).await?) {
            tokens[i] = result.next_page_token;
            found[i].extend(result.vulns);
        }
    }
    Ok(found)
}

/// `GET /v1/vulns/{id}`. `Ok(None)` when OSV no longer has the record.
async fn fetch_record(client: &reqwest::Client, id: &str) -> Result<Option<VulnInfo>> {
    let url = format!("{OSV_API}/vulns/{id}");
    let mut attempt = 0;
    loop {
        attempt += 1;
        let response = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if (status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
            && attempt < 3
        {
            tokio::time::sleep(std::time::Duration::from_secs(2 * attempt)).await;
            continue;
        }
        if !status.is_success() {
            anyhow::bail!("GET {url}: HTTP {status}");
        }
        let record: OsvRecord = response
            .json()
            .await
            .with_context(|| format!("decode JSON from {url}"))?;
        return Ok(Some(record.into_info()));
    }
}

// ── Scan ────────────────────────────────────────────────────────────

/// What to scan.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Only this repository (`""` = the default repo); `None` = all.
    pub repo: Option<String>,
    /// Only these attribute paths (empty = all).
    pub attrs: Vec<String>,
    /// Rescan packages scanned longer ago than this (or whose version
    /// changed since).
    pub max_age: Duration,
    /// Rescan everything selected, regardless of age.
    pub force: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            repo: None,
            attrs: Vec::new(),
            max_age: Duration::hours(24),
            force: false,
        }
    }
}

/// Outcome of [`scan_due`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// Packages scanned successfully.
    pub scanned: usize,
    /// Packages whose queries failed (state left as it was).
    pub failed: usize,
    /// Scanned packages with at least one vulnerability.
    pub vulnerable: usize,
    /// Scanned packages with a high or critical vulnerability.
    pub high_or_critical: usize,
    /// OSV records fetched (not served from the cache).
    pub records_fetched: usize,
}

/// Scan the packages that are due (never scanned, scanned more than
/// `max_age` ago, or whose current version changed since), recording the
/// results in `package_vulnerabilities`.
///
/// # Errors
/// Database errors. OSV failures are recorded per package
/// (`updates.cve_scan_error`) and counted in [`ScanSummary::failed`].
pub async fn scan_due(pool: &SqlitePool, options: &ScanOptions) -> Result<ScanSummary> {
    let stale_before = (Utc::now() - options.max_age).to_rfc3339();
    let mut targets =
        store::due_targets(pool, options.repo.as_deref(), &stale_before, options.force).await?;
    if !options.attrs.is_empty() {
        targets.retain(|t| options.attrs.contains(&t.attr_path));
    }
    if targets.is_empty() {
        debug!("CVE scan: nothing due");
        return Ok(ScanSummary::default());
    }
    info!("CVE scan: querying OSV.dev for {} packages", targets.len());
    scan_targets(pool, &targets).await
}

/// One OSV query, tied to the target and version it describes.
struct PlannedQuery {
    target: usize,
    latest: bool,
    query: OsvQuery,
}

fn plan_queries(targets: &[ScanTarget]) -> Vec<PlannedQuery> {
    let mut planned = Vec::new();
    for (i, target) in targets.iter().enumerate() {
        for package in osv_packages(target) {
            let versions = std::iter::once((false, target.current_version.as_str()))
                .chain(target.newer_version().map(|v| (true, v)));
            for (latest, version) in versions {
                planned.push(PlannedQuery {
                    target: i,
                    latest,
                    query: OsvQuery {
                        package: package.clone(),
                        version: version.to_owned(),
                        page_token: None,
                    },
                });
            }
        }
    }
    planned
}

async fn scan_targets(pool: &SqlitePool, targets: &[ScanTarget]) -> Result<ScanSummary> {
    let client = http_client()?;
    let planned = plan_queries(targets);

    // 1. Batch queries: which ids affect each (target, version).
    let mut current_ids: Vec<Vec<BatchVuln>> = vec![Vec::new(); targets.len()];
    let mut latest_ids: Vec<Vec<BatchVuln>> = vec![Vec::new(); targets.len()];
    let mut failed: HashMap<usize, String> = HashMap::new();
    for chunk in planned.chunks(BATCH_SIZE) {
        let queries: Vec<OsvQuery> = chunk.iter().map(|p| p.query.clone()).collect();
        match query_batch(&client, &queries).await {
            Ok(results) => {
                for (plan, vulns) in chunk.iter().zip(results) {
                    let slot = if plan.latest {
                        &mut latest_ids[plan.target]
                    } else {
                        &mut current_ids[plan.target]
                    };
                    slot.extend(vulns);
                }
            },
            Err(e) => {
                warn!("CVE scan: OSV batch query failed: {:#}", e);
                for plan in chunk {
                    failed.insert(plan.target, format!("{e:#}"));
                }
            },
        }
    }

    // 2. Records: cached unless OSV reports a different `modified`.
    let mut wanted: HashMap<String, String> = HashMap::new();
    for vuln in current_ids.iter().chain(&latest_ids).flatten() {
        wanted
            .entry(vuln.id.clone())
            .or_insert_with(|| vuln.modified.clone());
    }
    let ids: Vec<String> = wanted.keys().cloned().collect();
    let mut records = store::load_vulns(pool, &ids).await?;
    let stale: Vec<String> = wanted
        .iter()
        .filter(|(id, modified)| records.get(*id).is_none_or(|r| &r.modified != *modified))
        .map(|(id, _)| id.clone())
        .collect();
    let mut fetched = Vec::new();
    let mut results = futures::stream::iter(stale.iter().map(|id| {
        let client = &client;
        async move { (id, fetch_record(client, id).await) }
    }))
    .buffer_unordered(FETCH_CONCURRENCY);
    while let Some((id, result)) = results.next().await {
        match result {
            Ok(Some(info)) => fetched.push(info),
            Ok(None) => debug!("CVE scan: OSV has no record {id}"),
            Err(e) => warn!("CVE scan: fetching {id} failed: {:#}", e),
        }
    }
    drop(results);
    store::store_vulns(pool, &fetched).await?;
    let records_fetched = fetched.len();
    for info in fetched {
        records.insert(info.id.clone(), info);
    }

    let resolve = |vulns: &[BatchVuln]| -> Vec<VulnInfo> {
        let mut seen = HashSet::new();
        vulns
            .iter()
            .filter(|v| seen.insert(v.id.as_str()))
            .map(|v| {
                records
                    .get(&v.id)
                    .cloned()
                    .unwrap_or_else(|| VulnInfo::unfetched(&v.id, &v.modified))
            })
            .filter(|r| r.withdrawn.is_none())
            .collect()
    };

    // 3. Per package: merge, compare with the latest version, record.
    let now = Utc::now().to_rfc3339();
    let mut summary = ScanSummary {
        records_fetched,
        ..ScanSummary::default()
    };
    for (i, target) in targets.iter().enumerate() {
        if let Some(error) = failed.get(&i) {
            store::record_scan_error(pool, target, error).await?;
            summary.failed += 1;
            continue;
        }
        let mut vulns = merge_vulns(&resolve(&current_ids[i]));
        let latest = target.newer_version().map(|_| resolve(&latest_ids[i]));
        mark_fixed_in_latest(&mut vulns, latest.as_deref());
        store::record_scan(pool, target, &vulns, &now).await?;
        summary.scanned += 1;
        if !vulns.is_empty() {
            summary.vulnerable += 1;
        }
        if vulns.iter().any(|v| v.severity >= VulnSeverity::High) {
            summary.high_or_critical += 1;
        }
    }

    info!(
        "CVE scan: {} packages scanned ({} failed), {} with known vulnerabilities, {} high or \
         critical; {} OSV records fetched",
        summary.scanned,
        summary.failed,
        summary.vulnerable,
        summary.high_or_critical,
        summary.records_fetched
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(kind: &str, key: &str, attr: &str) -> ScanTarget {
        ScanTarget {
            repo_name: String::new(),
            attr_path: attr.to_owned(),
            pname: Some("foo".to_owned()),
            upstream_type: kind.to_owned(),
            upstream_key: key.to_owned(),
            upstream_instance: None,
            current_version: "1.0".to_owned(),
            latest_upstream_version: None,
        }
    }

    fn info(id: &str, aliases: &[&str], severity: VulnSeverity, score: Option<f64>) -> VulnInfo {
        VulnInfo {
            id: id.to_owned(),
            modified: "2026-01-01T00:00:00Z".to_owned(),
            aliases: aliases.iter().map(|a| (*a).to_owned()).collect(),
            summary: Some(format!("summary of {id}")),
            severity,
            cvss_score: score,
            cvss_vector: None,
            published: None,
            withdrawn: None,
        }
    }

    #[test]
    fn osv_packages_by_upstream() {
        let mut gitlab = target("gitlab", "GNOME/libxml2", "libxml2");
        gitlab.upstream_instance = Some("gitlab.gnome.org".to_owned());
        let cases: Vec<(ScanTarget, Vec<(&str, &str)>)> = vec![
            (
                target("github", "curl/curl", "curl"),
                vec![("GIT", "https://github.com/curl/curl")],
            ),
            (
                gitlab,
                vec![("GIT", "https://gitlab.gnome.org/GNOME/libxml2")],
            ),
            (
                target("gitlab", "a/b/c", "c"),
                vec![("GIT", "https://gitlab.com/a/b/c")],
            ),
            (
                target("sourcehut", "~sircmpwn/scdoc", "scdoc"),
                vec![("GIT", "https://git.sr.ht/~sircmpwn/scdoc")],
            ),
            (
                target("pypi", "Django", "python3Packages.django"),
                vec![("PyPI", "Django")],
            ),
            // Python packages on a forge: PyPI only, by pname.
            (
                target("github", "django/django", "python3Packages.django"),
                vec![("PyPI", "foo")],
            ),
            (
                target("github", "o/r", "python313Packages.foo"),
                vec![("PyPI", "foo")],
            ),
            // Not a Python package set.
            (
                target("github", "o/r", "pythonFull"),
                vec![("GIT", "https://github.com/o/r")],
            ),
            (
                target("directory", "https://ftp.gnu.org/gnu/hello/", "hello"),
                vec![],
            ),
            (target("github", "", "x"), vec![]),
        ];
        for (t, expected) in cases {
            let got: Vec<(&str, String)> = osv_packages(&t)
                .into_iter()
                .map(|p| (p.ecosystem, p.name))
                .collect();
            let expected: Vec<(&str, String)> = expected
                .into_iter()
                .map(|(e, n)| (e, n.to_owned()))
                .collect();
            assert_eq!(got, expected, "{} {}", t.upstream_type, t.attr_path);
        }
    }

    #[test]
    fn plan_queries_includes_newer_version() {
        let mut t = target("github", "o/r", "r");
        assert_eq!(plan_queries(std::slice::from_ref(&t)).len(), 1);
        t.latest_upstream_version = Some("1.0".to_owned());
        assert_eq!(plan_queries(std::slice::from_ref(&t)).len(), 1);
        t.latest_upstream_version = Some("2.0".to_owned());
        let planned = plan_queries(std::slice::from_ref(&t));
        let versions: Vec<(bool, &str)> = planned
            .iter()
            .map(|p| (p.latest, p.query.version.as_str()))
            .collect();
        assert_eq!(versions, vec![(false, "1.0"), (true, "2.0")]);
    }

    #[test]
    fn severity_labels() {
        for (label, expected) in [
            ("CRITICAL", VulnSeverity::Critical),
            ("HIGH", VulnSeverity::High),
            ("Important", VulnSeverity::High),
            ("MODERATE", VulnSeverity::Medium),
            ("Medium", VulnSeverity::Medium),
            ("low", VulnSeverity::Low),
            ("", VulnSeverity::Unknown),
            ("whatever", VulnSeverity::Unknown),
        ] {
            assert_eq!(VulnSeverity::parse(label), expected, "{label}");
        }
        for (score, expected) in [
            (9.8, VulnSeverity::Critical),
            (7.0, VulnSeverity::High),
            (6.9, VulnSeverity::Medium),
            (0.1, VulnSeverity::Low),
            (0.0, VulnSeverity::Unknown),
        ] {
            assert_eq!(VulnSeverity::from_score(score), expected, "{score}");
        }
    }

    fn record(json: &str) -> VulnInfo {
        serde_json::from_str::<OsvRecord>(json).unwrap().into_info()
    }

    #[test]
    fn record_severity_sources() {
        // CVSS v3 vector wins over labels.
        let r = record(
            r#"{"id":"CVE-1","modified":"m","severity":[{"type":"CVSS_V3",
            "score":"CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"}],
            "database_specific":{"severity":"LOW"}}"#,
        );
        assert_eq!(r.severity, VulnSeverity::Critical);
        assert_eq!(r.cvss_score, Some(9.8));
        assert!(r.cvss_vector.unwrap().starts_with("CVSS:3.1/"));

        // GHSA label.
        let r = record(r#"{"id":"GHSA-x","database_specific":{"severity":"HIGH"}}"#);
        assert_eq!((r.severity, r.cvss_score), (VulnSeverity::High, None));

        // Highest label across affected entries.
        let r = record(
            r#"{"id":"X","affected":[{"ecosystem_specific":{"severity":"low"}},
            {"database_specific":{"severity":"Medium"}}]}"#,
        );
        assert_eq!(r.severity, VulnSeverity::Medium);

        // CVSS v4 only: scored by `cvss4`, winning over labels.
        let r = record(
            r#"{"id":"Y","severity":[{"type":"CVSS_V4",
            "score":"CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N"}],
            "database_specific":{"severity":"LOW"}}"#,
        );
        assert_eq!(
            (r.severity, r.cvss_score),
            (VulnSeverity::Critical, Some(9.3))
        );
        assert!(r.cvss_vector.unwrap().starts_with("CVSS:4.0/"));

        // CVSS v3 is preferred over a (higher) v4 score.
        let r = record(
            r#"{"id":"Z","severity":[
            {"type":"CVSS_V4","score":"CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N"},
            {"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:L/UI:N/S:U/C:H/I:H/A:N"}]}"#,
        );
        assert_eq!((r.severity, r.cvss_score), (VulnSeverity::High, Some(8.1)));

        // A 0.0 CVSS score keeps its score but takes the label's rating.
        let r = record(
            r#"{"id":"V","severity":[{"type":"CVSS_V3",
            "score":"CVSS:3.1/AV:N/AC:L/PR:L/UI:N/S:U/C:N/I:N/A:N"}],
            "database_specific":{"severity":"LOW"}}"#,
        );
        assert_eq!((r.severity, r.cvss_score), (VulnSeverity::Low, Some(0.0)));

        // An invalid v4 vector falls through to the label.
        let r = record(
            r#"{"id":"W","severity":[{"type":"CVSS_V4","score":"CVSS:4.0/AV:N"}],
            "database_specific":{"severity":"MODERATE"}}"#,
        );
        assert_eq!((r.severity, r.cvss_score), (VulnSeverity::Medium, None));
        assert_eq!(r.cvss_vector, None);
    }

    #[test]
    fn record_summary_falls_back_to_details() {
        let r = record(r#"{"id":"A","details":"\n  First line.\nSecond."}"#);
        assert_eq!(r.summary.as_deref(), Some("First line."));
        let r = record(r#"{"id":"A","summary":"  ","details":"D"}"#);
        assert_eq!(r.summary.as_deref(), Some("D"));
        assert_eq!(record(r#"{"id":"A"}"#).summary, None);
    }

    #[test]
    fn merge_groups_aliases() {
        let records = vec![
            info(
                "CURL-CVE-2023-38545",
                &["CVE-2023-38545"],
                VulnSeverity::Medium,
                None,
            ),
            info(
                "CVE-2023-38545",
                &["CURL-CVE-2023-38545"],
                VulnSeverity::Critical,
                Some(9.8),
            ),
            info(
                "GHSA-aaaa",
                &["CVE-2024-1", "PYSEC-1"],
                VulnSeverity::High,
                None,
            ),
            info("PYSEC-1", &["CVE-2024-1"], VulnSeverity::Unknown, Some(7.5)),
            info("GHSA-bbbb", &[], VulnSeverity::Low, None),
        ];
        let merged = merge_vulns(&records);
        let summary: Vec<(&str, Vec<&str>, VulnSeverity, Option<f64>)> = merged
            .iter()
            .map(|m| {
                (
                    m.vuln_id.as_str(),
                    m.osv_ids.iter().map(String::as_str).collect(),
                    m.severity,
                    m.cvss_score,
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "CVE-2023-38545",
                    vec!["CURL-CVE-2023-38545", "CVE-2023-38545"],
                    VulnSeverity::Critical,
                    Some(9.8)
                ),
                (
                    "CVE-2024-1",
                    vec!["GHSA-aaaa", "PYSEC-1"],
                    VulnSeverity::High,
                    Some(7.5)
                ),
                ("GHSA-bbbb", vec!["GHSA-bbbb"], VulnSeverity::Low, None),
            ]
        );
        // GHSA summaries are preferred.
        assert_eq!(merged[1].summary.as_deref(), Some("summary of GHSA-aaaa"));
    }

    #[test]
    fn merge_joins_groups_transitively() {
        // A–B through an alias, B–C through another: one group.
        let records = vec![
            info("A", &["X"], VulnSeverity::Low, None),
            info("C", &["Y"], VulnSeverity::Low, None),
            info("B", &["X", "Y"], VulnSeverity::Low, None),
        ];
        let merged = merge_vulns(&records);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].osv_ids, vec!["A", "B", "C"]);
    }

    #[test]
    fn fixed_in_latest() {
        let current = vec![
            info("CVE-1", &[], VulnSeverity::High, None),
            info("GHSA-2", &["CVE-2"], VulnSeverity::High, None),
        ];
        let mut merged = merge_vulns(&current);
        mark_fixed_in_latest(&mut merged, None);
        assert!(merged.iter().all(|m| m.fixed_in_latest.is_none()));

        // The latest version still reports CVE-2 (under another record id).
        let latest = vec![info("PYSEC-9", &["CVE-2"], VulnSeverity::High, None)];
        mark_fixed_in_latest(&mut merged, Some(&latest));
        let fixed: Vec<(&str, Option<bool>)> = merged
            .iter()
            .map(|m| (m.vuln_id.as_str(), m.fixed_in_latest))
            .collect();
        assert_eq!(fixed, vec![("CVE-1", Some(true)), ("CVE-2", Some(false))]);
    }
}
