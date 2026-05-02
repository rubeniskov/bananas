//! Settings page — host-level system config only.
//!
//! Two cards:
//!   - Timezone picker (writes to `/etc/bananas/system.toml` via
//!     timedatectl through bananas-engine)
//!   - Read-only system.toml viewer
//!
//! Per-feature configs (stats / dashboard / cloud / future plugins)
//! live inside their respective plugin SPAs — settings is no longer
//! a hub for those. The Tab enum that used to switch between Stats,
//! Dashboard, and General was removed when settings dropped its
//! per-feature responsibilities.

#![allow(non_snake_case)]

use dioxus::prelude::*;
use gloo_net::http::Request;
use serde::Deserialize;
use serde_json::json;

use crate::{AuthCtx, components::Spinner};

#[component]
pub fn SettingsPage() -> Element {
    rsx! {
        div { class: "section-header",
            h2 { "Settings" }
        }
        TimezoneCard {}
        SystemTomlEditor {}
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ConfigResp {
    #[serde(default)]
    config: String,
}

/// Read-only system.toml viewer at the bottom of the General tab. The
/// timezone is set via the focused TimezoneCard above; a raw textarea
/// is here only so power users can inspect / copy the file's full
/// contents (and future fields beyond timezone, when those land).
#[component]
fn SystemTomlEditor() -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let mut content = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let tick = use_signal(|| 0u32);

    use_effect(move || {
        let _ = tick();
        let _ = auth_ctx.refresh.read();
        spawn(async move {
            loading.set(true);
            match Request::get("/api/system/config").send().await {
                Ok(resp) if resp.status() == 401 => auth_ctx.signal_unauthorized(),
                Ok(resp) if resp.ok() => {
                    if let Ok(c) = resp.json::<ConfigResp>().await {
                        content.set(c.config);
                    }
                }
                _ => {}
            }
            loading.set(false);
        });
    });

    rsx! {
        h4 { style: "margin: 1.4em 0 .4em", "system.toml" }
        p { class: "preview-label",
            "Read-only — current contents of /etc/bananas/system.toml. Edits via "
            "Settings → General are written through the focused widgets above."
        }
        if loading() {
            div { class: "settings-loading",
                Spinner { size: 20 }
                span { "Loading…" }
            }
        } else {
            crate::components::TextareaWithCopy {
                value: content(),
                id: "system-config-toml",
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
    let mut zones: Signal<Vec<String>> = use_signal(Vec::new);
    let mut banner: Signal<Option<(BannerKind, String)>> = use_signal(|| None);
    let mut busy = use_signal(|| false);

    // Pull the OS's full IANA zone list once at mount so the datalist
    // autocomplete is comprehensive (~400 entries vs the 14-name
    // hand-curated subset that used to live here).
    use_effect(move || {
        spawn(async move {
            if let Ok(resp) = Request::get("/api/system/timezones").send().await {
                if resp.ok() {
                    if let Ok(list) = resp.json::<Vec<String>>().await {
                        zones.set(list);
                    }
                }
            }
        });
    });

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
                "IANA tzdata zone (e.g. \"Europe/Madrid\", \"America/New_York\"). "
                "Applied via timedatectl and persisted to system.toml."
            }
            if let Some((kind, msg)) = banner() {
                div { class: "banner {kind.css()}", pre { "{msg}" } }
            }
            div { class: "settings-tz-row",
                input {
                    r#type: "text",
                    class: "settings-tz-input",
                    list: "all-tz",
                    placeholder: "Region/City",
                    value: "{tz()}",
                    oninput: move |e| tz.set(e.value()),
                    disabled: busy(),
                }
                datalist { id: "all-tz",
                    for z in zones.read().iter() {
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
