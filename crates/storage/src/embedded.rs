//! Serve the storage SPA's static assets from bytes embedded into
//! this binary. **Strict asset lookup only** — `index.html` is
//! never publicly served. Webadmin owns THE single document root;
//! this daemon answers only `GET /assets/storage/<rest>` requests
//! that webadmin sub-proxies over the Unix socket.
//!
//! The daemon's `build.rs` writes the dx-emitted dist tree to
//! `$OUT_DIR/ui/` (with `.br` and `.gz` companions); `include_dir!`
//! snapshots the tree into the binary at compile time. The serve
//! handler picks the right encoding variant from `Accept-Encoding`
//! exactly the way `tower_http::ServeDir.precompressed_*()` would.

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use include_dir::{Dir, include_dir};

/// The full SPA dist tree, baked in at compile time. `build.rs`
/// has already stripped `index.html` (+ companions) from the
/// tree — the daemon never serves the document root publicly.
static UI: Dir<'_> = include_dir!("$OUT_DIR/ui");

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// The MFE entry script URL — extracted from the dx-emitted
/// `index.html` at build time and inlined as a `&'static str`.
/// Returned by the
/// `/api/storage/__mfe_entry` JSON endpoint so the host SPA's MFE
/// loader knows which content-hashed JS shim to inject.
///
/// Dioxus.toml's `base_path = "/assets/storage"` causes dx to emit
/// `<script type="module" src="/assets/storage/bananas-storage-ui-<hash>.js">`,
/// which is the public URL exactly — webadmin sub-proxies that
/// path back to this daemon's socket where the embedded tree
/// serves the actual bytes.
pub const MFE_ENTRY: &str = include_str!(concat!(env!("OUT_DIR"), "/mfe_entry.txt"));

/// Serve `<rest>` from the embedded UI tree. The caller hands us
/// the request path with the `/assets/storage/` prefix already
/// stripped, so `rest` looks like:
///  - `assets/bananas-storage-ui-<hash>.js` (dx emits hashed files
///    under its own `assets/` subdir; combined with our
///    `base_path = "/assets/storage"`, the public URL becomes
///    `/assets/storage/assets/<hash>.js`)
///  - `wasm/bananas-storage-ui_bg.wasm` (older dx layout)
///  - `main.css` (top-level static asset)
///
/// Strict semantics:
///  - Looks up `<rest>` verbatim in the embedded tree (rooted at
///    dx's `public/`).
///  - **Refuses to serve `index.html`** — `build.rs` strips it
///    from the embed tree at compile time, so this is just
///    defense-in-depth.
///  - Picks the best encoding variant the client accepts.
///  - Returns 404 for any other miss. No SPA fallback; webadmin
///    owns all HTML payloads.
pub fn serve(rest: &str, req: &Request) -> Response {
    let cleaned = rest.trim_start_matches('/');
    if cleaned.is_empty() || cleaned == "index.html" || cleaned.ends_with("/index.html") {
        return not_found("not a public asset path");
    }

    let accept = req
        .headers()
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    pick(cleaned, accept)
        .unwrap_or_else(|| not_found(&format!("asset not found in embedded UI: {cleaned}")))
}

/// Try the .br / .gz / raw variants in that order, returning the
/// first the client accepts. All assets are content-hashed so the
/// response always carries the immutable cache directive.
fn pick(path: &str, accept: &str) -> Option<Response> {
    if accept.split(',').any(|tok| tok.trim().starts_with("br")) {
        if let Some(file) = UI.get_file(format!("{path}.br")) {
            return Some(build_response(
                file.contents(),
                Some("br"),
                content_type_for(path),
            ));
        }
    }
    if accept.split(',').any(|tok| tok.trim().starts_with("gzip")) {
        if let Some(file) = UI.get_file(format!("{path}.gz")) {
            return Some(build_response(
                file.contents(),
                Some("gzip"),
                content_type_for(path),
            ));
        }
    }
    UI.get_file(path)
        .map(|file| build_response(file.contents(), None, content_type_for(path)))
}

fn build_response(
    bytes: &'static [u8],
    encoding: Option<&'static str>,
    content_type: &'static str,
) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, IMMUTABLE);
    if let Some(enc) = encoding {
        builder = builder.header(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
        builder = builder.header(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }
    builder.body(Body::from(bytes)).expect("response builder")
}

fn not_found(msg: &str) -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(msg.to_string()))
        .expect("404 response")
}

fn content_type_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/vnd.microsoft.icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}
