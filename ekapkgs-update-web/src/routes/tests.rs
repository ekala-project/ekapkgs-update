use axum::Router;
use axum::body::Body;
use axum::routing::get;
use http_body_util::BodyExt;
use hyper::Request;
use sqlx::SqlitePool;
use tower::ServiceExt;

use crate::routes;
use crate::state::AppState;

async fn setup_test_app() -> Router {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

    // Run migrations to create tables
    sqlx::migrate!("../ekapkgs-update/migrations")
        .run(&pool)
        .await
        .unwrap();

    let db = ekapkgs_update::database::Database::from(pool);
    let state = AppState::new(db);

    Router::new()
        .route("/", get(routes::dashboard::index))
        .route("/sessions", get(routes::sessions::list))
        .route("/packages", get(routes::packages::list))
        .route("/analytics", get(routes::analytics::index))
        .route("/api/stats", get(routes::dashboard::stats_json))
        .route("/api/sessions", get(routes::sessions::list_json))
        .with_state(state)
}

#[tokio::test]
async fn dashboard_returns_200() {
    let app = setup_test_app().await;
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn sessions_returns_200() {
    let app = setup_test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn packages_returns_200() {
    let app = setup_test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/packages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn packages_search_returns_200() {
    let app = setup_test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/packages?search=hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn packages_search_sql_injection_safe() {
    let app = setup_test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/packages?search=%27%3B+DROP+TABLE+updates%3B+--")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn analytics_returns_200() {
    let app = setup_test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/analytics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn stats_json_returns_valid_json() {
    let app = setup_test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("total_packages").is_some());
    assert!(json.get("success_rate").is_some());
    assert!(json.get("active_updates").is_some());
    assert!(json.get("total_sessions").is_some());
}

#[tokio::test]
async fn sessions_json_returns_valid_json() {
    let app = setup_test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
}
