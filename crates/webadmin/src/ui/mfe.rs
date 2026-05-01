//! Microfrontend dynamic-loader for plugin SPAs.
//!
//! The host shell composes plugin SPAs into a single page at
//! runtime instead of full-page navigating to a per-plugin URL.
//! Pattern: the Rust+Dioxus version of single-spa / Module
//! Federation — host discovers plugins, fetches each one's
//! content-hashed entry script via JSON handshake, injects the
//! `<script type="module">` tag, and the plugin's Dioxus runtime
//! mounts into a host-owned div.
//!
//! Sequence (first activation of a plugin tab):
//! 1. Shell renders an empty `<div id="<id>-mfe-root">`.
//! 2. Shell fetches `/api/<id>/__mfe_entry` — auth-gated JSON
//!    handshake on the plugin daemon. The response is `{"entry":
//!    "/assets/<id>/<hash>.js"}`. The URL is in webadmin's public
//!    namespace; the plugin daemon's Dioxus.toml `base_path`
//!    pre-baked it into the embedded index.html.
//! 3. Shell sets `window.__bananas_mfe_root = "<id>-mfe-root"` so
//!    the plugin's `main()` knows where to mount (defaults to
//!    `"main"` for standalone dev access).
//! 4. Shell appends `<script type="module" src="...">` to `<head>`.
//!    The shim resolves its companion `.wasm` via `import.meta.url`
//!    relative to the script URL.
//! 5. Plugin wasm runs, `main()` mounts its Dioxus app into the
//!    shell-provided div. Two Dioxus runtimes coexist in one
//!    document, sharing cookie / theme attribute / location.
//!
//! Subsequent activations: the `<script>` tag is already in the
//! document (`is_loaded` returns true) — we skip the handshake
//! and the shell just toggles the mount div's `display:none`.

use serde::Deserialize;
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

#[derive(Deserialize)]
struct MfeEntry {
    entry: String,
}

/// Fetch `/api/<plugin_id>/__mfe_entry`, return the `entry` field.
/// The response shape is `{"entry": "/assets/<id>/<hash>.js"}` and
/// the URL is in webadmin's public namespace, ready to drop into a
/// `<script src>` attribute.
pub async fn discover_entry(plugin_id: &str) -> Result<String, String> {
    let url = format!("/api/{plugin_id}/__mfe_entry");
    let resp = gloo_net::http::Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("fetch {url}: {e}"))?;
    if !resp.ok() {
        return Err(format!("{url} returned HTTP {}", resp.status()));
    }
    let body: MfeEntry = resp
        .json()
        .await
        .map_err(|e| format!("{url} JSON parse: {e}"))?;
    if body.entry.is_empty() {
        return Err(format!("{url} returned empty entry"));
    }
    Ok(body.entry)
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
pub async fn load(plugin_id: &str) -> Result<(), String> {
    if is_loaded(plugin_id) {
        return Ok(());
    }
    let entry = discover_entry(plugin_id).await?;
    inject(plugin_id, &entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_id_format() {
        assert_eq!(mount_id("cloud"), "cloud-mfe-root");
    }
}
