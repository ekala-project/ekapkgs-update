//! The single-page app shell and its static assets, embedded in the binary.
//!
//! `elm.js` is compiled by `build.rs` (or handed to it by the Nix build via
//! `EKAPKGS_WEB_ELM_JS`) into `OUT_DIR`. Without `elm` it is a stub that only
//! shows a "frontend not built" notice (see [`FRONTEND`]); the stylesheet and the ports glue
//! (`app.js`) are plain files. Everything is compiled in with
//! `include_str!`, so the server does not depend on its working directory
//! (the NixOS unit runs it from `/`). Asset URLs carry a content hash
//! (`?v=…`) and an `ETag`, so browsers may cache them for a day and still
//! pick up a new build immediately.

use std::sync::LazyLock;

use axum::extract::Path;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

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
        path: "js/elm.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_str!(concat!(env!("OUT_DIR"), "/elm.js")),
    },
    Asset {
        path: "js/app.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_str!("../static/js/app.js"),
    },
];

/// Where the embedded `elm.js` came from: `prebuilt` (`EKAPKGS_WEB_ELM_JS`),
/// `elm` (compiled by `build.rs`) or `stub` (`elm` was not on `PATH`).
#[cfg(test)]
pub const FRONTEND: &str = env!("EKAPKGS_WEB_FRONTEND");

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

/// The HTML shell every page route returns. The Elm app renders into
/// `<body>`; the `<noscript>` text is all a browser without JavaScript sees.
static INDEX_HTML: LazyLock<String> = LazyLock::new(|| {
    let v = VERSION.as_str();
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="color-scheme" content="light dark">
  <title>ekapkgs-update</title>
  <link rel="stylesheet" href="/static/css/styles.css?v={v}">
  <script src="/static/js/elm.js?v={v}"></script>
  <script src="/static/js/app.js?v={v}" defer></script>
</head>
<body>
  <noscript><p class="noscript">The ekapkgs-update dashboard needs JavaScript. The same data is available as JSON under <a href="/api/dashboard">/api</a>.</p></noscript>
</body>
</html>
"#
    )
});

/// Respond with the app shell. Page routes use 200; unknown paths use 404
/// (the frontend then shows its "not found" page).
pub fn index_response(status: StatusCode) -> Response {
    let mut response = (status, Html(INDEX_HTML.as_str())).into_response();
    // The shell names versioned assets, so it must never be cached itself.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// `GET /static/*path`
pub async fn serve(Path(path): Path<String>, headers: HeaderMap) -> Response {
    let Some(asset) = ASSETS.iter().find(|a| a.path == path) else {
        return (
            StatusCode::NOT_FOUND,
            format!("No static asset named “{path}”."),
        )
            .into_response();
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
