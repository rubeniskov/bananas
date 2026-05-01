//! Shared form-state hooks: every macro-generated `view()` calls
//! `use_form_state::<C>()` to get a single bundle of the
//! `busy/hydrated/error/info` signals plus a pre-wired
//! `submit(payload_toml)` closure. Keeps the macro emit small —
//! only per-field row markup ends up token-stream'd.

use dioxus::prelude::*;
use gloo_net::http::Request;

use crate::{ConfigForm, ConfigResp};

/// Bundle of signals the macro-emitted view binds to. The submit
/// closure is plain `Callback<String>` so the form's `onsubmit`
/// handler just calls `submit.call(composed_toml())`.
#[derive(Clone, Copy)]
pub struct FormState {
    pub error: Signal<Option<String>>,
    pub info: Signal<Option<String>>,
    pub busy: Signal<bool>,
    pub hydrated: Signal<bool>,
    pub load_failed: Signal<bool>,
}

/// Allocate the form-state signals + spawn the initial GET. The
/// `on_loaded` callback receives the parsed TOML body so the
/// macro-emitted view can `set()` every field signal in one place.
/// `on_unauthorized` fires on 401 — the plugin's wrapper passes
/// `auth_ctx.signal_unauthorized()` here.
pub fn use_form_state<C: ConfigForm + 'static>(
    on_loaded: impl FnMut(String) + 'static + Copy,
    on_unauthorized: impl Fn() + 'static + Copy,
) -> FormState {
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut info: Signal<Option<String>> = use_signal(|| None);
    let busy = use_signal(|| false);
    let mut hydrated = use_signal(|| false);
    let mut load_failed = use_signal(|| false);

    {
        let mut on_loaded = on_loaded;
        use_effect(move || {
            if hydrated() {
                return;
            }
            spawn(async move {
                match Request::get(C::ENDPOINT).send().await {
                    Ok(r) if r.status() == 401 => on_unauthorized(),
                    Ok(r) if r.ok() => match r.json::<ConfigResp>().await {
                        Ok(body) => {
                            on_loaded(body.config);
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
                let _ = info; // silence unused-write warning when no save fires
            });
        });
    }
    FormState {
        error,
        info,
        busy,
        hydrated,
        load_failed,
    }
}

/// PUT the composed TOML to `C::ENDPOINT`. Mirrors the submit
/// closure each hand-written form had — busy guard, banner state,
/// 401 handler. The macro-emitted form calls this directly from its
/// `<form onsubmit>` handler.
pub async fn submit_config<C: ConfigForm>(
    composed_toml: String,
    mut state: FormState,
    on_unauthorized: impl Fn() + 'static,
) {
    if state.busy.peek().clone() || !state.hydrated.peek().clone() {
        return;
    }
    state.busy.set(true);
    state.error.set(None);
    state.info.set(None);

    let body = serde_json::json!({ "config": composed_toml }).to_string();
    let resp = match Request::put(C::ENDPOINT)
        .header("content-type", "application/json")
        .body(body)
    {
        Ok(r) => r,
        Err(e) => {
            state.busy.set(false);
            state
                .error
                .set(Some(format!("Could not build request: {e}")));
            return;
        }
    };
    match resp.send().await {
        Ok(r) if r.status() == 401 => {
            state.busy.set(false);
            on_unauthorized();
        }
        Ok(r) if r.ok() => {
            state.busy.set(false);
            state.info.set(Some(C::SUCCESS_MSG.to_string()));
        }
        Ok(r) => {
            let status = r.status();
            let txt = r.text().await.unwrap_or_default();
            state.busy.set(false);
            let msg = serde_json::from_str::<serde_json::Value>(&txt)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str().map(String::from)))
                .unwrap_or_else(|| format!("HTTP {status}"));
            state.error.set(Some(msg));
        }
        Err(e) => {
            state.busy.set(false);
            state.error.set(Some(format!("Network error: {e}")));
        }
    }
}
