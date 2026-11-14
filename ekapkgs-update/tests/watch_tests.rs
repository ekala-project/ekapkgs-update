//! Integration tests for the `watch` command pipeline.
//!
//! Tests cover the upstream index database operations, Atom/RSS feed parsing,
//! and the dispatch logic that detects new versions and emits `UpdateRequest`s.
//! All tests use in-memory SQLite databases for isolation.

use std::collections::HashSet;

use sqlx::SqlitePool;
use tokio::sync::mpsc;

use ekapkgs_update::commands::run::UpdateRequest;
use ekapkgs_update::commands::watch::feeds::{parse_feed_bytes, upstream_source_from_index};
use ekapkgs_update::database::Database;
use ekapkgs_update::vcs_sources::UpstreamSource;

/// Create an in-memory database with migrations applied.
async fn setup_db() -> Database {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    Database::from(pool)
}

// ── Upstream Index Database Tests ─────────────────────────────────

#[tokio::test]
async fn upstream_index_upsert_and_query() {
    let db = setup_db().await;

    // Insert a GitHub source
    db.upsert_upstream_index(
        "",
        "ripgrep",
        "github",
        "BurntSushi/ripgrep",
        None,
        "14.1.0",
        Some("https://github.com/BurntSushi/ripgrep/archive/14.1.0.tar.gz"),
    )
    .await
    .unwrap();

    // Query it back
    let entries = db
        .get_attr_paths_for_source("github", "BurntSushi/ripgrep", None)
        .await
        .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].attr_path, "ripgrep");
    assert_eq!(entries[0].upstream_type, "github");
    assert_eq!(entries[0].upstream_key, "BurntSushi/ripgrep");
    assert_eq!(entries[0].current_version.as_deref(), Some("14.1.0"));
    assert!(entries[0].instance.is_none());
}

#[tokio::test]
async fn upstream_index_multiple_attr_paths_same_source() {
    let db = setup_db().await;

    // Two packages from the same upstream (e.g., python312Packages.foo and
    // python313Packages.foo sharing the same source)
    db.upsert_upstream_index(
        "",
        "python312Packages.requests",
        "pypi",
        "requests",
        None,
        "2.31.0",
        None,
    )
    .await
    .unwrap();
    db.upsert_upstream_index(
        "",
        "python313Packages.requests",
        "pypi",
        "requests",
        None,
        "2.31.0",
        None,
    )
    .await
    .unwrap();

    let entries = db
        .get_attr_paths_for_source("pypi", "requests", None)
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);

    let attr_paths: Vec<&str> = entries.iter().map(|e| e.attr_path.as_str()).collect();
    assert!(attr_paths.contains(&"python312Packages.requests"));
    assert!(attr_paths.contains(&"python313Packages.requests"));
}

#[tokio::test]
async fn upstream_index_gitlab_with_instance() {
    let db = setup_db().await;

    db.upsert_upstream_index(
        "",
        "mesa",
        "gitlab",
        "mesa/mesa",
        Some("gitlab.freedesktop.org"),
        "24.1.0",
        None,
    )
    .await
    .unwrap();

    // Query without instance should still match (NULL handling)
    let no_instance = db
        .get_attr_paths_for_source("gitlab", "mesa/mesa", None)
        .await
        .unwrap();
    assert_eq!(no_instance.len(), 0); // NULL != 'gitlab.freedesktop.org'

    // Query with correct instance
    let with_instance = db
        .get_attr_paths_for_source("gitlab", "mesa/mesa", Some("gitlab.freedesktop.org"))
        .await
        .unwrap();
    assert_eq!(with_instance.len(), 1);
    assert_eq!(with_instance[0].attr_path, "mesa");
}

#[tokio::test]
async fn upstream_index_upsert_updates_version() {
    let db = setup_db().await;

    db.upsert_upstream_index("", "jq", "github", "jqlang/jq", None, "1.6", None)
        .await
        .unwrap();

    // Upsert with new version
    db.upsert_upstream_index("", "jq", "github", "jqlang/jq", None, "1.7.1", None)
        .await
        .unwrap();

    let entries = db
        .get_attr_paths_for_source("github", "jqlang/jq", None)
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].current_version.as_deref(), Some("1.7.1"));
}

#[tokio::test]
async fn upstream_index_get_all_unique_sources() {
    let db = setup_db().await;

    // Multiple packages, some sharing upstream
    db.upsert_upstream_index(
        "",
        "ripgrep",
        "github",
        "BurntSushi/ripgrep",
        None,
        "14.1.0",
        None,
    )
    .await
    .unwrap();
    db.upsert_upstream_index(
        "",
        "python3Packages.requests",
        "pypi",
        "requests",
        None,
        "2.31.0",
        None,
    )
    .await
    .unwrap();
    db.upsert_upstream_index(
        "",
        "python3Packages.flask",
        "pypi",
        "flask",
        None,
        "3.0.0",
        None,
    )
    .await
    .unwrap();
    // Alias for the same upstream
    db.upsert_upstream_index(
        "",
        "rg",
        "github",
        "BurntSushi/ripgrep",
        None,
        "14.1.0",
        None,
    )
    .await
    .unwrap();

    let sources = db.get_all_upstream_sources().await.unwrap();
    // 3 unique sources: BurntSushi/ripgrep, requests, flask
    assert_eq!(sources.len(), 3);
}

#[tokio::test]
async fn upstream_index_clear() {
    let db = setup_db().await;

    db.upsert_upstream_index("", "jq", "github", "jqlang/jq", None, "1.7", None)
        .await
        .unwrap();
    assert_eq!(db.upstream_index_count(None).await.unwrap(), 1);

    db.clear_upstream_index(None).await.unwrap();
    assert_eq!(db.upstream_index_count(None).await.unwrap(), 0);
}

// ── Feed Parsing Tests ────────────────────────────────────────────

/// Minimal GitHub releases.atom fixture
const GITHUB_ATOM_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Release notes from ripgrep</title>
  <entry>
    <title>14.1.1</title>
    <link href="https://github.com/BurntSushi/ripgrep/releases/tag/14.1.1"/>
    <updated>2024-09-01T00:00:00Z</updated>
  </entry>
  <entry>
    <title>14.1.0</title>
    <link href="https://github.com/BurntSushi/ripgrep/releases/tag/14.1.0"/>
    <updated>2024-08-01T00:00:00Z</updated>
  </entry>
  <entry>
    <title>14.0.3</title>
    <link href="https://github.com/BurntSushi/ripgrep/releases/tag/14.0.3"/>
    <updated>2024-07-01T00:00:00Z</updated>
  </entry>
</feed>"#;

#[test]
fn parse_github_atom_feed() {
    let releases = parse_feed_bytes(GITHUB_ATOM_FIXTURE.as_bytes()).unwrap();
    assert_eq!(releases.len(), 3);
    assert_eq!(releases[0].tag_name, "14.1.1");
    assert_eq!(releases[1].tag_name, "14.1.0");
    assert_eq!(releases[2].tag_name, "14.0.3");
    assert!(releases.iter().all(|r| !r.is_prerelease));
}

/// Minimal PyPI RSS fixture
const PYPI_RSS_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>PyPI recent updates for requests</title>
    <item>
      <title>2.32.0</title>
      <link>https://pypi.org/project/requests/2.32.0/</link>
    </item>
    <item>
      <title>2.31.0</title>
      <link>https://pypi.org/project/requests/2.31.0/</link>
    </item>
  </channel>
</rss>"#;

#[test]
fn parse_pypi_rss_feed() {
    let releases = parse_feed_bytes(PYPI_RSS_FIXTURE.as_bytes()).unwrap();
    assert_eq!(releases.len(), 2);
    assert_eq!(releases[0].tag_name, "2.32.0");
    assert_eq!(releases[1].tag_name, "2.31.0");
}

#[test]
fn parse_empty_feed() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Empty feed</title>
</feed>"#;
    let releases = parse_feed_bytes(xml.as_bytes()).unwrap();
    assert!(releases.is_empty());
}

#[test]
fn parse_feed_entries_without_title_uses_link() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Test</title>
  <entry>
    <link href="https://example.com/releases/tag/v3.0.0"/>
  </entry>
</feed>"#;
    let releases = parse_feed_bytes(xml.as_bytes()).unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].tag_name, "v3.0.0");
}

#[test]
fn parse_feed_invalid_xml_returns_error() {
    let result = parse_feed_bytes(b"this is not xml");
    assert!(result.is_err());
}

// ── Dispatch Logic Tests ──────────────────────────────────────────

/// Test that the dispatch logic produces a correct UpdateRequest when a newer
/// version is detected. Exercises the same steps as the watch poller but
/// without network I/O (the dispatch decision itself is unit-tested in
/// `commands/watch/listener.rs`).
#[tokio::test]
async fn dispatch_sends_update_for_newer_version() {
    let db = setup_db().await;

    // Index a package at version 1.0.0
    db.upsert_upstream_index("", "my-pkg", "github", "owner/repo", None, "1.0.0", None)
        .await
        .unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();

    // Look up entries (same as the poller does after fetching releases)
    let entries = db
        .get_attr_paths_for_source("github", "owner/repo", None)
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);

    let entry = &entries[0];
    let current = entry.current_version.as_deref().unwrap_or("");
    assert_eq!(current, "1.0.0");

    // Simulate detecting version 2.0.0
    let latest_version = "2.0.0";

    // Version comparison
    assert_eq!(
        ekapkgs_update::vcs_sources::compare_version_components(latest_version, current),
        std::cmp::Ordering::Greater
    );

    // Backoff check
    assert!(db.should_check_update("", &entry.attr_path).await.unwrap());

    // Construct and send the request (same as the poller does)
    let req = UpdateRequest {
        attr_path: entry.attr_path.clone(),
        drv_path: None,
        current_version: current.to_owned(),
        new_version: latest_version.to_owned(),
        repo_name: None,
        eval_entry_point: None,
        pr_config: None,
        fork: None,
    };
    tx.send(req).unwrap();

    // Verify what was dispatched
    drop(tx);
    let received = rx.recv().await.unwrap();
    assert_eq!(received.attr_path, "my-pkg");
    assert_eq!(received.current_version, "1.0.0");
    assert_eq!(received.new_version, "2.0.0");
    assert!(received.drv_path.is_none());
    assert!(rx.recv().await.is_none()); // No more
}

/// Test that no update is dispatched when the indexed version matches upstream.
#[tokio::test]
async fn dispatch_skips_same_version() {
    let db = setup_db().await;

    db.upsert_upstream_index("", "my-pkg", "github", "owner/repo", None, "2.0.0", None)
        .await
        .unwrap();

    let entries = db
        .get_attr_paths_for_source("github", "owner/repo", None)
        .await
        .unwrap();
    let current = entries[0].current_version.as_deref().unwrap_or("");
    let latest = "2.0.0";

    // Same version — should not dispatch
    assert_eq!(current, latest);
}

/// Test that no update is dispatched when upstream is older.
#[tokio::test]
async fn dispatch_skips_older_version() {
    let db = setup_db().await;

    db.upsert_upstream_index("", "my-pkg", "github", "owner/repo", None, "3.0.0", None)
        .await
        .unwrap();

    let entries = db
        .get_attr_paths_for_source("github", "owner/repo", None)
        .await
        .unwrap();
    let current = entries[0].current_version.as_deref().unwrap_or("");
    let latest = "2.5.0";

    assert!(
        ekapkgs_update::vcs_sources::compare_version_components(latest, current)
            != std::cmp::Ordering::Greater
    );
}

/// Test that backoff prevents dispatch for recently failed packages.
#[tokio::test]
async fn dispatch_respects_backoff() {
    let db = setup_db().await;

    db.upsert_upstream_index("", "my-pkg", "github", "owner/repo", None, "1.0.0", None)
        .await
        .unwrap();

    // Record a "no update" which sets the backoff timer
    db.record_no_update("", "my-pkg", "1.0.0", Some("1.0.0"), None)
        .await
        .unwrap();

    // Now the package should be in backoff
    assert!(!db.should_check_update("", "my-pkg").await.unwrap());
}

/// Test the deduplication logic: two attr_paths sharing the same upstream
/// should only produce one UpdateRequest.
#[tokio::test]
async fn dispatch_deduplicates_aliases() {
    let db = setup_db().await;

    // Two attr_paths for the same upstream (e.g., python aliases)
    db.upsert_upstream_index(
        "",
        "python312Packages.foo",
        "pypi",
        "foo",
        None,
        "1.0.0",
        None,
    )
    .await
    .unwrap();
    db.upsert_upstream_index(
        "",
        "python313Packages.foo",
        "pypi",
        "foo",
        None,
        "1.0.0",
        None,
    )
    .await
    .unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut seen = HashSet::new();

    let entries = db
        .get_attr_paths_for_source("pypi", "foo", None)
        .await
        .unwrap();

    let latest_version = "2.0.0";
    let mut sent = 0;

    for entry in &entries {
        let current = entry.current_version.as_deref().unwrap_or("");

        if current == latest_version {
            continue;
        }

        // Dedup check — aliases share source, current and target version
        let dedup_key = (entry.upstream_key.clone(), latest_version.to_owned());
        if !seen.insert(dedup_key) {
            continue; // Already queued
        }

        let req = UpdateRequest {
            attr_path: entry.attr_path.clone(),
            drv_path: None,
            current_version: current.to_owned(),
            new_version: latest_version.to_owned(),
            repo_name: None,
            eval_entry_point: None,
            pr_config: None,
            fork: None,
        };
        tx.send(req).unwrap();
        sent += 1;
    }

    drop(tx);

    // Only one request should have been sent despite two attr_paths
    assert_eq!(sent, 1);

    let received = rx.recv().await.unwrap();
    assert_eq!(received.new_version, "2.0.0");
    assert!(rx.recv().await.is_none()); // No more
}

// ── UpstreamSource Reconstruction Tests ───────────────────────────
// (These verify the full roundtrip: source → DB index fields → source)

#[tokio::test]
async fn index_roundtrip_github() {
    let db = setup_db().await;
    let source = UpstreamSource::GitHub {
        owner: "NixOS".to_owned(),
        repo: "nixpkgs".to_owned(),
    };

    db.upsert_upstream_index(
        "",
        "nixpkgs-unstable",
        source.source_type(),
        &source.source_key(),
        source.instance(),
        "24.05",
        None,
    )
    .await
    .unwrap();

    let entries = db.get_all_upstream_sources().await.unwrap();
    assert_eq!(entries.len(), 1);

    let reconstructed = upstream_source_from_index(
        &entries[0].upstream_type,
        &entries[0].upstream_key,
        entries[0].instance.as_deref(),
    )
    .unwrap();

    assert_eq!(reconstructed.source_type(), "github");
    assert_eq!(reconstructed.source_key(), "NixOS/nixpkgs");
    assert!(!reconstructed.feed_urls().is_empty());
}

#[tokio::test]
async fn index_roundtrip_gitlab_with_instance() {
    let db = setup_db().await;
    let source = UpstreamSource::GitLab {
        instance: "gitlab.gnome.org".to_owned(),
        owner: "GNOME".to_owned(),
        project: "glib".to_owned(),
    };

    db.upsert_upstream_index(
        "",
        "glib",
        source.source_type(),
        &source.source_key(),
        source.instance(),
        "2.80.0",
        None,
    )
    .await
    .unwrap();

    let entries = db
        .get_attr_paths_for_source("gitlab", "GNOME/glib", Some("gitlab.gnome.org"))
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);

    let reconstructed = upstream_source_from_index(
        &entries[0].upstream_type,
        &entries[0].upstream_key,
        entries[0].instance.as_deref(),
    )
    .unwrap();

    assert_eq!(reconstructed.source_type(), "gitlab");
    assert_eq!(reconstructed.instance(), Some("gitlab.gnome.org"));
}

// ── Index replacement, version policy and dispatch guards ─────────

use ekapkgs_update::commands::watch::feeds::update_version_for_entry;
use ekapkgs_update::database::NewUpstreamIndexEntry;
use ekapkgs_update::vcs_sources::{Release, SemverStrategy, VersionPolicy};

fn new_entry(
    attr_path: &str,
    key: &str,
    version: &str,
    policy: VersionPolicy,
) -> NewUpstreamIndexEntry {
    NewUpstreamIndexEntry {
        attr_path: attr_path.to_owned(),
        pname: None,
        upstream_type: "github".to_owned(),
        upstream_key: key.to_owned(),
        instance: None,
        current_version: version.to_owned(),
        src_url: None,
        drv_path: Some(format!("/nix/store/aaaa-{attr_path}.drv")),
        policy,
    }
}

fn policy(strategy: SemverStrategy) -> VersionPolicy {
    VersionPolicy {
        strategy,
        include_prereleases: false,
        version_regex: None,
    }
}

fn releases(tags: &[&str]) -> Vec<Release> {
    tags.iter()
        .map(|t| Release {
            tag_name: (*t).to_owned(),
            is_prerelease: false,
        })
        .collect()
}

#[tokio::test]
async fn replace_upstream_index_is_per_repo_and_keeps_policy() {
    let db = setup_db().await;

    db.replace_upstream_index(
        "corepkgs",
        &[
            new_entry(
                "cmake",
                "Kitware/CMake",
                "4.2.0",
                policy(SemverStrategy::Minor),
            ),
            new_entry(
                "cmake.v3",
                "Kitware/CMake",
                "3.31.8",
                policy(SemverStrategy::Minor),
            ),
        ],
        true,
    )
    .await
    .unwrap();
    db.replace_upstream_index(
        "ekapkgs",
        &[new_entry(
            "ripgrep",
            "BurntSushi/ripgrep",
            "14.1.0",
            policy(SemverStrategy::Latest),
        )],
        true,
    )
    .await
    .unwrap();

    // Re-indexing corepkgs replaces only corepkgs' rows.
    db.replace_upstream_index(
        "corepkgs",
        &[new_entry(
            "cmake",
            "Kitware/CMake",
            "4.2.1",
            policy(SemverStrategy::Minor),
        )],
        true,
    )
    .await
    .unwrap();

    assert_eq!(db.upstream_index_count(Some("corepkgs")).await.unwrap(), 1);
    assert_eq!(db.upstream_index_count(Some("ekapkgs")).await.unwrap(), 1);

    let entries = db
        .get_attr_paths_for_source("github", "Kitware/CMake", None)
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].current_version.as_deref(), Some("4.2.1"));
    assert_eq!(
        entries[0].drv_path.as_deref(),
        Some("/nix/store/aaaa-cmake.drv")
    );
    assert_eq!(entries[0].version_policy(), policy(SemverStrategy::Minor));
}

#[tokio::test]
async fn prune_removes_unconfigured_repos() {
    let db = setup_db().await;
    for repo in ["corepkgs", "ekapkgs", "old-repo"] {
        db.replace_upstream_index(
            repo,
            &[new_entry(
                "zlib",
                "madler/zlib",
                "1.3.1",
                policy(SemverStrategy::Latest),
            )],
            true,
        )
        .await
        .unwrap();
    }

    db.retain_watched_repos(&["corepkgs", "ekapkgs"])
        .await
        .unwrap();
    assert_eq!(db.upstream_index_count(Some("old-repo")).await.unwrap(), 0);
    assert_eq!(db.upstream_index_count(Some("corepkgs")).await.unwrap(), 1);
}

#[tokio::test]
async fn dispatch_respects_version_policy() {
    let db = setup_db().await;
    db.replace_upstream_index(
        "corepkgs",
        &[
            new_entry(
                "cmake",
                "Kitware/CMake",
                "4.2.0",
                policy(SemverStrategy::Latest),
            ),
            new_entry(
                "cmake.v3",
                "Kitware/CMake",
                "3.31.8",
                policy(SemverStrategy::Minor),
            ),
            new_entry(
                "cmake.v3_30",
                "Kitware/CMake",
                "3.30.1",
                policy(SemverStrategy::Patch),
            ),
        ],
        true,
    )
    .await
    .unwrap();

    let available = releases(&["v4.3.0", "v4.2.3", "v3.31.9", "v3.30.4", "v4.4.0-rc1"]);
    let entries = db
        .get_attr_paths_for_source("github", "Kitware/CMake", None)
        .await
        .unwrap();
    let decisions: Vec<(String, Option<String>)> = entries
        .iter()
        .map(|e| (e.attr_path.clone(), update_version_for_entry(e, &available)))
        .collect();

    assert_eq!(
        decisions,
        vec![
            // Latest: newest stable release, prerelease ignored.
            ("cmake".to_owned(), Some("4.3.0".to_owned())),
            // Minor: stays on the 3.x series.
            ("cmake.v3".to_owned(), Some("3.31.9".to_owned())),
            // Patch: stays on 3.30.x.
            ("cmake.v3_30".to_owned(), Some("3.30.4".to_owned())),
        ]
    );

    // Nothing newer within the series -> no update.
    let none_newer = releases(&["v3.31.8", "v3.30.1"]);
    assert_eq!(update_version_for_entry(&entries[1], &none_newer), None);
}

#[tokio::test]
async fn dispatch_uses_version_regex_and_prerelease_policy() {
    let db = setup_db().await;
    db.replace_upstream_index(
        "",
        &[new_entry(
            "jq",
            "jqlang/jq",
            "1.7.1",
            VersionPolicy {
                strategy: SemverStrategy::Latest,
                include_prereleases: true,
                version_regex: Some("jq-(.*)".to_owned()),
            },
        )],
        true,
    )
    .await
    .unwrap();
    let entries = db
        .get_attr_paths_for_source("github", "jqlang/jq", None)
        .await
        .unwrap();

    let available = releases(&["jq-1.8.0rc1", "jq-1.7.1"]);
    assert_eq!(
        update_version_for_entry(&entries[0], &available).as_deref(),
        Some("1.8.0rc1")
    );
}

#[tokio::test]
async fn incomplete_index_keeps_stale_entries() {
    let db = setup_db().await;
    db.replace_upstream_index(
        "corepkgs",
        &[
            new_entry(
                "zlib",
                "madler/zlib",
                "1.3.1",
                policy(SemverStrategy::Latest),
            ),
            new_entry("jq", "jqlang/jq", "1.7.1", policy(SemverStrategy::Latest)),
        ],
        true,
    )
    .await
    .unwrap();

    // Without pruning (evaluation looked incomplete), entries missing from
    // the new run are kept and the others refreshed.
    let pruned = db
        .replace_upstream_index(
            "corepkgs",
            &[new_entry(
                "zlib",
                "madler/zlib",
                "1.3.2",
                policy(SemverStrategy::Latest),
            )],
            false,
        )
        .await
        .unwrap();
    assert_eq!(pruned, 0);
    assert_eq!(db.upstream_index_count(Some("corepkgs")).await.unwrap(), 2);
    let zlib = db
        .get_attr_paths_for_source("github", "madler/zlib", None)
        .await
        .unwrap();
    assert_eq!(zlib[0].current_version.as_deref(), Some("1.3.2"));

    // With pruning, they are removed.
    let pruned = db
        .replace_upstream_index(
            "corepkgs",
            &[new_entry(
                "zlib",
                "madler/zlib",
                "1.3.2",
                policy(SemverStrategy::Latest),
            )],
            true,
        )
        .await
        .unwrap();
    assert_eq!(pruned, 1);
    assert_eq!(db.upstream_index_count(Some("corepkgs")).await.unwrap(), 1);
}
