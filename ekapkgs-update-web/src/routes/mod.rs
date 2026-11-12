//! HTTP routes. Every handler is read-only.

pub mod analytics;
pub mod api;
pub mod dashboard;
pub mod failures;
pub mod packages;
pub mod sessions;
pub mod watch;
pub mod ws;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use axum::Router;
use axum::routing::get;

use crate::format::{Chip, non_empty};
use crate::state::AppState;
use crate::{assets, error};

/// Packages per page on `/packages`.
pub const PACKAGES_PER_PAGE: i64 = 50;
/// Sessions per page on `/sessions`.
pub const SESSIONS_PER_PAGE: i64 = 50;
/// Failure logs per page on `/failures`.
pub const FAILURES_PER_PAGE: i64 = 25;
/// Packages per page in a session's rollup.
pub const ROLLUP_PER_PAGE: i64 = 100;
/// Watch cycles per page on `/watch`.
pub const CYCLES_PER_PAGE: i64 = 25;

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

/// A chip (filter tab) linking to `href`.
pub fn chip(
    label: &str,
    count: Option<i64>,
    href: String,
    active: bool,
    tone: &'static str,
) -> Chip {
    Chip {
        label: label.to_owned(),
        count: count.map(crate::format::thousands),
        href,
        active,
        tone,
    }
}

/// The full application router.
pub fn router(state: AppState) -> Router {
    Router::new()
        // Pages
        .route("/", get(dashboard::index))
        .route("/packages", get(packages::list))
        .route("/packages/:attr", get(packages::detail))
        .route("/sessions", get(sessions::list))
        .route("/sessions/:id", get(sessions::detail))
        .route("/failures", get(failures::list))
        .route("/watch", get(watch::index))
        .route("/analytics", get(analytics::index))
        // HTMX fragments (live panels)
        .route("/fragments/stats", get(dashboard::stats_fragment))
        .route("/fragments/active", get(dashboard::active_fragment))
        .route("/fragments/watch", get(dashboard::watch_fragment))
        .route("/fragments/activity", get(dashboard::activity_fragment))
        .route("/fragments/sessions", get(sessions::list_fragment))
        .route("/fragments/sessions/:id", get(sessions::live_fragment))
        // JSON API
        .route("/api/stats", get(api::stats))
        .route("/api/sessions", get(api::sessions))
        .route("/api/sessions/:id/phases", get(api::session_phases))
        .route("/api/packages", get(api::packages))
        .route("/api/packages/:attr", get(api::package))
        .route("/api/watch", get(api::watch))
        // Live change notifications
        .route("/ws/live", get(ws::live))
        // Embedded assets
        .route("/static/*path", get(assets::serve))
        .fallback(error::fallback)
        .with_state(state)
}
