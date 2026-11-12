//! Static assets embedded in the binary.
//!
//! CSS and JavaScript are compiled in with `include_str!`, so the server does
//! not depend on its working directory (the NixOS unit runs it from `/`).
//! Asset URLs carry a content hash (`?v=…`) and an `ETag`, so browsers may
//! cache them for a day and still pick up a new build immediately.

use std::sync::LazyLock;

use axum::extract::Path;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::error::AppError;

/// One embedded file.
struct Asset {
    path: &'static str,
    content_type: &'static str,
    body: &'static str,
}

const ASSETS: &[Asset] = &[
    Asset {
        path: "css/styles.css",
        content_type: "text/css; charset=utf-8",
        body: include_str!("../static/css/styles.css"),
    },
    Asset {
        path: "js/htmx.min.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_str!("../static/js/htmx.min.js"),
    },
    Asset {
        path: "js/live.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_str!("../static/js/live.js"),
    },
];

const CACHE_CONTROL: &str = "public, max-age=86400";

/// FNV-1a, used for cache-busting tokens and live-update hashes.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Combined hash of every asset, appended to asset URLs as `?v=`.
static VERSION: LazyLock<String> = LazyLock::new(|| {
    let combined = ASSETS.iter().fold(String::new(), |acc, a| {
        format!("{acc}{:016x}", fnv1a(a.body.as_bytes()))
    });
    format!("{:012x}", fnv1a(combined.as_bytes()) & 0xffff_ffff_ffff)
});

/// Cache-busting token for asset URLs (used by `base.html`).
pub fn version() -> &'static str {
    VERSION.as_str()
}

/// `GET /static/*path`
pub async fn serve(Path(path): Path<String>, headers: HeaderMap) -> Response {
    let Some(asset) = ASSETS.iter().find(|a| a.path == path) else {
        return AppError::NotFound(format!("No static asset named “{path}”.")).into_response();
    };
    let etag = format!("\"{:016x}\"", fnv1a(asset.body.as_bytes()));

    let not_modified = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag));

    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        asset.body.into_response()
    };
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(asset.content_type),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(CACHE_CONTROL),
    );
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    response
}
