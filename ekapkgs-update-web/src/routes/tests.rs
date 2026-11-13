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

/// GET a JSON endpoint, assert 200 and return the parsed body.
async fn api(app: &Router, uri: &str) -> Value {
    let reply = get(app, uri).await;
    assert_eq!(reply.status, StatusCode::OK, "{uri}: {}", reply.body);
    assert!(
        reply.content_type().starts_with("application/json"),
        "{uri}: {}",
        reply.content_type()
    );
    reply.json()
}

/// The `attr_path` of every row in `value[key]`.
fn attrs(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} is not an array: {value}"))
        .iter()
        .map(|row| row["attr_path"].as_str().unwrap().to_owned())
        .collect()
}

fn len(value: &Value) -> usize {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {value}"))
        .len()
}

// ── App shell ───────────────────────────────────────────────────────────

#[tokio::test]
async fn page_routes_serve_the_app_shell() {
    let app = seeded_app().await;
    let run_done = format!("/sessions/{RUN_DONE}");
    for uri in [
        "/",
        "/packages",
        "/packages?status=failed&page=2",
        "/packages/python3Packages.requests",
        "/sessions",
        run_done.as_str(),
        "/failures",
        "/watch",
        "/analytics?range=7d",
    ] {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::OK, "{uri}");
        assert!(reply.content_type().starts_with("text/html"), "{uri}");
        assert_eq!(reply.headers[header::CACHE_CONTROL], "no-cache", "{uri}");
        assert!(reply.body.contains("/static/js/elm.js?v="), "{uri}");
        assert!(reply.body.contains("/static/js/app.js?v="), "{uri}");
        assert!(reply.body.contains("/static/css/styles.css?v="), "{uri}");
    }

    // Unknown paths get the shell too (the frontend renders its 404 page),
    // but with a 404 status. The shell does not check that a package or
    // session exists; the API does.
    for uri in ["/no/such/page", "/fragments/stats", "/packages/a/b"] {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{uri}");
        assert!(reply.content_type().starts_with("text/html"), "{uri}");
        assert!(reply.body.contains("/static/js/elm.js?v="), "{uri}");
    }
}

// ── JSON API ────────────────────────────────────────────────────────────

#[tokio::test]
async fn api_stats_and_dashboard() {
    let app = seeded_app().await;

    let stats = api(&app, "/api/stats").await;
    assert_eq!(stats["total_packages"], 8);
    // Watch sessions that attempted nothing are not counted.
    assert_eq!(stats["total_sessions"], 3);
    assert_eq!(stats["running_sessions"], 1);
    assert!(stats["packages"].is_object());
    assert!(stats["outcomes_30d"].is_object());

    let dash = api(&app, "/api/dashboard").await;
    assert_eq!(dash["window_days"], 30);
    assert_eq!(dash["counts"]["total"], 8);
    assert!(dash["current"].is_object());
    assert!(dash["previous"].is_object());
    let running = dash["running"].as_array().unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0]["id"], RUN_LIVE);
    assert!(
        attrs(&dash, "running_phases").contains(&"fd".to_owned()),
        "{dash}"
    );
    assert_eq!(len(&dash["watch"]["repos"]), 2);
    assert!(dash["watch"]["last_poll"].is_object());
    assert!(!dash["activity"].as_array().unwrap().is_empty());
    let prs = attrs(&dash, "recent_prs");
    assert!(prs.contains(&"ripgrep".to_owned()), "{prs:?}");
}

#[tokio::test]
async fn api_packages_filters_and_sorts() {
    let app = seeded_app().await;

    let all = api(&app, "/api/packages").await;
    assert_eq!(all["total"], 8);
    assert_eq!(all["all_total"], 8);
    assert_eq!(all["page"], 1);
    assert_eq!(all["per_page"], 50);
    assert!(all["status_counts"].is_object());
    assert!(!all["sorts"].as_array().unwrap().is_empty());
    assert_eq!(len(&all["repos"]), 2);
    assert!(!all["upstream_types"].as_array().unwrap().is_empty());

    let failed = api(&app, "/api/packages?status=failed").await;
    assert_eq!(failed["total"], 1);
    assert_eq!(attrs(&failed, "packages"), ["python3Packages.requests"]);
    // Facet counts follow the filter; the overall total does not.
    assert_eq!(failed["all_total"], 8);

    let backlog = attrs(&api(&app, "/api/packages?status=backlog").await, "packages");
    assert!(backlog.contains(&"fd".to_owned()), "{backlog:?}");
    assert!(!backlog.contains(&"ripgrep".to_owned()), "{backlog:?}");

    let extra = api(&app, "/api/packages?repo=extra").await;
    let extra_attrs = attrs(&extra, "packages");
    assert!(extra_attrs.contains(&"curl".to_owned()), "{extra_attrs:?}");
    assert!(!extra_attrs.contains(&"ripgrep".to_owned()));
    for row in extra["packages"].as_array().unwrap() {
        assert_eq!(row["repo_name"], "extra");
    }
    // `default` names the unnamed repository.
    let default = attrs(&api(&app, "/api/packages?repo=default").await, "packages");
    assert!(default.contains(&"ripgrep".to_owned()));
    assert!(!default.contains(&"curl".to_owned()));

    let search = attrs(&api(&app, "/api/packages?q=requ").await, "packages");
    assert_eq!(search, ["python3Packages.requests"]);

    let upstream = attrs(&api(&app, "/api/packages?upstream=pypi").await, "packages");
    assert!(upstream.contains(&"python3Packages.requests".to_owned()));
    assert!(!upstream.contains(&"ripgrep".to_owned()));

    let none = api(&app, "/api/packages?q=nothing-matches").await;
    assert_eq!(none["total"], 0);
    assert_eq!(len(&none["packages"]), 0);

    // Unknown values and every sort fall back gracefully.
    for uri in [
        "/api/packages?status=bogus",
        "/api/packages?sort=bogus&page=abc&limit=zzz",
        "/api/packages?sort=status",
        "/api/packages?sort=checked",
        "/api/packages?sort=rebuilds",
        "/api/packages?sort=failures",
        "/api/packages?sort=attr",
    ] {
        let body = api(&app, uri).await;
        assert_eq!(body["total"], 8, "{uri}");
        assert!(
            attrs(&body, "packages").contains(&"ripgrep".to_owned()),
            "{uri}"
        );
    }
    let bogus = api(&app, "/api/packages?sort=bogus").await;
    assert_eq!(bogus["sort"], all["sort"]);
    let by_attr = api(&app, "/api/packages?sort=attr").await;
    assert_eq!(by_attr["sort"], "attr");
    let sorted = attrs(&by_attr, "packages");
    let mut expected = sorted.clone();
    expected.sort();
    assert_eq!(sorted, expected);
}

#[tokio::test]
async fn api_packages_paginate() {
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

    let first = api(&app, "/api/packages?sort=attr").await;
    assert_eq!(first["total"], 68);
    assert_eq!(len(&first["packages"]), 50);
    assert!(attrs(&first, "packages").contains(&"bulk01".to_owned()));
    assert!(!attrs(&first, "packages").contains(&"ripgrep".to_owned()));

    let second = api(&app, "/api/packages?sort=attr&page=2").await;
    assert_eq!(second["page"], 2);
    assert_eq!(len(&second["packages"]), 18);
    assert!(attrs(&second, "packages").contains(&"ripgrep".to_owned()));

    let limited = api(&app, "/api/packages?limit=10&page=2").await;
    assert_eq!(len(&limited["packages"]), 10);
    assert_eq!(limited["per_page"], 10);

    // Past the end is an empty page, not an error.
    let beyond = api(&app, "/api/packages?page=99").await;
    assert_eq!(len(&beyond["packages"]), 0);
    assert_eq!(beyond["total"], 68);
}

#[tokio::test]
async fn api_package_detail() {
    let app = seeded_app().await;

    let package = api(&app, "/api/packages/python3Packages.requests").await;
    assert_eq!(package["attr_path"], "python3Packages.requests");
    assert_eq!(package["repo_name"], "");
    assert_eq!(package["update"]["status"], "failed");
    assert_eq!(len(&package["phases"]), 2);
    assert_eq!(len(&package["logs"]), 1);
    assert_eq!(len(&package["autofix"]["queue"]), 1);
    assert_eq!(len(&package["autofix"]["attempts"]), 2);
    assert!(package["index"].is_array());
    assert!(package["sources"].is_array());

    // Database content is returned verbatim; the frontend escapes it.
    let jq = api(&app, "/api/packages/jq").await;
    assert_eq!(
        jq["update"]["description"],
        "Lightweight <b>JSON</b> processor"
    );

    let multi = api(&app, "/api/packages/hello?repo=extra").await;
    assert_eq!(multi["repo_name"], "extra");
    assert_eq!(len(&multi["repos"]), 2);
    // Autofix rows are scoped by repository, like everything else.
    let queue = multi["autofix"]["queue"].as_array().unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["repo_name"], "extra");
    let attempts = multi["autofix"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["error_message"], "extra repo attempt");

    let unscoped = api(&app, "/api/packages/hello?repo=default").await;
    assert_eq!(unscoped["repo_name"], "");
    assert_eq!(len(&unscoped["autofix"]["queue"]), 0);
}

#[tokio::test]
async fn api_sessions() {
    let app = seeded_app().await;

    let visible = api(&app, "/api/sessions").await;
    assert_eq!(visible["total"], 3);
    assert_eq!(visible["hidden_empty"], 1);
    assert!(!visible["facets"].as_array().unwrap().is_empty());

    let completed = api(&app, "/api/sessions?status=completed").await;
    assert_eq!(completed["total"], 2, "empty watch sessions are hidden");

    let all = api(&app, "/api/sessions?show_empty=1").await;
    assert_eq!(all["total"], 4);
    assert_eq!(len(&all["sessions"]), 4);
    assert_eq!(all["hidden_empty"], 0);
    assert!(
        all["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == WATCH_EMPTY)
    );

    let running = api(&app, "/api/sessions?status=running").await;
    let ids: Vec<&str> = running["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [RUN_LIVE]);

    let watch = api(&app, "/api/sessions?mode=watch").await;
    let ids: Vec<&str> = watch["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [WATCH_DONE]);

    let cancelled = api(&app, "/api/sessions?status=cancelled").await;
    assert_eq!(cancelled["total"], 0);

    let detail = api(&app, &format!("/api/sessions/{RUN_DONE}")).await;
    assert_eq!(detail["session"]["id"], RUN_DONE);
    assert!(detail["session"].get("config_json").is_some());
    assert_eq!(len(&detail["phases"]), 7);
    let logs = attrs(&detail, "logs");
    assert_eq!(logs, ["python3Packages.requests"]);
    assert!(detail["cycle"].is_null());

    let from_watch = api(&app, &format!("/api/sessions/{WATCH_DONE}")).await;
    assert_eq!(from_watch["cycle"]["id"], 3);

    let phases = api(&app, &format!("/api/sessions/{RUN_DONE}/phases")).await;
    assert_eq!(len(&phases), 7);
    assert_eq!(phases[0]["phase"], "metadata_extraction");
}

#[tokio::test]
async fn api_failures() {
    let app = seeded_app().await;

    let all = api(&app, "/api/failures").await;
    assert_eq!(all["total"], 2);
    assert_eq!(all["per_page"], 25);
    assert!(all["facets"]["repos"].is_array());
    assert!(all["facets"]["phases"].is_array());
    assert!(all["facets"]["error_types"].is_array());

    let drvs = |body: &Value| -> Vec<String> {
        body["logs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["drv_path"].as_str().unwrap().to_owned())
            .collect()
    };
    let hash = api(&app, "/api/failures?error_type=HashError").await;
    assert_eq!(drvs(&hash), ["extra:hello@2.12"]);
    // Long logs are cut to their tail.
    let tail = hash["logs"][0]["log_tail"].as_str().unwrap();
    assert!(tail.ends_with("final line: HashError"), "{tail}");
    assert!(!tail.contains("START-OF-LOG"));
    assert!(hash["logs"][0]["log_len"].as_i64().unwrap() > 6000);

    let repo = api(&app, "/api/failures?repo=default").await;
    assert_eq!(attrs(&repo, "logs"), ["python3Packages.requests"]);

    let phase = api(&app, "/api/failures?phase=build&q=requests").await;
    assert_eq!(
        drvs(&phase),
        ["/nix/store/abc-python3.12-requests-2.32.0.drv"]
    );

    let none = api(&app, "/api/failures?q=zzz").await;
    assert_eq!(none["total"], 0);
}

#[tokio::test]
async fn api_watch() {
    let app = seeded_app().await;

    let watch = api(&app, "/api/watch").await;
    assert_eq!(len(&watch["repos"]), 2);
    assert!(watch["kind"].is_null());
    assert_eq!(watch["cycle_total"], 4);
    assert_eq!(watch["cycle_counts"]["all"], 4);
    assert_eq!(watch["cycle_counts"]["poll"], 3);
    assert_eq!(watch["cycle_counts"]["index"], 1);
    assert!(watch["last_poll"].is_object());
    assert!(!watch["source_types"].as_array().unwrap().is_empty());
    assert!(!watch["failing_sources"].as_array().unwrap().is_empty());
    assert!(watch["recent_releases"].is_array());

    let index = api(&app, "/api/watch?kind=index").await;
    assert_eq!(index["kind"], "index");
    assert_eq!(index["cycle_total"], 1);
    assert_eq!(index["cycles"][0]["id"], 1);

    let poll = api(&app, "/api/watch?kind=poll&limit=1").await;
    assert_eq!(poll["cycle_total"], 3);
    assert_eq!(len(&poll["cycles"]), 1);
    assert_eq!(poll["cycles"][0]["error"], "network unreachable");

    // An unknown kind means every kind.
    let bogus = api(&app, "/api/watch?kind=bogus").await;
    assert!(bogus["kind"].is_null());
    assert_eq!(bogus["cycle_total"], 4);
}

#[tokio::test]
async fn api_analytics_ranges() {
    let app = seeded_app().await;

    let default = api(&app, "/api/analytics").await;
    assert_eq!(default["range"], "30d");
    assert_eq!(default["days"], 30);
    assert_eq!(default["trend_days"], 30);
    for key in [
        "daily",
        "error_types",
        "phase_stats",
        "phase_percentiles",
        "status_by_repo",
        "upstream_types",
        "rebuild_bucket_order",
        "rebuild_buckets",
        "top_rebuilds",
        "autofix_attempts",
        "top_failing",
    ] {
        assert!(default[key].is_array(), "{key}: {}", default[key]);
    }
    assert!(default["outcomes"].is_object());
    assert_eq!(
        default["rebuild_bucket_order"].as_array().unwrap().last(),
        Some(&Value::from("unknown"))
    );
    assert!(!default["phase_stats"].as_array().unwrap().is_empty());

    let week = api(&app, "/api/analytics?range=7d").await;
    assert_eq!(week["range"], "7d");
    assert_eq!(week["trend_days"], 7);

    let all = api(&app, "/api/analytics?range=all").await;
    assert_eq!(all["range"], "all");
    assert_eq!(all["days"], 0);
    assert_eq!(all["trend_days"], 90);

    let bogus = api(&app, "/api/analytics?range=1y").await;
    assert_eq!(bogus["range"], "30d");
}

// ── Errors ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn api_unknown_things_are_json_404() {
    let app = seeded_app().await;
    for uri in [
        "/api/sessions/does-not-exist",
        "/api/sessions/does-not-exist/phases",
        "/api/packages/does-not-exist",
        "/api/packages/ripgrep?repo=extra",
        "/api/no/such/endpoint",
        "/api",
    ] {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{uri}");
        assert!(
            reply.content_type().starts_with("application/json"),
            "{uri}: {}",
            reply.content_type()
        );
        let body = reply.json();
        assert_eq!(body["status"], 404, "{uri}");
        assert!(!body["error"].as_str().unwrap().is_empty(), "{uri}");
    }
}

// ── Empty database ──────────────────────────────────────────────────────

#[tokio::test]
async fn empty_database_serves_every_endpoint() {
    let app = empty_app().await;
    for uri in [
        "/api/stats",
        "/api/dashboard",
        "/api/packages",
        "/api/sessions",
        "/api/failures",
        "/api/watch",
        "/api/analytics",
        "/api/analytics?range=all",
    ] {
        api(&app, uri).await;
    }
    let dash = api(&app, "/api/dashboard").await;
    assert_eq!(dash["counts"]["total"], 0);
    assert_eq!(len(&dash["running"]), 0);
    assert!(dash["watch"]["last_poll"].is_null());
    assert_eq!(api(&app, "/api/packages").await["total"], 0);
    assert_eq!(api(&app, "/api/sessions").await["total"], 0);
    assert_eq!(api(&app, "/api/failures").await["total"], 0);
    assert_eq!(api(&app, "/api/watch").await["cycle_total"], 0);

    let reply = get(&app, "/").await;
    assert_eq!(reply.status, StatusCode::OK);
}

// ── Static assets ───────────────────────────────────────────────────────

#[tokio::test]
async fn static_assets_are_embedded_and_cacheable() {
    let app = empty_app().await;
    let cases = [
        ("/static/css/styles.css", "text/css"),
        ("/static/js/elm.js", "text/javascript"),
        ("/static/js/app.js", "text/javascript"),
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

    // elm.js defines `Elm.Main` (compiled or stub), and the shell boots it.
    let elm = get(&app, "/static/js/elm.js").await.body;
    assert!(elm.contains("Elm"), "elm.js does not look like Elm output");
    // Without `elm`, build.rs embeds a stub; it must still satisfy app.js.
    let stub = elm.contains("frontend not built");
    assert_eq!(
        stub,
        crate::assets::FRONTEND == "stub",
        "{}",
        crate::assets::FRONTEND
    );
    if stub {
        assert!(elm.contains("Elm = { Main: { init: init } }"));
        assert!(elm.contains("/api/dashboard"));
    }
    let app_js = get(&app, "/static/js/app.js").await.body;
    assert!(app_js.contains("Elm.Main.init"));
    assert!(app_js.contains("/ws/live"));

    for uri in ["/static/css/nope.css", "/static/js/htmx.min.js"] {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{uri}");
    }
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
