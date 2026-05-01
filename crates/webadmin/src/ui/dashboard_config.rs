//! Form-based dashboard config editor — symmetric with stats_config.
//! Loads /etc/bananas/dashboard.toml through /api/dashboard/config,
//! parses the flat schema into typed form fields, lets the operator
//! edit each value, and serializes a clean TOML payload on save.
//! Composed TOML appears below the form in a read-only Copy textarea.
//!
//! No service restart on save — bananas-dashboard polls the file's
//! mtime every 2 s and reapplies theme + refresh-rate changes in
//! place, so the LCD doesn't blink off on every settings tweak.

#![allow(non_snake_case)]

use dioxus::prelude::*;
use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{AuthCtx, components::TextareaWithCopy};

#[derive(Debug, Clone, Deserialize)]
struct ConfigResp {
    #[serde(default)]
    config: String,
}

/// Mirror of `bananas-dashboard`'s `DashboardConfig` — flat fields
/// directly under the document root, no `[ui]` / `[live_socket]`
/// subsections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Cfg {
    width: u32,
    height: u32,
    title: String,
    theme: String,
    spark_window: usize,
    refresh_ms: u64,
    socket: String,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            width: 800,
            height: 480,
            title: "bananas-dashboard".into(),
            theme: "auto".into(),
            spark_window: 60,
            refresh_ms: 2000,
            socket: "/run/bananas/stats.sock".into(),
        }
    }
}

#[component]
pub fn DashboardConfigForm() -> Element {
    let auth_ctx = use_context::<AuthCtx>();

    let mut cfg = use_signal(Cfg::default);
    let mut width = use_signal(|| "800".to_string());
    let mut height = use_signal(|| "480".to_string());
    let mut title = use_signal(|| "bananas-dashboard".to_string());
    let mut theme = use_signal(|| "auto".to_string());
    let mut spark_window = use_signal(|| "60".to_string());
    let mut refresh_ms = use_signal(|| "2000".to_string());

    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut info: Signal<Option<String>> = use_signal(|| None);
    let mut busy = use_signal(|| false);
    let mut hydrated = use_signal(|| false);
    let mut load_failed = use_signal(|| false);

    use_effect(move || {
        if hydrated() {
            return;
        }
        spawn(async move {
            match Request::get("/api/dashboard/config").send().await {
                Ok(r) if r.status() == 401 => auth_ctx.signal_unauthorized(),
                Ok(r) if r.ok() => match r.json::<ConfigResp>().await {
                    Ok(body) => {
                        let parsed: Cfg = toml::from_str(&body.config).unwrap_or_default();
                        width.set(parsed.width.to_string());
                        height.set(parsed.height.to_string());
                        title.set(parsed.title.clone());
                        theme.set(parsed.theme.clone());
                        spark_window.set(parsed.spark_window.to_string());
                        refresh_ms.set(parsed.refresh_ms.to_string());
                        cfg.set(parsed);
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

    let composed_toml = move || -> String {
        let mut new = Cfg::default();
        new.width = width().parse().unwrap_or(800);
        new.height = height().parse().unwrap_or(480);
        new.title = title();
        new.theme = theme();
        new.spark_window = spark_window().parse().unwrap_or(60);
        new.refresh_ms = refresh_ms().parse().unwrap_or(2000);
        new.socket = cfg().socket.clone();
        toml::to_string_pretty(&new).unwrap_or_default()
    };

    let preview = composed_toml();

    let mut submit = move |_| {
        if busy() || !hydrated() {
            return;
        }
        busy.set(true);
        error.set(None);
        info.set(None);
        let body = json!({ "config": composed_toml() });
        spawn(async move {
            let resp = Request::put("/api/dashboard/config")
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
                    info.set(Some(
                        "Saved. The LCD picks the new theme/refresh-rate up within ~2 s — no service restart.".into(),
                    ));
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
        form {
            class: "settings-form",
            onsubmit: move |e| { e.prevent_default(); submit(()); },

            p { class: "preview-label",
                "Edits land in "
                code { "/etc/bananas/dashboard.toml" }
                ". The LCD app polls every 2 s and hot-reloads — no service restart."
            }

            if let Some(msg) = error() {
                div { class: "banner err", pre { "{msg}" } }
            }
            if let Some(msg) = info() {
                div { class: "banner ok", pre { "{msg}" } }
            }

            if !hydrated() && !load_failed() {
                p { class: "preview-label", "Loading…" }
            }

            fieldset {
                legend { "Render target" }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Render width in pixels. Native panel is 800. Change only if you swap the LCD.",
                        "Width (px)" }
                    input { r#type: "number", min: "320", required: true,
                        value: "{width()}",
                        oninput: move |e| width.set(e.value()) }
                    span {}
                }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Render height in pixels. Native panel is 480.",
                        "Height (px)" }
                    input { r#type: "number", min: "240", required: true,
                        value: "{height()}",
                        oninput: move |e| height.set(e.value()) }
                    span {}
                }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Window title (only visible if the dashboard ever runs in a desktop window manager).",
                        "Title" }
                    input { r#type: "text",
                        value: "{title()}",
                        oninput: move |e| title.set(e.value()) }
                    span {}
                }
            }

            fieldset {
                legend { "Appearance" }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "auto = dark in PM hours / light in AM. Or pin to dark/light.",
                        "Theme" }
                    select {
                        value: "{theme()}",
                        onchange: move |e| theme.set(e.value()),
                        option { value: "auto", "auto (dark/light by clock)" }
                        option { value: "dark", "dark" }
                        option { value: "light", "light" }
                    }
                    span {}
                }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Number of historical points kept on the live sparklines.",
                        "Sparkline window" }
                    input { r#type: "number", min: "10", max: "1000", required: true,
                        value: "{spark_window()}",
                        oninput: move |e| spark_window.set(e.value()) }
                    span {}
                }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Min interval between LCD repaints (ms). The sampler still ticks at sampling.interval_ms; this just throttles the GPU work. 2000 ms keeps Mali-400 + lima happy at ~3% CPU.",
                        "Refresh interval (ms)" }
                    input { r#type: "number", min: "250", step: "250", required: true,
                        value: "{refresh_ms()}",
                        oninput: move |e| refresh_ms.set(e.value()) }
                    span {}
                }
            }

            div { class: "settings-actions",
                button { class: "primary", r#type: "submit",
                    disabled: busy() || !hydrated(),
                    if busy() { "Saving…" } else { "Save" }
                }
            }

            h4 { style: "margin: 1.2em 0 .4em",
                "Generated TOML"
            }
            p { class: "preview-label",
                "Read-only — exactly what we'll write to /etc/bananas/dashboard.toml on save. "
                "Copy the block if you'd rather edit it by hand on the device."
            }
            TextareaWithCopy { value: preview, id: "dashboard-config-toml" }
        }
    }
}
