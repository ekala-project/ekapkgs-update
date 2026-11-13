//! HTTP routes. Every handler is read-only.
//!
//! The dashboard is an Elm single-page app: every page path returns the
//! same HTML shell ([`assets::index_response`]) and the app fetches its data
//! from the JSON API under `/api`. Unknown `/api` paths are JSON 404s;
//! any other unknown path returns the shell with status 404, which the app
//! renders as its "not found" page.
//!
//! Responses are gzip-compressed (when `Accept-Encoding` allows) by a
//! [`CompressionLayer`] that is part of [`router`], so the tests exercise
//! it too. Brotli is not enabled: at tower-http's default quality it came
//! out larger than gzip for `elm.js`, and browsers prefer it when offered. Bodies under 32 bytes, 304s and the `/ws/live` upgrade (empty
//! bodies) pass through unchanged.

pub mod api;
pub mod ws;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use axum::Router;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use tower_http::compression::CompressionLayer;

use crate::assets;
use crate::params::non_empty;
use crate::state::AppState;

/// Query parameters, read leniently: unknown keys are ignored and malformed
/// values fall back to defaults instead of producing a 400.
pub type Params = HashMap<String, String>;

/// A trimmed, non-empty query parameter.
pub fn param(params: &Params, key: &str) -> Option<String> {
    non_empty(params.get(key))
}

/// A boolean flag parameter (`1`, `true`, `yes`, `on`).
pub fn flag(params: &Params, key: &str) -> bool {
    params
        .get(key)
        .is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
}

/// Client-side routes (see `frontend/src/Route.elm`).
const PAGES: [&str; 8] = [
    "/",
    "/packages",
    "/packages/:attr",
    "/sessions",
    "/sessions/:id",
    "/failures",
    "/watch",
    "/analytics",
];

async fn page() -> Response {
    assets::index_response(StatusCode::OK)
}

async fn fallback() -> Response {
    assets::index_response(StatusCode::NOT_FOUND)
}

/// The full application router.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/stats", get(api::stats))
        .route("/dashboard", get(api::dashboard))
        .route("/packages", get(api::packages))
        .route("/packages/:attr", get(api::package))
        .route("/sessions", get(api::sessions))
        .route("/sessions/:id", get(api::session))
        .route("/sessions/:id/phases", get(api::session_phases))
        .route("/failures", get(api::failures))
        .route("/watch", get(api::watch))
        .route("/analytics", get(api::analytics))
        .fallback(api::not_found);

    let mut router = Router::new();
    for path in PAGES {
        router = router.route(path, get(page));
    }
    router
        .nest("/api", api)
        // Live change notifications
        .route("/ws/live", get(ws::live))
        // Embedded assets
        .route("/static/*path", get(assets::serve))
        .fallback(fallback)
        .with_state(state)
        .layer(CompressionLayer::new())
}
