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

use std::sync::LazyLock;

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

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// The MFE entry script URL — extracted from the embedded
/// `index.html` once on first read. Returned by the
/// `/api/storage/__mfe_entry` JSON endpoint so the host SPA's MFE
/// loader knows which content-hashed JS shim to inject.
///
/// Dioxus.toml's `base_path = "/assets/storage"` causes dx to emit
/// `<script type="module" src="/assets/storage/bananas-storage-ui-<hash>.js">`,
/// which is the public URL exactly — webadmin sub-proxies that
/// path back to this daemon's socket where the embedded tree
/// serves the actual bytes.
pub static MFE_ENTRY: LazyLock<String> = LazyLock::new(|| {
    let html = UI
        .get_file("index.html")
        .expect("embedded UI tree missing index.html — build.rs ran but produced no SPA")
        .contents_utf8()
        .expect("embedded index.html is not valid UTF-8");
    extract_module_src(html)
        .expect("no <script type=\"module\" src=\"…\"> in embedded storage-ui index.html")
});

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
///  - **Refuses to serve `index.html`** — that file exists in the
///    tree but is consumed only by `MFE_ENTRY` extraction; it's
///    never publicly returned.
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

/// Plain-string scan of `html` for the first
/// `<script type="module" ... src="...">` tag and return its src.
/// dx-cli's emitted index.html is small and deterministic, so a
/// real HTML parser is overkill. The src we extract is the *full*
/// public URL (Dioxus.toml's `base_path` already prefixed it with
/// `/assets/storage/`); we return it as-is.
fn extract_module_src(html: &str) -> Option<String> {
    let mut cursor = 0usize;
    while cursor < html.len() {
        let rel = html[cursor..].find("<script")?;
        let tag_start = cursor + rel;
        let close = html[tag_start..].find('>')?;
        let tag = &html[tag_start..tag_start + close];
        let is_module = tag.contains("type=\"module\"") || tag.contains("type='module'");
        if is_module {
            for needle in ["src=\"", "src='"] {
                if let Some(pos) = tag.find(needle) {
                    let after = &tag[pos + needle.len()..];
                    let quote = needle.chars().last().unwrap();
                    if let Some(end) = after.find(quote) {
                        return Some(after[..end].to_string());
                    }
                }
            }
        }
        cursor = tag_start + close + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_module_src_double_quoted() {
        let html = r#"<!doctype html><html><body>
            <div id="main"></div>
            <script type="module" src="/assets/storage/bananas-storage-ui-deadbeef.js"></script>
        </body></html>"#;
        assert_eq!(
            extract_module_src(html).as_deref(),
            Some("/assets/storage/bananas-storage-ui-deadbeef.js")
        );
    }

    #[test]
    fn extracts_module_src_single_quoted() {
        let html = "<script type='module' src='/assets/storage/foo.js'></script>";
        assert_eq!(
            extract_module_src(html).as_deref(),
            Some("/assets/storage/foo.js")
        );
    }

    #[test]
    fn ignores_non_module_scripts() {
        let html = r#"
            <script src="/legacy/old.js"></script>
            <script type="module" src="/assets/storage/right.js"></script>
        "#;
        assert_eq!(
            extract_module_src(html).as_deref(),
            Some("/assets/storage/right.js")
        );
    }

    #[test]
    fn missing_module_returns_none() {
        let html = "<script src=\"/legacy/old.js\"></script>";
        assert!(extract_module_src(html).is_none());
    }
}
