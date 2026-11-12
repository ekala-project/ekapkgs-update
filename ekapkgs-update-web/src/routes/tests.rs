//! Router tests against an in-memory SQLite database with the real
//! migrations applied. Every page, fragment and API endpoint is requested
//! with `tower::ServiceExt::oneshot`, both against a seeded database and an
//! empty one.

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use hyper::Request;
use serde_json::Value;
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use crate::routes;
use crate::state::AppState;

const RUN_DONE: &str = "a1b2c3d4-0000-4000-8000-000000000001";
const RUN_LIVE: &str = "b2c3d4e5-0000-4000-8000-000000000002";
const WATCH_EMPTY: &str = "c3d4e5f6-0000-4000-8000-000000000003";
const WATCH_DONE: &str = "d4e5f6a7-0000-4000-8000-000000000004";

/// Fixture covering every table the dashboard reads. `AGO(n)` is replaced
/// by an RFC 3339 timestamp `n` minutes in the past (negative = future).
const SEED_SQL: &str = r#"
INSERT INTO update_sessions (id, mode, status, started_at, completed_at, updated_at,
    packages_attempted, packages_succeeded, packages_failed, packages_skipped,
    packages_evaluated, packages_backoff, config_json) VALUES
  ('a1b2c3d4-0000-4000-8000-000000000001', 'run', 'completed', AGO(120), AGO(60), AGO(60),
    3, 1, 1, 1, 100, 5, '{"concurrency":4,"commit_strategy":"worktrees"}'),
  ('b2c3d4e5-0000-4000-8000-000000000002', 'run', 'running', AGO(10), NULL, AGO(1),
    2, 0, 0, 0, 50, 0, NULL),
  ('c3d4e5f6-0000-4000-8000-000000000003', 'watch', 'completed', AGO(300), AGO(299), AGO(299),
    0, 0, 0, 0, 0, 0, NULL),
  ('d4e5f6a7-0000-4000-8000-000000000004', 'watch', 'completed', AGO(200), AGO(190), AGO(190),
    1, 1, 0, 0, 5, 0, '{"mode":"watch"}');

INSERT INTO updates (repo_name, attr_path, status, current_version, latest_upstream_version,
    proposed_version, baseline_version, failed_version, failure_count, last_error,
    last_session_id, last_checked, last_attempted, next_attempt, pr_url, pr_number,
    rebuild_count, pname, description, homepage, upstream_type, upstream_key,
    upstream_instance, semver_strategy) VALUES
  ('', 'hello', 'up_to_date', '2.12', '2.12', NULL, NULL, NULL, 0, NULL,
    'a1b2c3d4-0000-4000-8000-000000000001', AGO(60), AGO(60), NULL, NULL, NULL,
    3, 'hello', 'A program that produces a familiar, friendly greeting',
    'https://www.gnu.org/software/hello/', 'directory', 'hello', NULL, NULL),
  ('', 'ripgrep', 'proposed', '14.0.0', '14.1.0', '14.1.0', NULL, NULL, 0, NULL,
    'a1b2c3d4-0000-4000-8000-000000000001', AGO(70), AGO(70), NULL,
    'https://github.com/example/pkgs/pull/42', 42, 12, 'ripgrep', 'Fast line-oriented search',
    'https://github.com/BurntSushi/ripgrep', 'github', 'BurntSushi/ripgrep', NULL, 'latest'),
  ('', 'python3Packages.requests', 'failed', '2.31.0', '2.32.0', NULL, NULL, '2.32.0', 2,
    'BuildError: hash mismatch', 'a1b2c3d4-0000-4000-8000-000000000001', AGO(70), AGO(70),
    AGO(-2880), NULL, NULL, 250, 'requests', 'HTTP for humans', NULL, 'pypi', 'requests',
    NULL, NULL),
  ('', 'fd', 'outdated', '9.0.0', '10.0.0', NULL, '10.0.0', NULL, 0, NULL, NULL,
    AGO(400), NULL, NULL, NULL, NULL, 1, 'fd', NULL, NULL, 'github', 'sharkdp/fd', NULL, NULL),
  ('', 'jq', 'upstream_error', '1.7', NULL, NULL, NULL, NULL, 0, 'HTTP 404 Not Found', NULL,
    AGO(30), NULL, NULL, NULL, NULL, NULL, 'jq', 'Lightweight <b>JSON</b> processor', NULL,
    'github', 'jqlang/jq', NULL, NULL),
  ('', 'newpkg', 'pending', NULL, NULL, NULL, NULL, NULL, 0, NULL, NULL,
    NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('extra', 'hello', 'outdated', '2.10', '2.12', NULL, NULL, NULL, 0, NULL, NULL,
    AGO(90), NULL, NULL, NULL, NULL, 2, 'hello', NULL, NULL, 'directory', 'hello', NULL, NULL),
  ('extra', 'curl', 'up_to_date', '8.10.0', '8.10.0', NULL, NULL, NULL, 0, NULL,
    'd4e5f6a7-0000-4000-8000-000000000004', AGO(190), AGO(190), NULL,
    'https://github.com/example/extra/pull/7', 7, 900, 'curl', 'Command line URL tool',
    'https://curl.se', 'github', 'curl/curl', NULL, NULL);

INSERT INTO update_phases (session_id, repo_name, attr_path, phase, started_at, completed_at,
    duration_ms, status, error_type, error_details, artifacts_path) VALUES
  ('a1b2c3d4-0000-4000-8000-000000000001', '', 'ripgrep', 'metadata_extraction',
    AGO(118), AGO(118), 900, 'success', NULL, NULL, NULL),
  ('a1b2c3d4-0000-4000-8000-000000000001', '', 'ripgrep', 'version_selection',
    AGO(117), AGO(117), 1200, 'success', NULL, NULL, NULL),
  ('a1b2c3d4-0000-4000-8000-000000000001', '', 'ripgrep', 'build',
    AGO(116), AGO(100), 960000, 'success', NULL, NULL, NULL),
  ('a1b2c3d4-0000-4000-8000-000000000001', '', 'ripgrep', 'pr_creation',
    AGO(100), AGO(99), 4000, 'success', NULL, NULL, NULL),
  ('a1b2c3d4-0000-4000-8000-000000000001', '', 'python3Packages.requests', 'metadata_extraction',
    AGO(110), AGO(110), 800, 'success', NULL, NULL, NULL),
  ('a1b2c3d4-0000-4000-8000-000000000001', '', 'python3Packages.requests', 'build',
    AGO(109), AGO(70), 2340000, 'failed', 'BuildError',
    '{"BuildError":{"message":"hash mismatch in fixed-output derivation"}}',
    '/var/lib/ekapkgs-update/preserved/requests'),
  ('a1b2c3d4-0000-4000-8000-000000000001', '', 'hello', 'version_selection',
    AGO(105), AGO(105), 300, 'skipped', NULL, NULL, NULL),
  ('b2c3d4e5-0000-4000-8000-000000000002', '', 'jq', 'metadata_extraction',
    AGO(9), AGO(9), 700, 'success', NULL, NULL, NULL),
  ('b2c3d4e5-0000-4000-8000-000000000002', '', 'fd', 'build',
    AGO(2), NULL, NULL, 'running', NULL, NULL, NULL),
  ('d4e5f6a7-0000-4000-8000-000000000004', 'extra', 'curl', 'build',
    AGO(199), AGO(195), 240000, 'success', NULL, NULL, NULL),
  ('d4e5f6a7-0000-4000-8000-000000000004', 'extra', 'curl', 'pr_creation',
    AGO(195), AGO(194), 3000, 'success', NULL, NULL, NULL);

INSERT INTO update_logs (drv_path, repo_name, attr_path, timestamp, status, error_log,
    old_version, new_version, session_id, phase, error_type) VALUES
  ('/nix/store/abc-python3.12-requests-2.32.0.drv', '', 'python3Packages.requests', AGO(70),
    'failed', 'error: builder failed
hash mismatch in fixed-output derivation', '2.31.0', '2.32.0',
    'a1b2c3d4-0000-4000-8000-000000000001', 'build', 'BuildError'),
  ('extra:hello@2.12', 'extra', 'hello', AGO(90), 'failed',
    'START-OF-LOG ' || replace(hex(zeroblob(4000)), '00', 'x ') || ' final line: HashError',
    '2.10', '2.12', NULL, 'source_hash_update', 'HashError');

INSERT INTO watched_repos (name, upstream, path, entrypoint, base_branch, fork_remote,
    head_commit, head_subject, sync_status, sync_error, last_synced_at, index_status,
    index_error, last_indexed_at, index_duration_ms, evaluated_count, indexed_count,
    index_skipped, index_errors, bootstrapped_at, created_at, updated_at) VALUES
  ('', NULL, '/srv/pkgs', '/srv/pkgs/default.nix', NULL, NULL, NULL, NULL, 'unmanaged', NULL,
    AGO(500), 'ok', NULL, AGO(500), 65000, 1200, 900, 300, 2, AGO(400), AGO(10000), AGO(400)),
  ('extra', 'https://github.com/example/extra', '/srv/extra', '/srv/extra/pkgs/top-level.nix',
    'main', 'fork', 'deadbeefcafebabe1234567890abcdef12345678', 'curl: 8.9.0 -> 8.10.0', 'ok',
    NULL, AGO(200), 'error', 'nix-eval-jobs exited with status 1', AGO(210), 3000, 10, 4, 6, 0,
    NULL, AGO(10000), AGO(200));

INSERT INTO watch_cycles (id, kind, repo_name, started_at, completed_at, duration_ms, status,
    bootstrap, sources_polled, sources_failed, packages_observed, new_releases, dispatched,
    updated, failed, session_id, error) VALUES
  (1, 'index', '', AGO(500), AGO(499), 65000, 'completed', 0, 0, 0, 900, 0, 0, 0, 0, NULL, NULL),
  (2, 'poll', NULL, AGO(400), AGO(399), 30000, 'completed', 1, 5, 0, 12, 0, 0, 0, 0, NULL, NULL),
  (3, 'poll', NULL, AGO(200), AGO(190), 600000, 'completed', 0, 5, 1, 12, 2, 1, 1, 0,
    'd4e5f6a7-0000-4000-8000-000000000004', NULL),
  (4, 'poll', NULL, AGO(30), AGO(29), 1000, 'failed', 0, 0, 0, 0, 0, 0, 0, 0, NULL,
    'network unreachable');

INSERT INTO upstream_sources (upstream_type, upstream_key, instance, latest_version,
    latest_tag, release_count, latest_changed_at, first_seen_at, last_polled_at,
    last_success_at, last_error, consecutive_failures) VALUES
  ('github', 'BurntSushi/ripgrep', '', '14.1.0', '14.1.0', 40, AGO(250), AGO(10000), AGO(30),
    AGO(30), NULL, 0),
  ('github', 'jqlang/jq', '', NULL, NULL, NULL, NULL, AGO(10000), AGO(30), NULL,
    'HTTP 404 Not Found', 3),
  ('pypi', 'requests', '', '2.32.0', NULL, 150, AGO(5000), AGO(10000), AGO(30), AGO(30), NULL, 0),
  ('gitlab', 'GNOME/glib', 'gitlab.gnome.org', NULL, NULL, NULL, NULL, AGO(100), NULL, NULL,
    NULL, 0);

INSERT INTO upstream_index (repo_name, attr_path, upstream_type, upstream_key, instance,
    current_version, src_url, indexed_at, pname, semver_strategy, version_regex,
    include_prereleases) VALUES
  ('', 'ripgrep', 'github', 'BurntSushi/ripgrep', NULL, '14.0.0',
    'https://github.com/BurntSushi/ripgrep/archive/14.0.0.tar.gz', AGO(500), 'ripgrep',
    'latest', NULL, 0),
  ('', 'jq', 'github', 'jqlang/jq', NULL, '1.7', NULL, AGO(500), 'jq', NULL, '^jq-(.*)$', 1),
  ('', 'python3Packages.requests', 'pypi', 'requests', NULL, '2.31.0', NULL, AGO(500),
    'requests', 'minor', NULL, 0),
  ('extra', 'curl', 'github', 'curl/curl', NULL, '8.10.0', NULL, AGO(210), 'curl', NULL,
    NULL, 0);

INSERT INTO autofix_queue (id, repo_name, attr_path, session_id, error_type, failed_phase,
    status, priority, attempts, max_attempts, created_at, updated_at, fixed_at,
    artifacts_path) VALUES
  (1, '', 'python3Packages.requests', 'a1b2c3d4-0000-4000-8000-000000000001', 'BuildError',
    'build', 'escalated', 0, 2, 3, AGO(65), AGO(50), NULL,
    '/var/lib/ekapkgs-update/preserved/requests'),
  (2, 'extra', 'hello', 'a1b2c3d4-0000-4000-8000-000000000001',
    'BuildError', 'build', 'queued', 0, 0, 3, AGO(65), AGO(65), NULL, NULL);

INSERT INTO autofix_attempts (queue_id, repo_name, attempt_number, started_at, completed_at,
    prompt_tokens, completion_tokens, changes_applied, build_success, status,
    error_message) VALUES
  (1, '', 1, AGO(64), AGO(63), 1200, 300, 1, 0, 'build_failed', 'still fails after patch'),
  (1, '', 2, AGO(55), AGO(54), 1300, 0, 0, NULL, 'llm_error', 'timeout'),
  (2, 'extra', 1, AGO(60), AGO(59), 900, 100, 0, NULL, 'parse_error', 'extra repo attempt');
"#;

/// Replace every `AGO(n)` with a quoted timestamp `n` minutes before now.
fn expand_timestamps(sql: &str) -> String {
    let now = Utc::now();
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(start) = rest.find("AGO(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        let end = after.find(')').expect("unterminated AGO(");
        let minutes: i64 = after[..end]
            .trim()
            .parse()
            .expect("AGO(n) takes an integer");
        out.push('\'');
        out.push_str(&(now - Duration::minutes(minutes)).to_rfc3339());
        out.push('\'');
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

async fn pool() -> SqlitePool {
    // One connection: the in-memory database lives as long as it does, and
    // there is no shared-cache locking between connections.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("../ekapkgs-update/migrations")
        .run(&pool)
        .await
        .unwrap();
    pool
}

async fn seeded_pool() -> SqlitePool {
    let pool = pool().await;
    sqlx::Executor::execute(&pool, expand_timestamps(SEED_SQL).as_str())
        .await
        .unwrap();
    pool
}

fn app(pool: SqlitePool) -> Router {
    routes::router(AppState::new(ekapkgs_update::database::Database::from(
        pool,
    )))
}

async fn seeded_app() -> Router {
    app(seeded_pool().await)
}

async fn empty_app() -> Router {
    app(pool().await)
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("invalid JSON ({e}): {}", self.body))
    }

    fn content_type(&self) -> &str {
        self.headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    }
}

async fn send(app: &Router, request: Request<Body>) -> Reply {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        headers,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

async fn get(app: &Router, uri: &str) -> Reply {
    send(
        app,
        Request::builder().uri(uri).body(Body::empty()).unwrap(),
    )
    .await
}

/// GET `uri`, assert 200 and that every needle occurs in the body.
async fn expect_ok(app: &Router, uri: &str, needles: &[&str]) -> String {
    let reply = get(app, uri).await;
    assert_eq!(reply.status, StatusCode::OK, "{uri}: {}", reply.body);
    let missing: Vec<&str> = needles
        .iter()
        .copied()
        .filter(|needle| !reply.body.contains(needle))
        .collect();
    assert!(
        missing.is_empty(),
        "{uri}: missing {missing:?} in body:\n{}",
        reply.body
    );
    reply.body
}

// ── Pages ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn pages_render_seeded_content() {
    let app = seeded_app().await;
    let run_done = format!("/sessions/{RUN_DONE}");
    let watch_done = format!("/sessions/{WATCH_DONE}");
    let cases: Vec<(&str, Vec<&str>)> = vec![
        (
            "/",
            vec![
                "Running sessions",
                "Recent activity",
                "b2c3d4e5",
                "href=\"/packages/ripgrep?repo=default\"",
                "Success rate · 30 days",
                "BuildError",
                "Latest PRs",
                "#42",
                "review the backlog",
            ],
        ),
        (
            "/packages",
            vec![
                "python3Packages.requests",
                "href=\"/packages/ripgrep?repo=default\"",
                "outdated · backlog",
                "upstream error",
                "#42",
                "8 results",
            ],
        ),
        (
            "/packages/ripgrep",
            vec![
                "14.1.0",
                "#42",
                "BurntSushi/ripgrep",
                "Fast line-oriented search",
                "Update history",
                "a1b2c3d4",
                "archive/14.0.0.tar.gz",
            ],
        ),
        (
            "/packages/python3Packages.requests",
            vec![
                "BuildError",
                "hash mismatch in fixed-output derivation",
                ">escalated<",
                "still fails after patch",
                "/var/lib/ekapkgs-update/preserved/requests",
                "backoff",
            ],
        ),
        (
            "/packages/hello",
            vec![
                "This attribute exists in 2 repositories",
                "?repo=extra",
                "2.12",
            ],
        ),
        (
            "/packages/hello?repo=extra",
            vec!["2.10", "HashError", "showing the last"],
        ),
        (
            "/sessions",
            vec![
                "a1b2c3d4",
                "b2c3d4e5",
                "1 finished watch session that attempted nothing is hidden",
                "Show it",
            ],
        ),
        (
            run_done.as_str(),
            vec![
                "BuildError",
                "python3Packages.requests",
                "&quot;concurrency&quot;",
                "Failure logs",
                "Raw phases",
            ],
        ),
        (
            watch_done.as_str(),
            vec!["Watch poll cycle #3", "curl", "extra"],
        ),
        (
            "/failures",
            vec![
                "python3Packages.requests",
                "HashError",
                "hash mismatch",
                // Long logs keep their tail.
                "final line: HashError",
                "showing the last 6,000 characters",
            ],
        ),
        (
            "/watch",
            vec![
                "extra",
                "jqlang/jq",
                "HTTP 404 Not Found",
                "nix-eval-jobs exited with status 1",
                "network unreachable",
                "deadbeef",
                "bootstrap",
                "BurntSushi/ripgrep",
            ],
        ),
        (
            "/analytics",
            vec![
                "Daily outcomes",
                "BuildError",
                "Build",
                "Status by repository",
                "github",
                "Largest rebuilds",
                ">build failed<",
                "Table view",
            ],
        ),
        ("/analytics?range=7d", vec!["7 days"]),
        ("/analytics?range=all", vec!["All time"]),
    ];
    for (uri, needles) in &cases {
        let body = expect_ok(&app, uri, needles).await;
        assert!(
            !body.contains("START-OF-LOG"),
            "{uri}: log head was not truncated"
        );
        assert!(
            body.starts_with("<!DOCTYPE html>"),
            "{uri}: not a full page"
        );
        assert!(
            body.contains("aria-current=\"page\""),
            "{uri}: no active nav item"
        );
    }
}

#[tokio::test]
async fn pages_escape_database_content() {
    let app = seeded_app().await;
    let body = expect_ok(
        &app,
        "/packages/jq",
        &["Lightweight &lt;b&gt;JSON&lt;/b&gt; processor"],
    )
    .await;
    assert!(!body.contains("<b>JSON</b>"));
}

#[tokio::test]
async fn package_filters_and_status_chips() {
    let app = seeded_app().await;

    let backlog = expect_ok(
        &app,
        "/packages?status=backlog",
        &["/packages/fd?repo=default"],
    )
    .await;
    assert!(!backlog.contains("/packages/ripgrep?repo=default"));

    let extra = expect_ok(&app, "/packages?repo=extra", &["/packages/curl?repo=extra"]).await;
    assert!(!extra.contains("/packages/ripgrep?repo=default"));

    let search = expect_ok(&app, "/packages?q=requ", &["python3Packages.requests"]).await;
    assert!(!search.contains("/packages/fd?repo=default"));

    let upstream = expect_ok(
        &app,
        "/packages?upstream=pypi",
        &["python3Packages.requests"],
    )
    .await;
    assert!(!upstream.contains("/packages/ripgrep?repo=default"));

    expect_ok(
        &app,
        "/packages?q=nothing-matches",
        &["No matching packages"],
    )
    .await;

    // Unknown values and every sort fall back gracefully.
    for uri in [
        "/packages?status=bogus",
        "/packages?sort=bogus&page=abc",
        "/packages?sort=status",
        "/packages?sort=checked",
        "/packages?sort=rebuilds",
        "/packages?sort=failures",
        "/packages?sort=attr",
    ] {
        expect_ok(&app, uri, &["ripgrep"]).await;
    }
}

#[tokio::test]
async fn packages_paginate() {
    let pool = seeded_pool().await;
    sqlx::Executor::execute(
        &pool,
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 60) \
         INSERT INTO updates (repo_name, attr_path, status) \
         SELECT '', printf('bulk%02d', i), 'up_to_date' FROM n",
    )
    .await
    .unwrap();
    let app = app(pool);

    let first = expect_ok(
        &app,
        "/packages?sort=attr",
        &["Page 1 of 2", "rel=\"next\""],
    )
    .await;
    assert!(first.contains("bulk01"));
    assert!(!first.contains("/packages/ripgrep?repo=default"));

    let second = expect_ok(
        &app,
        "/packages?sort=attr&page=2",
        &["Page 2 of 2", "rel=\"prev\""],
    )
    .await;
    assert!(second.contains("/packages/ripgrep?repo=default"));
    assert!(!second.contains("bulk01"));

    // Out-of-range pages clamp to the last page.
    expect_ok(&app, "/packages?sort=attr&page=99", &["Page 2 of 2"]).await;

    let json = get(&app, "/api/packages?limit=10&page=2").await.json();
    assert_eq!(json["packages"].as_array().unwrap().len(), 10);
    assert_eq!(json["total"], 68);
    assert_eq!(json["page"], 2);
}

#[tokio::test]
async fn session_filters() {
    let app = seeded_app().await;

    let running = expect_ok(&app, "/sessions?status=running", &["b2c3d4e5"]).await;
    assert!(!running.contains(&format!("/sessions/{RUN_DONE}")));

    let watch = expect_ok(&app, "/sessions?mode=watch", &["d4e5f6a7", "Show it"]).await;
    assert!(!watch.contains(&format!("/sessions/{RUN_DONE}")));
    assert!(!watch.contains(&format!("/sessions/{WATCH_EMPTY}")));

    expect_ok(
        &app,
        "/sessions?mode=watch&show_empty=1",
        &["c3d4e5f6", "Hide them"],
    )
    .await;

    expect_ok(
        &app,
        "/sessions?status=cancelled",
        &["No matching sessions"],
    )
    .await;

    // The rollup can be filtered by outcome.
    let failed = expect_ok(
        &app,
        &format!("/sessions/{RUN_DONE}?outcome=failed"),
        &["python3Packages.requests"],
    )
    .await;
    assert!(!failed.contains("/packages/ripgrep?repo=default"));
}

#[tokio::test]
async fn failure_filters() {
    let app = seeded_app().await;
    let hash = expect_ok(
        &app,
        "/failures?error_type=HashError",
        &["extra:hello@2.12"],
    )
    .await;
    assert!(!hash.contains("/nix/store/abc-python3.12-requests-2.32.0.drv"));

    let repo = expect_ok(
        &app,
        "/failures?repo=default",
        &["python3Packages.requests"],
    )
    .await;
    assert!(!repo.contains("extra:hello@2.12"));

    let phase = expect_ok(
        &app,
        "/failures?phase=build&q=requests",
        &["requests-2.32.0.drv"],
    )
    .await;
    assert!(!phase.contains("extra:hello@2.12"));

    expect_ok(&app, "/failures?q=zzz", &["No matching failures"]).await;
}

#[tokio::test]
async fn watch_cycle_filter() {
    let app = seeded_app().await;
    let index = expect_ok(&app, "/watch?kind=index", &["#1"]).await;
    assert!(!index.contains("network unreachable"));
    expect_ok(&app, "/watch?kind=poll", &["network unreachable"]).await;
}

// ── Fragments ───────────────────────────────────────────────────────────

#[tokio::test]
async fn fragments_render_without_layout() {
    let app = seeded_app().await;
    let live = format!("/fragments/sessions/{RUN_LIVE}");
    let done = format!("/fragments/sessions/{RUN_DONE}");
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("/fragments/stats", vec!["Success rate", "tile"]),
        (
            "/fragments/active",
            vec!["b2c3d4e5", "/packages/fd?repo=default"],
        ),
        ("/fragments/watch", vec!["Last poll", "extra"]),
        ("/fragments/activity", vec!["ripgrep", "Latest PRs"]),
        ("/fragments/sessions", vec!["a1b2c3d4", "b2c3d4e5"]),
        ("/fragments/sessions?status=running", vec!["b2c3d4e5"]),
        (
            live.as_str(),
            vec!["id=\"session-live\"", "hx-get=", "In flight"],
        ),
        (done.as_str(), vec!["id=\"session-live\"", "BuildError"]),
    ];
    for (uri, needles) in &cases {
        let body = expect_ok(&app, uri, needles).await;
        assert!(
            !body.contains("<html"),
            "{uri}: fragment rendered the full layout"
        );
    }

    // A finished session's live fragment stops polling.
    let done_body = get(&app, &done).await.body;
    assert!(!done_body.contains("hx-get="));
}

// ── JSON API ────────────────────────────────────────────────────────────

#[tokio::test]
async fn api_endpoints() {
    let app = seeded_app().await;

    let stats = get(&app, "/api/stats").await;
    assert_eq!(stats.status, StatusCode::OK);
    assert!(stats.content_type().starts_with("application/json"));
    let stats = stats.json();
    assert_eq!(stats["total_packages"], 8);
    // Watch sessions that attempted nothing are not counted.
    assert_eq!(stats["total_sessions"], 3);
    assert_eq!(stats["running_sessions"], 1);
    assert!(stats["packages"].is_object());
    assert!(stats["outcomes_30d"].is_object());

    let sessions = get(&app, "/api/sessions?status=completed").await.json();
    assert_eq!(
        sessions["total"], 2,
        "empty watch sessions are hidden: {sessions}"
    );
    let sessions = get(&app, "/api/sessions?show_empty=1").await.json();
    assert_eq!(sessions["total"], 4);
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 4);

    let phases = get(&app, &format!("/api/sessions/{RUN_DONE}/phases"))
        .await
        .json();
    let phases = phases.as_array().unwrap();
    assert_eq!(phases.len(), 7);
    assert_eq!(phases[0]["phase"], "metadata_extraction");

    let packages = get(&app, "/api/packages?status=failed").await.json();
    assert_eq!(packages["total"], 1);
    assert_eq!(
        packages["packages"][0]["attr_path"],
        "python3Packages.requests"
    );
    assert!(packages["status_counts"].is_object());

    let package = get(&app, "/api/packages/python3Packages.requests")
        .await
        .json();
    assert_eq!(package["attr_path"], "python3Packages.requests");
    assert_eq!(package["update"]["status"], "failed");
    assert_eq!(package["phases"].as_array().unwrap().len(), 2);
    assert_eq!(package["logs"].as_array().unwrap().len(), 1);
    assert_eq!(package["autofix"]["queue"].as_array().unwrap().len(), 1);
    assert_eq!(package["autofix"]["attempts"].as_array().unwrap().len(), 2);

    let multi = get(&app, "/api/packages/hello?repo=extra").await.json();
    assert_eq!(multi["repo_name"], "extra");
    // Autofix rows are scoped by repository, like everything else.
    let queue = multi["autofix"]["queue"].as_array().unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["repo_name"], "extra");
    let attempts = multi["autofix"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["error_message"], "extra repo attempt");
    let unscoped = get(&app, "/api/packages/hello?repo=default").await.json();
    assert!(unscoped["autofix"]["queue"].as_array().unwrap().is_empty());
    assert_eq!(multi["repos"].as_array().unwrap().len(), 2);

    let watch = get(&app, "/api/watch").await.json();
    assert_eq!(watch["repos"].as_array().unwrap().len(), 2);
    assert_eq!(watch["cycle_total"], 4);
    assert!(!watch["failing_sources"].as_array().unwrap().is_empty());
}

// ── Errors ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_things_are_404() {
    let app = seeded_app().await;
    for uri in [
        "/sessions/does-not-exist",
        "/fragments/sessions/does-not-exist",
        "/packages/does-not-exist",
        "/packages/ripgrep?repo=extra",
        "/no/such/page",
        "/static/css/nope.css",
    ] {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{uri}");
        assert!(reply.content_type().starts_with("text/html"), "{uri}");
        assert!(reply.body.contains("404"), "{uri}: {}", reply.body);
    }
    for uri in [
        "/api/sessions/does-not-exist/phases",
        "/api/packages/does-not-exist",
    ] {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(reply.json()["status"], 404, "{uri}");
    }
}

// ── Empty database ──────────────────────────────────────────────────────

#[tokio::test]
async fn empty_database_renders_every_page() {
    let app = empty_app().await;
    let cases: &[(&str, &[&str])] = &[
        (
            "/",
            &[
                "No packages tracked yet",
                "No session is running",
                "Watch mode has not run",
            ],
        ),
        ("/packages", &["No packages tracked yet"]),
        ("/sessions", &["No sessions yet"]),
        ("/failures", &["No failures recorded"]),
        ("/watch", &["Watch mode has not run yet"]),
        (
            "/analytics",
            &["No finished sessions", "No packages tracked yet"],
        ),
        ("/analytics?range=all", &["No phases recorded"]),
        ("/fragments/stats", &["No packages tracked yet"]),
        ("/fragments/active", &["No session is running"]),
        ("/fragments/watch", &["Watch mode has not run"]),
        ("/fragments/activity", &["No update activity"]),
        ("/fragments/sessions", &["No sessions yet"]),
    ];
    for (uri, needles) in cases {
        expect_ok(&app, uri, needles).await;
    }
    for uri in ["/api/stats", "/api/sessions", "/api/packages", "/api/watch"] {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::OK, "{uri}: {}", reply.body);
        reply.json();
    }
}

// ── Static assets ───────────────────────────────────────────────────────

#[tokio::test]
async fn static_assets_are_embedded_and_cacheable() {
    let app = empty_app().await;
    let cases = [
        ("/static/css/styles.css", "text/css"),
        ("/static/js/htmx.min.js", "text/javascript"),
        ("/static/js/live.js", "text/javascript"),
    ];
    for (uri, content_type) in cases {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::OK, "{uri}");
        assert!(reply.content_type().starts_with(content_type), "{uri}");
        assert!(!reply.body.is_empty(), "{uri}");
        assert!(
            reply.headers[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .contains("max-age"),
            "{uri}"
        );
        let etag = reply.headers[header::ETAG].to_str().unwrap().to_owned();

        let revalidated = send(
            &app,
            Request::builder()
                .uri(uri)
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(revalidated.status, StatusCode::NOT_MODIFIED, "{uri}");
        assert!(revalidated.body.is_empty(), "{uri}");

        let stale = send(
            &app,
            Request::builder()
                .uri(uri)
                .header(header::IF_NONE_MATCH, "\"0000\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(stale.status, StatusCode::OK, "{uri}");
    }

    // Pages reference assets with the cache-busting version.
    let page = get(&app, "/").await.body;
    assert!(page.contains(&format!(
        "/static/css/styles.css?v={}",
        crate::assets::version()
    )));
}

// ── Live channel ────────────────────────────────────────────────────────

#[tokio::test]
async fn live_fingerprint_changes_with_new_phases() {
    let pool = seeded_pool().await;
    let before = crate::queries::live::fingerprint(&pool).await.unwrap();
    assert_eq!(
        before,
        crate::queries::live::fingerprint(&pool).await.unwrap()
    );
    sqlx::Executor::execute(
        &pool,
        expand_timestamps(
            "INSERT INTO update_phases (session_id, repo_name, attr_path, phase, started_at, status) \
             VALUES ('b2c3d4e5-0000-4000-8000-000000000002', '', 'jq', 'version_selection', AGO(0), 'running')",
        )
        .as_str(),
    )
    .await
    .unwrap();
    let after = crate::queries::live::fingerprint(&pool).await.unwrap();
    assert_ne!(before, after);
}

#[tokio::test]
async fn websocket_route_requires_upgrade() {
    let app = empty_app().await;
    let reply = get(&app, "/ws/live").await;
    assert!(reply.status.is_client_error(), "{}", reply.status);
}
