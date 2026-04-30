//! Updates page — opkg-backed table of upgradable bananas-* packages.
//!
//! Header: "Refresh" button (re-runs `opkg update` on the device).
//! Body: a table of packages where the candidate version is newer
//! than installed. Each row carries an "Upgrade" button; an
//! "Upgrade all" button up top runs them in a single transaction.
//! Install drawer streams the SSE log line-by-line until the helper
//! signals done/error.
//!
//! When everything's up to date, the body shows a single "All
//! packages up to date" line — the SPA never renders the GitHub /
//! sha256 / asset_url chrome the previous custom flow had.

#![allow(non_snake_case)]

use dioxus::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{EventSource, MessageEvent};

use crate::{
    AuthCtx, BannerKind,
    api::{self, UpdatesCheck, UpgradablePackage, UpgradeRequest},
    components::Spinner,
};

#[component]
pub fn UpdatesPage() -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let mut data: Signal<Option<UpdatesCheck>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut banner: Signal<Option<(BannerKind, String)>> = use_signal(|| None);
    let mut install_modal: Signal<Option<Vec<String>>> = use_signal(|| None);

    let mut load = move || {
        loading.set(true);
        spawn(async move {
            match api::fetch_updates_check().await {
                Ok(d) => {
                    if let Some(err) = d.error.as_ref() {
                        banner.set(Some((BannerKind::Err, format!("Feed warning: {err}"))));
                    } else {
                        banner.set(None);
                    }
                    data.set(Some(d));
                }
                Err(api::ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => {
                    banner.set(Some((BannerKind::Err, format!("Check failed: {e}"))));
                }
            }
            loading.set(false);
        });
    };

    use_effect(move || {
        let _ = auth_ctx.refresh.read();
        load();
    });

    rsx! {
        div { class: "section-header",
            h2 { "Updates" }
            div { class: "section-actions",
                button {
                    class: "ghost",
                    r#type: "button",
                    "data-tip": "Re-run opkg update + opkg list-upgradable",
                    disabled: loading(),
                    onclick: move |_| load(),
                    crate::icons::Icon { name: "rotate-cw" }
                    span { "Refresh" }
                }
            }
        }

        if let Some((kind, msg)) = banner() {
            div { class: "banner {kind.css()}",
                pre { "{msg}" }
                button { class: "ghost",
                    onclick: move |_| banner.set(None),
                    "✕"
                }
            }
        }

        if loading() && data().is_none() {
            div { class: "settings-loading",
                Spinner { size: 20 }
                span { "Checking for updates…" }
            }
        }

        if let Some(d) = data() {
            if d.packages.is_empty() {
                div { class: "updates-empty",
                    crate::icons::Icon { name: "circle-check" }
                    span { "All packages up to date." }
                }
            } else {
                div { class: "updates-toolbar",
                    span { class: "updates-count",
                        "{d.packages.len()} package",
                        if d.packages.len() == 1 { "" } else { "s" },
                        " upgradable"
                    }
                    {
                        let all_packages: Vec<String> = d.packages.iter().map(|p| p.name.clone()).collect();
                        rsx! {
                            button {
                                class: "primary",
                                r#type: "button",
                                onclick: move |_| install_modal.set(Some(all_packages.clone())),
                                crate::icons::Icon { name: "download" }
                                span { "Upgrade all" }
                            }
                        }
                    }
                }

                table { class: "upgradable-table",
                    thead {
                        tr {
                            th { "Package" }
                            th { "Installed" }
                            th { "Available" }
                            th { class: "actions" }
                        }
                    }
                    tbody {
                        for pkg in d.packages.iter() {
                            {
                                let pkg_owned: UpgradablePackage = pkg.clone();
                                let pkg_name = pkg.name.clone();
                                rsx! {
                                    tr { key: "{pkg_owned.name}",
                                        td { class: "pkg-name", "{pkg_owned.name}" }
                                        td { class: "pkg-version", "{pkg_owned.installed}" }
                                        td { class: "pkg-version pkg-candidate", "{pkg_owned.candidate}" }
                                        td { class: "actions",
                                            button {
                                                class: "ghost",
                                                r#type: "button",
                                                onclick: move |_| install_modal.set(Some(vec![pkg_name.clone()])),
                                                "Upgrade"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(packages) = install_modal() {
            InstallModal {
                packages: packages.clone(),
                on_close: move |_| {
                    install_modal.set(None);
                    load();
                },
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LogLine {
    #[serde(default)]
    pub seq: u64,
    pub phase: String,
    pub text: String,
}

#[derive(Props, Clone, PartialEq)]
struct InstallModalProps {
    packages: Vec<String>,
    on_close: EventHandler<()>,
}

#[component]
fn InstallModal(props: InstallModalProps) -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let log: Signal<Vec<LogLine>> = use_signal(Vec::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut finished: Signal<Option<bool>> = use_signal(|| None);
    let mut started: Signal<bool> = use_signal(|| false);

    let packages = props.packages.clone();

    use_effect(move || {
        if started() {
            return;
        }
        started.set(true);
        let packages = packages.clone();
        spawn(async move {
            let req = UpgradeRequest {
                packages: packages.clone(),
            };
            match api::post_install(&req).await {
                Ok(_) => {
                    open_event_source(log, finished, error);
                }
                Err(api::ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => {
                    error.set(Some(format!("Install failed to start: {e}")));
                    finished.set(Some(false));
                }
            }
        });
    });

    rsx! {
        div { class: "modal-overlay",
            div { class: "modal install-modal",
                div { class: "modal-header",
                    strong { "Upgrading…" }
                    button {
                        class: "ghost",
                        r#type: "button",
                        disabled: finished().is_none(),
                        onclick: move |_| props.on_close.call(()),
                        "✕"
                    }
                }
                div { class: "install-log",
                    if log.read().is_empty() && finished().is_none() {
                        div { class: "install-log-empty",
                            Spinner { size: 16 }
                            span { "Starting…" }
                        }
                    } else {
                        for line in log.read().iter() {
                            div { key: "{line.seq}", class: "install-log-line",
                                pre { "{line.text}" }
                            }
                        }
                    }
                    if let Some(err) = error() {
                        div { class: "install-log-line error",
                            pre { "{err}" }
                        }
                    }
                }
                div { class: "modal-footer",
                    match finished() {
                        Some(true) => rsx! {
                            span { class: "ok-text",
                                crate::icons::Icon { name: "circle-check" }
                                span { "Upgrade complete." }
                            }
                            button {
                                class: "primary",
                                r#type: "button",
                                onclick: move |_| props.on_close.call(()),
                                "Close"
                            }
                        },
                        Some(false) => rsx! {
                            span { class: "err-text",
                                crate::icons::Icon { name: "triangle-alert" }
                                span { "Upgrade failed — check the log above." }
                            }
                            button {
                                class: "primary",
                                r#type: "button",
                                onclick: move |_| props.on_close.call(()),
                                "Close"
                            }
                        },
                        None => rsx! {
                            span { class: "running-text",
                                Spinner { size: 14 }
                                span { "Running…" }
                            }
                        },
                    }
                }
            }
        }
    }
}

/// Open an EventSource subscription against /api/updates/status and
/// route incoming events into the log signals. The browser handles
/// reconnect automatically (~3 s default backoff); the server's SSE
/// handler always re-streams the full log from byte offset 0 on
/// reconnect, so we clear the log on each `open` to avoid duplicates
/// after a server restart mid-upgrade. Only transitions to a terminal
/// state (`finished = Some(_)`) when the helper signals done/error
/// OR the EventSource transitions to CLOSED (permanent failure).
fn open_event_source(
    mut log: Signal<Vec<LogLine>>,
    mut finished: Signal<Option<bool>>,
    mut error: Signal<Option<String>>,
) {
    let es = match EventSource::new("/api/updates/status") {
        Ok(es) => es,
        Err(e) => {
            error.set(Some(format!("EventSource open failed: {e:?}")));
            finished.set(Some(false));
            return;
        }
    };

    // `open` fires on initial connect AND on every successful
    // auto-reconnect. Clearing the log here keeps the modal in sync
    // with the server's resumed stream (which starts again at
    // since=0). Also clears any "reconnecting…" banner.
    let mut log_open = log;
    let mut error_open = error;
    let on_open = Closure::<dyn FnMut(web_sys::Event)>::new(move |_evt: web_sys::Event| {
        log_open.set(Vec::new());
        if error_open.peek().is_some() {
            error_open.set(None);
        }
    });
    es.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    on_open.forget();

    let es_clone_msg = es.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |evt: MessageEvent| {
        let data = evt.data().as_string().unwrap_or_default();
        match serde_json::from_str::<LogLine>(&data) {
            Ok(line) => {
                let is_final = matches!(line.phase.as_str(), "done" | "error");
                let was_done = line.phase == "done";
                log.with_mut(|v| v.push(line));
                if is_final {
                    finished.set(Some(was_done));
                    es_clone_msg.close();
                }
            }
            Err(e) => {
                tracing::warn!(?data, %e, "bad SSE line");
            }
        }
    });
    es.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    let es_clone_close = es.clone();
    let on_close = Closure::<dyn FnMut(MessageEvent)>::new(move |_evt: MessageEvent| {
        es_clone_close.close();
    });
    es.add_event_listener_with_callback("close", on_close.as_ref().unchecked_ref())
        .ok();
    on_close.forget();

    // EventSource readyState constants: 0=CONNECTING, 1=OPEN, 2=CLOSED.
    // Browsers fire onerror for every transport hiccup and immediately
    // start auto-reconnecting (state goes back to CONNECTING). We only
    // surface a hard failure once the browser itself has given up
    // (state=CLOSED) — typically only on a 4xx response or a closed
    // server socket the browser refuses to retry.
    let es_state = es.clone();
    let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_evt: web_sys::Event| {
        if finished.peek().is_some() {
            return;
        }
        if es_state.ready_state() == EventSource::CLOSED {
            error.set(Some("connection lost".into()));
            finished.set(Some(false));
        } else {
            // Browser is reconnecting. Show a soft notice so the user
            // knows the upgrade is still in flight; cleared by `open`.
            error.set(Some("reconnecting…".into()));
        }
    });
    es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();
}
