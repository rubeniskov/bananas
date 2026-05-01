//! Microfrontend dynamic-loader for plugin SPAs.
//!
//! Each plugin (bananas-cloud today, others later) ships its own wasm
//! SPA bundle, served by its daemon at `<spa_path>/`. The webadmin
//! shell composes them into a single page at runtime instead of full-
//! page navigating to `/cloud/`. The pattern is the Rust+Dioxus
//! version of single-spa / Module Federation: the host discovers
//! plugins, loads each plugin's bundle on demand, and the plugin's
//! Dioxus runtime mounts into a host-owned div.
//!
//! Sequence (first activation of a plugin tab):
//! 1. Shell renders an empty `<div id="<id>-mfe-root">`.
//! 2. Shell fetches `<spa_path>/index.html` to find the bundle's JS
//!    shim URL — dx-cli emits a content-hashed filename so we can't
//!    hardcode it.
//! 3. Shell sets `window.__bananas_mfe_root = "<id>-mfe-root"` so the
//!    plugin's `main()` knows where to mount (defaults to `"main"`
//!    for standalone access at `/cloud/`).
//! 4. Shell appends `<script type="module" src="...">` to `<head>`.
//!    The shim resolves its companion `.wasm` via `import.meta.url`
//!    (cloud-ui's `Dioxus.toml` sets `base_path = "/cloud"` so the
//!    relative URL lands on the right path).
//! 5. The plugin's wasm runs, `main()` mounts its Dioxus app into
//!    the shell-provided div. Two Dioxus runtimes coexist in one
//!    document, sharing cookie / theme attribute / location.
//!
//! Subsequent activations: the script tag is already in the document
//! (`is_loaded` returns true) — we skip discovery entirely and the
//! shell just toggles the mount div's `display:none`.

use wasm_bindgen::JsValue;

/// DOM id of the mount div the shell renders for `plugin_id`. The
/// plugin's `main()` reads `window.__bananas_mfe_root` to find this.
pub fn mount_id(plugin_id: &str) -> String {
    format!("{plugin_id}-mfe-root")
}

/// True if the plugin's `<script>` tag is already in the document.
/// We tag every injected script with `data-bananas-mfe="<id>"` for
/// exactly this lookup — avoids a second wasm fetch + reinit on tab
/// re-activation.
pub fn is_loaded(plugin_id: &str) -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let Some(document) = window.document() else {
        return false;
    };
    let selector = format!("script[data-bananas-mfe=\"{plugin_id}\"]");
    matches!(document.query_selector(&selector), Ok(Some(_)))
}

/// Fetch `<spa_path>/index.html`, extract the first
/// `<script type="module" ...>` tag's `src` attribute, and resolve it
/// to an absolute URL the shell can inject.
pub async fn discover_entry(spa_path: &str) -> Result<String, String> {
    let url = format!("{}/index.html", spa_path.trim_end_matches('/'));
    let resp = gloo_net::http::Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("fetch {url}: {e}"))?;
    if !resp.ok() {
        return Err(format!("{url} returned HTTP {}", resp.status()));
    }
    let html = resp.text().await.map_err(|e| e.to_string())?;
    extract_module_src(&html, spa_path)
        .ok_or_else(|| format!("no <script type=\"module\"> found in {url}"))
}

/// Scan `html` for the first `<script ... type="module" ... src="...">`
/// tag and return its src resolved against `spa_path` (so a relative
/// `./assets/foo.js` becomes `/cloud/assets/foo.js`).
///
/// Plain string scanning rather than a real HTML parser — dx-cli's
/// emitted index.html is small and deterministic. If dx-cli ever
/// changes the shape this returns None and the loader fails fast with
/// a recognizable error, rather than silently mis-parsing.
fn extract_module_src(html: &str, spa_path: &str) -> Option<String> {
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
                        return Some(resolve_url(&after[..end], spa_path));
                    }
                }
            }
        }
        cursor = tag_start + close + 1;
    }
    None
}

fn resolve_url(src: &str, spa_path: &str) -> String {
    if src.starts_with('/') || src.starts_with("http://") || src.starts_with("https://") {
        src.to_string()
    } else {
        let trimmed = src.trim_start_matches("./");
        format!("{}/{}", spa_path.trim_end_matches('/'), trimmed)
    }
}

/// Set the global mount target and append the plugin's JS shim to
/// `<head>`. Idempotent on `is_loaded` — callers should check first
/// rather than relying on the script tag's own dedup behaviour.
pub fn inject(plugin_id: &str, entry_url: &str) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;

    // Tell the plugin where to mount before its script runs. Plugins
    // read this exactly once on boot in their `main()`.
    js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__bananas_mfe_root"),
        &JsValue::from_str(&mount_id(plugin_id)),
    )
    .map_err(|_| "set window.__bananas_mfe_root failed".to_string())?;

    let script = document
        .create_element("script")
        .map_err(|_| "create_element(script) failed".to_string())?;
    script
        .set_attribute("type", "module")
        .map_err(|_| "set type=module failed".to_string())?;
    script
        .set_attribute("src", entry_url)
        .map_err(|_| "set src failed".to_string())?;
    script
        .set_attribute("data-bananas-mfe", plugin_id)
        .map_err(|_| "set data-bananas-mfe failed".to_string())?;

    // Fetch <head> via querySelector rather than Document::head()
    // to avoid pulling in the HtmlHeadElement web-sys feature.
    let head = document
        .query_selector("head")
        .map_err(|_| "query_selector(head) failed".to_string())?
        .ok_or_else(|| "no <head> element".to_string())?;
    head.append_child(&script)
        .map_err(|_| "append_child(script) failed".to_string())?;
    Ok(())
}

/// One-shot load: discover the entry, inject the script. No-op when
/// the plugin is already loaded. Caller is responsible for rendering
/// the mount div (so the plugin's `main()` finds it on boot).
pub async fn load(plugin_id: &str, spa_path: &str) -> Result<(), String> {
    if is_loaded(plugin_id) {
        return Ok(());
    }
    let entry = discover_entry(spa_path).await?;
    inject(plugin_id, &entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_module_src_double_quoted() {
        let html = r#"<!doctype html><html><body>
            <div id="main"></div>
            <script type="module" src="/cloud/assets/bananas-cloud-ui-deadbeef.js"></script>
        </body></html>"#;
        assert_eq!(
            extract_module_src(html, "/cloud").as_deref(),
            Some("/cloud/assets/bananas-cloud-ui-deadbeef.js")
        );
    }

    #[test]
    fn extracts_module_src_single_quoted() {
        let html = "<script type='module' src='./assets/foo.js'></script>";
        assert_eq!(
            extract_module_src(html, "/cloud").as_deref(),
            Some("/cloud/assets/foo.js")
        );
    }

    #[test]
    fn ignores_non_module_scripts() {
        let html = r#"
            <script src="/legacy/old.js"></script>
            <script type="module" src="/cloud/assets/right.js"></script>
        "#;
        assert_eq!(
            extract_module_src(html, "/cloud").as_deref(),
            Some("/cloud/assets/right.js")
        );
    }

    #[test]
    fn missing_module_returns_none() {
        let html = "<script src=\"/legacy/old.js\"></script>";
        assert!(extract_module_src(html, "/cloud").is_none());
    }

    #[test]
    fn resolves_relative_against_spa_path() {
        assert_eq!(
            resolve_url("assets/foo.js", "/cloud"),
            "/cloud/assets/foo.js"
        );
        assert_eq!(
            resolve_url("./assets/foo.js", "/cloud/"),
            "/cloud/assets/foo.js"
        );
        assert_eq!(resolve_url("/abs/foo.js", "/cloud"), "/abs/foo.js");
        assert_eq!(
            resolve_url("https://cdn/foo.js", "/cloud"),
            "https://cdn/foo.js"
        );
    }

    #[test]
    fn mount_id_format() {
        assert_eq!(mount_id("cloud"), "cloud-mfe-root");
    }
}
