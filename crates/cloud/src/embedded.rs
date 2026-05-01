//! Serve the cloud SPA bundle from bytes embedded into this binary.
//!
//! Replaces the old `tower_http::ServeDir::new(/usr/share/bananas/
//! cloud-ui).precompressed_br().precompressed_gzip()` setup. The
//! daemon's `build.rs` writes the dist tree to `$OUT_DIR/ui/` (built
//! by `dx build` and pre-compressed with brotli + gzip companions);
//! `include_dir!` snapshots the whole tree into the binary at compile
//! time. The handler below picks the right variant based on the
//! incoming `Accept-Encoding` header — same negotiation behaviour
//! ServeDir provided.
//!
//! Two responsibilities:
//!  - **/cloud/assets/<hash>.<ext>** lookups: exact-path match in the
//!    embedded tree, with `.br` / `.gz` companion preference. Tagged
//!    `Cache-Control: public, max-age=31536000, immutable` because
//!    dx-cli emits content-hashed filenames.
//!  - **SPA fallback**: any path that doesn't match (e.g. /cloud/, a
//!    deep-link) returns `index.html`, also accept-encoding-negotiated.

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use include_dir::{Dir, include_dir};

/// The full SPA dist tree, baked in at compile time. `build.rs`
/// guarantees `$OUT_DIR/ui/index.html` exists before this macro
/// expands.
static UI: Dir<'_> = include_dir!("$OUT_DIR/ui");

/// Cache header for assets — content-hashed filenames are immutable,
/// so we pin them in the browser cache for a year. index.html itself
/// uses a short cache so SPA upgrades roll out promptly.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const NO_CACHE: &str = "no-cache";

/// Serve `<rest>` from the embedded UI tree, with content-encoding
/// negotiation. Falls back to `index.html` for SPA routes (anything
/// not matching a literal asset path).
///
/// Caller is responsible for stripping the daemon's mount prefix
/// (`/cloud/`) so `rest` is relative to the SPA's own root (e.g.
/// `assets/main-<hash>.css` or `index.html`).
pub fn serve(rest: &str, req: &Request) -> Response {
    // Empty / trailing-slash → index.html (SPA root).
    let cleaned = rest.trim_start_matches('/');
    let lookup_path = if cleaned.is_empty() {
        "index.html"
    } else {
        cleaned
    };

    let accept = req
        .headers()
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some(resp) = pick(lookup_path, accept, /*is_index=*/ false) {
        return resp;
    }

    // SPA fallback — deep links like `/` (with hash routes) end up
    // here when no exact asset matches. We DON'T fall back for paths
    // under `assets/` though: those are content-hashed by dx, so a
    // miss is genuine breakage and should surface as 404 rather than
    // silently serving HTML the browser will choke on parsing as JS.
    if lookup_path.starts_with("assets/") {
        return not_found(&format!("asset not found in embedded UI: {lookup_path}"));
    }
    pick("index.html", accept, /*is_index=*/ true)
        .unwrap_or_else(|| not_found("cloud-ui bundle is empty — build.rs may have failed"))
}

/// Try the .br / .gz / raw variants in that order, returning the
/// first the client accepts. `is_index` toggles caching: index.html
/// gets `no-cache` so updates land on next refresh; everything else
/// (content-hashed assets) gets `immutable`.
fn pick(path: &str, accept: &str, is_index: bool) -> Option<Response> {
    if accept.split(',').any(|tok| tok.trim().starts_with("br")) {
        if let Some(file) = UI.get_file(format!("{path}.br")) {
            return Some(build_response(
                file.contents(),
                Some("br"),
                content_type_for(path),
                is_index,
            ));
        }
    }
    if accept.split(',').any(|tok| tok.trim().starts_with("gzip")) {
        if let Some(file) = UI.get_file(format!("{path}.gz")) {
            return Some(build_response(
                file.contents(),
                Some("gzip"),
                content_type_for(path),
                is_index,
            ));
        }
    }
    UI.get_file(path)
        .map(|file| build_response(file.contents(), None, content_type_for(path), is_index))
}

fn build_response(
    bytes: &'static [u8],
    encoding: Option<&'static str>,
    content_type: &'static str,
    is_index: bool,
) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CACHE_CONTROL,
            if is_index { NO_CACHE } else { IMMUTABLE },
        );
    if let Some(enc) = encoding {
        builder = builder.header(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
        // Tell intermediaries that content varies on Accept-Encoding
        // so they don't serve a brotli payload to a gzip-only client.
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
