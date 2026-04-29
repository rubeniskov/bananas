//! Modal that loads, edits, and saves the bananas-stats TOML config.
//! Save round-trips through /api/stats/config which calls the helper
//! to atomic-replace /etc/bananas/stats.toml and `systemctl restart
//! bananas-stats.service`.

#![allow(non_snake_case)]

use dioxus::prelude::*;
use gloo_net::http::Request;
use serde::Deserialize;
use serde_json::json;

use crate::{AuthCtx, icons::Icon};

#[derive(Props, Clone, PartialEq)]
pub struct StatsConfigModalProps {
    pub on_close: EventHandler<()>,
    pub on_saved: EventHandler<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ConfigResp {
    #[serde(default)]
    config: String,
}

#[component]
pub fn StatsConfigModal(props: StatsConfigModalProps) -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let mut text = use_signal(String::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut busy = use_signal(|| false);
    let mut hydrated = use_signal(|| false);
    let mut load_failed = use_signal(|| false);

    // First-load hydrate.
    use_effect(move || {
        if hydrated() { return; }
        spawn(async move {
            match Request::get("/api/stats/config").send().await {
                Ok(r) if r.status() == 401 => auth_ctx.signal_unauthorized(),
                Ok(r) if r.ok() => match r.json::<ConfigResp>().await {
                    Ok(body) => {
                        text.set(body.config);
                        hydrated.set(true);
                    }
                    Err(e) => {
                        error.set(Some(format!("Could not parse config: {e}")));
                        load_failed.set(true);
                    }
                },
                Ok(r) => {
                    error.set(Some(format!("HTTP {} loading config", r.status())));
                    load_failed.set(true);
                }
                Err(e) => {
                    error.set(Some(format!("Network error: {e}")));
                    load_failed.set(true);
                }
            }
        });
    });

    let mut submit = move |_| {
        if busy() || !hydrated() { return; }
        busy.set(true);
        error.set(None);
        let body = json!({ "config": text() });
        spawn(async move {
            let resp = Request::put("/api/stats/config")
                .header("content-type", "application/json")
                .body(body.to_string());
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    busy.set(false);
                    error.set(Some(format!("Could not build request: {e}")));
                    return;
                }
            };
            match resp.send().await {
                Ok(r) if r.status() == 401 => {
                    busy.set(false);
                    auth_ctx.signal_unauthorized();
                }
                Ok(r) if r.ok() => {
                    busy.set(false);
                    let payload = r.text().await.unwrap_or_default();
                    let summary = match serde_json::from_str::<serde_json::Value>(&payload) {
                        Ok(v) => v.get("output").and_then(|o| o.as_str()).unwrap_or("Saved.").to_string(),
                        Err(_) => "Saved.".into(),
                    };
                    props.on_saved.call(summary);
                }
                Ok(r) => {
                    let status = r.status();
                    let txt = r.text().await.unwrap_or_default();
                    busy.set(false);
                    let msg = serde_json::from_str::<serde_json::Value>(&txt)
                        .ok()
                        .and_then(|v| v.get("error").and_then(|e| e.as_str().map(String::from)))
                        .unwrap_or_else(|| format!("HTTP {status}"));
                    error.set(Some(msg));
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(format!("Network error: {e}")));
                }
            }
        });
    };

    rsx! {
        div { class: "modal-overlay", onclick: move |_| props.on_close.call(()),
            form {
                class: "modal form-modal",
                onclick: move |e| e.stop_propagation(),
                onsubmit: move |e| { e.prevent_default(); submit(()); },

                div { class: "modal-header",
                    h3 { "Stats service config" }
                    button {
                        class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()),
                        Icon { name: "x" }
                    }
                }

                div { class: "modal-body form-modal-body",
                    p { class: "preview-label",
                        "Edit "
                        code { "/etc/bananas/stats.toml" }
                        ". Saving validates the TOML, atomic-replaces the file, and restarts "
                        code { "bananas-stats.service" }
                        "."
                    }

                    if let Some(msg) = error() {
                        div { class: "banner err", pre { "{msg}" } }
                    }

                    if !hydrated() && !load_failed() {
                        p { class: "preview-label", "Loading…" }
                    }

                    textarea {
                        id: "stats-config-editor",
                        style: "min-height: 360px; font: 13px/1.4 ui-monospace, monospace; width: 100%; box-sizing: border-box;",
                        spellcheck: false,
                        value: "{text()}",
                        oninput: move |e| text.set(e.value()),
                        readonly: !hydrated(),
                    }
                }

                div { class: "modal-footer",
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()), "Cancel" }
                    button { class: "primary", r#type: "submit",
                        disabled: busy() || !hydrated(),
                        if busy() { "Saving…" } else { "Save & restart service" }
                    }
                }
            }
        }
    }
}
