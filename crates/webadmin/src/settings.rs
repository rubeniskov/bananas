//! Settings page — canonical home for all service config files. Three
//! tabs: Stats (sampler), Dashboard (LCD UI), General (timezone, etc.).
//! Each tab renders a TOML editor backed by /api/<name>/config; the
//! General tab also exposes a focused timezone picker that hits
//! /api/system/timezone so the operator doesn't have to round-trip
//! through TOML for the most common change.
//!
//! Power users who want the rich form for stats.toml can still open
//! the gear-icon modal from the Stats page; this page is the
//! plain-and-canonical view.

#![allow(non_snake_case)]

use dioxus::prelude::*;
use gloo_net::http::Request;
use serde::Deserialize;
use serde_json::json;

use crate::{AuthCtx, components::Spinner, icons::Icon};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Stats,
    Dashboard,
    General,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Stats => "Stats",
            Self::Dashboard => "Dashboard",
            Self::General => "General",
        }
    }

    fn config_endpoint(self) -> &'static str {
        match self {
            Self::Stats => "/api/stats/config",
            Self::Dashboard => "/api/dashboard/config",
            Self::General => "/api/system/config",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Stats => "Sampler-side config. Restarts bananas-stats on save.",
            Self::Dashboard => {
                "LCD dashboard appearance + refresh cadence. Hot-reloads — no service restart."
            }
            Self::General => {
                "Host-level settings (timezone). The General tab below has a focused picker for timezone."
            }
        }
    }
}

#[component]
pub fn SettingsPage() -> Element {
    let mut tab = use_signal(|| Tab::Stats);
    rsx! {
        div { class: "section-header",
            h2 { "Settings" }
        }
        div { class: "settings-tabs",
            for t in [Tab::Stats, Tab::Dashboard, Tab::General] {
                {
                    let active = tab() == t;
                    let cls = if active { "settings-tab active" } else { "settings-tab" };
                    rsx! {
                        button {
                            r#type: "button",
                            class: "{cls}",
                            onclick: move |_| tab.set(t),
                            "{t.label()}"
                        }
                    }
                }
            }
        }
        match tab() {
            Tab::Stats => rsx! { ConfigEditor { tab: Tab::Stats } },
            Tab::Dashboard => rsx! { ConfigEditor { tab: Tab::Dashboard } },
            Tab::General => rsx! {
                TimezoneCard {}
                ConfigEditor { tab: Tab::General }
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ConfigResp {
    #[serde(default)]
    config: String,
}

#[derive(Props, Clone, PartialEq)]
struct ConfigEditorProps {
    tab: Tab,
}

/// Generic raw-TOML editor backed by /api/<name>/config. Three-state
/// banner: idle / saving / ok / err. Reuses the same shape as the
/// existing stats_config modal but without the parsed form fields —
/// the canonical source of truth on disk is the TOML, so we let the
/// operator edit it directly.
#[component]
fn ConfigEditor(props: ConfigEditorProps) -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let endpoint: &'static str = props.tab.config_endpoint();
    let mut content = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut busy = use_signal(|| false);
    let mut banner: Signal<Option<(BannerKind, String)>> = use_signal(|| None);
    let mut tick = use_signal(|| 0u32);

    use_effect(move || {
        let _ = tick();
        let _ = auth_ctx.refresh.read();
        let endpoint = endpoint;
        spawn(async move {
            loading.set(true);
            match Request::get(endpoint).send().await {
                Ok(resp) if resp.status() == 401 => auth_ctx.signal_unauthorized(),
                Ok(resp) if resp.ok() => match resp.json::<ConfigResp>().await {
                    Ok(c) => content.set(c.config),
                    Err(e) => banner.set(Some((BannerKind::Err, format!("Parse failed: {e}")))),
                },
                Ok(resp) => banner.set(Some((
                    BannerKind::Err,
                    format!("HTTP {} loading config", resp.status()),
                ))),
                Err(e) => banner.set(Some((BannerKind::Err, format!("Network: {e}")))),
            }
            loading.set(false);
        });
    });

    let on_save = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        let body = content();
        spawn(async move {
            let resp = Request::put(endpoint).json(&json!({ "config": body }));
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    busy.set(false);
                    banner.set(Some((BannerKind::Err, e.to_string())));
                    return;
                }
            };
            match resp.send().await {
                Ok(r) if r.status() == 401 => auth_ctx.signal_unauthorized(),
                Ok(r) if r.ok() => {
                    banner.set(Some((BannerKind::Ok, "Saved.".into())));
                    tick.set(tick() + 1);
                }
                Ok(r) => {
                    let status = r.status();
                    let txt = r.text().await.unwrap_or_default();
                    banner.set(Some((BannerKind::Err, format!("HTTP {status}: {txt}"))));
                }
                Err(e) => {
                    banner.set(Some((BannerKind::Err, format!("Network: {e}"))));
                }
            }
            busy.set(false);
        });
    };

    rsx! {
        p { class: "preview-label", "{props.tab.description()}" }
        if let Some((kind, msg)) = banner() {
            div { class: "banner {kind.css()}", pre { "{msg}" } }
        }
        if loading() {
            div { class: "settings-loading",
                Spinner { size: 24 }
                span { "Loading…" }
            }
        } else {
            textarea {
                class: "settings-toml",
                rows: "20",
                spellcheck: false,
                disabled: busy(),
                value: "{content()}",
                oninput: move |e| content.set(e.value()),
            }
            div { class: "settings-actions",
                button {
                    class: "ghost",
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| tick.set(tick() + 1),
                    Icon { name: "rotate-cw" }
                    "Reload"
                }
                button {
                    class: "primary",
                    r#type: "button",
                    disabled: busy(),
                    onclick: on_save,
                    Icon { name: "check" }
                    if busy() { "Saving…" } else { "Save" }
                }
            }
        }
    }
}

/// Focused timezone picker. Browser hands us its IANA tz; we POST it
/// to /api/system/timezone which both calls timedatectl and persists
/// to system.toml. Saves the operator from typing IANA names by hand.
#[component]
fn TimezoneCard() -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let mut tz = use_signal(|| {
        // Default the input to the browser's detected zone — easier
        // than typing "Europe/Madrid" by hand on a phone.
        js_sys::Reflect::get(&js_sys::global(), &"Intl".into())
            .ok()
            .and_then(|intl| {
                let opts = js_sys::Reflect::get(&intl, &"DateTimeFormat".into()).ok()?;
                let ctor = opts.dyn_ref::<js_sys::Function>()?;
                let dtf = ctor.call0(&wasm_bindgen::JsValue::UNDEFINED).ok()?;
                let resolved = js_sys::Reflect::get(&dtf, &"resolvedOptions".into()).ok()?;
                let resolved_fn = resolved.dyn_ref::<js_sys::Function>()?;
                let opts_obj = resolved_fn.call0(&dtf).ok()?;
                let zone = js_sys::Reflect::get(&opts_obj, &"timeZone".into()).ok()?;
                zone.as_string()
            })
            .unwrap_or_else(|| "UTC".to_string())
    });
    let mut banner: Signal<Option<(BannerKind, String)>> = use_signal(|| None);
    let mut busy = use_signal(|| false);

    let apply = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        let target = tz();
        spawn(async move {
            let resp = Request::post("/api/system/timezone").json(&json!({ "tz": target.clone() }));
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    busy.set(false);
                    banner.set(Some((BannerKind::Err, e.to_string())));
                    return;
                }
            };
            match resp.send().await {
                Ok(r) if r.status() == 401 => auth_ctx.signal_unauthorized(),
                Ok(r) if r.ok() => {
                    banner.set(Some((BannerKind::Ok, format!("Timezone set to {target}."))));
                }
                Ok(r) => {
                    let status = r.status();
                    let txt = r.text().await.unwrap_or_default();
                    let err = parse_api_err(&txt).unwrap_or(txt);
                    banner.set(Some((BannerKind::Err, format!("HTTP {status}: {err}"))));
                }
                Err(e) => {
                    banner.set(Some((BannerKind::Err, format!("Network: {e}"))));
                }
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "settings-card",
            h3 { "Timezone" }
            p { class: "preview-label",
                "IANA tzdata zone (e.g. \"Europe/Madrid\", \"America/New_York\"). Applied via timedatectl and persisted to system.toml."
            }
            if let Some((kind, msg)) = banner() {
                div { class: "banner {kind.css()}", pre { "{msg}" } }
            }
            div { class: "settings-tz-row",
                input {
                    r#type: "text",
                    class: "settings-tz-input",
                    list: "common-tz",
                    placeholder: "Region/City",
                    value: "{tz()}",
                    oninput: move |e| tz.set(e.value()),
                    disabled: busy(),
                }
                datalist { id: "common-tz",
                    for z in COMMON_TZ.iter() {
                        option { value: "{z}" }
                    }
                }
                button {
                    class: "primary",
                    r#type: "button",
                    disabled: busy(),
                    onclick: apply,
                    if busy() { "Applying…" } else { "Apply" }
                }
            }
        }
    }
}

/// A short list of the most common IANA zones, surfaced as a datalist
/// so the operator gets autocomplete in the input. Not exhaustive —
/// anything under /usr/share/zoneinfo is accepted by the helper.
const COMMON_TZ: &[&str] = &[
    "UTC",
    "Europe/Madrid",
    "Europe/Berlin",
    "Europe/London",
    "Europe/Paris",
    "America/New_York",
    "America/Los_Angeles",
    "America/Chicago",
    "America/Toronto",
    "America/Sao_Paulo",
    "Asia/Tokyo",
    "Asia/Shanghai",
    "Asia/Singapore",
    "Australia/Sydney",
];

#[derive(Clone, Copy, PartialEq)]
enum BannerKind {
    Ok,
    Err,
}
impl BannerKind {
    fn css(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Err => "err",
        }
    }
}

fn parse_api_err(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("error")?.as_str().map(|s| s.to_string())
}

// Wasm-bindgen utilities used by TimezoneCard for browser tz detection.
use wasm_bindgen::JsCast;
