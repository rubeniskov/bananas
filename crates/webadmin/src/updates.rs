//! Updates page — per-component status cards + install flow.
//!
//! Layout:
//!   - Header: latest tag (links to GitHub release notes) + Refresh
//!     button so the operator can force a re-check without waiting
//!     for the 5-minute server cache.
//!   - One card per component (server / helper / stats / dashboard /
//!     webadmin). Each shows installed-vs-latest and an Install button
//!     when an update is available. Server + Helper are "Coming soon"
//!     in v1 since their self-update primitive lands separately.
//!
//! The install button opens a modal that subscribes to /api/updates/status
//! via EventSource and streams the SSE log lines until the helper's
//! Done/Error event closes the connection.

#![allow(non_snake_case)]

use dioxus::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{EventSource, MessageEvent};

use crate::{
    AuthCtx, BannerKind,
    api::{self, ComponentStatus, InstallRequest, UpdatesCheck},
    components::Spinner,
};

/// Order shown on the page. Mirrors crate::version::ALL on the server.
const COMPONENTS: &[(&str, &str)] = &[
    ("server", "bananas-server"),
    ("helper", "bananas-helper"),
    ("stats", "bananas-stats"),
    ("dashboard", "bananas-dashboard"),
    ("webadmin", "bananas-webadmin"),
];

#[component]
pub fn UpdatesPage() -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let mut data: Signal<Option<UpdatesCheck>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut banner: Signal<Option<(BannerKind, String)>> = use_signal(|| None);
    let mut install_modal: Signal<Option<(String, String)>> = use_signal(|| None);

    let mut load = move || {
        loading.set(true);
        spawn(async move {
            match api::fetch_updates_check().await {
                Ok(d) => {
                    data.set(Some(d));
                    banner.set(None);
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
                    "data-tip": "Re-check GitHub now",
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
            if let Some(err) = d.error.as_ref() {
                div { class: "banner err",
                    pre { "GitHub fetch failed: {err}" }
                }
            } else if let (Some(latest), Some(url)) = (d.latest_version.as_ref(), d.release_url.as_ref()) {
                div { class: "updates-header",
                    span { "Latest release: " }
                    a { href: "{url}", target: "_blank", rel: "noopener noreferrer",
                        strong { "v{latest}" }
                    }
                }
            }

            div { class: "updates-grid",
                for &(slug, label) in COMPONENTS {
                    {
                        let component = slug.to_string();
                        let display_name = label.to_string();
                        let status = d.components.get(slug).cloned();
                        let install_for = component.clone();
                        let install_version = status
                            .as_ref()
                            .and_then(|s| s.latest.clone())
                            .unwrap_or_default();
                        rsx! {
                            ComponentCard {
                                key: "{slug}",
                                slug: component,
                                display_name,
                                status,
                                on_install: move |_| {
                                    install_modal.set(Some((install_for.clone(), install_version.clone())));
                                },
                            }
                        }
                    }
                }
            }
        }

        if let Some((component, version)) = install_modal() {
            InstallModal {
                component: component.clone(),
                version: version.clone(),
                on_close: move |_| {
                    install_modal.set(None);
                    load();
                },
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ComponentCardProps {
    slug: String,
    display_name: String,
    status: Option<ComponentStatus>,
    on_install: EventHandler<()>,
}

#[component]
fn ComponentCard(props: ComponentCardProps) -> Element {
    let (installed, latest, outdated, can_install) = match props.status.as_ref() {
        Some(s) => (
            s.installed.clone().unwrap_or_else(|| "—".into()),
            s.latest.clone().unwrap_or_else(|| "?".into()),
            s.outdated,
            s.outdated && s.asset_url.is_some() && s.sha256.is_some(),
        ),
        None => ("—".into(), "?".into(), false, false),
    };

    // Server + Helper are deferred to step 7+8 — show a "coming soon"
    // affordance instead of a live install button.
    let deferred = matches!(props.slug.as_str(), "server" | "helper");
    let badge = if deferred {
        ("Self-update coming soon", "deferred")
    } else if outdated {
        ("Update available", "outdated")
    } else {
        ("Up to date", "current")
    };

    rsx! {
        div { class: "update-card",
            div { class: "update-card-head",
                strong { "{props.display_name}" }
                span { class: "update-badge {badge.1}", "{badge.0}" }
            }
            div { class: "update-card-body",
                div { class: "update-row",
                    span { class: "update-label", "Installed" }
                    span { class: "update-version", "{installed}" }
                }
                div { class: "update-row",
                    span { class: "update-label", "Latest" }
                    span { class: "update-version", "{latest}" }
                }
            }
            div { class: "update-card-foot",
                if deferred {
                    span { class: "update-foot-note",
                        "Self-update arrives with bananas-server v1.x"
                    }
                } else if can_install {
                    button {
                        class: "primary",
                        r#type: "button",
                        onclick: move |_| props.on_install.call(()),
                        crate::icons::Icon { name: "download" }
                        span { "Install" }
                    }
                } else {
                    span { class: "update-foot-note", "Nothing to install" }
                }
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
    component: String,
    version: String,
    on_close: EventHandler<()>,
}

#[component]
fn InstallModal(props: InstallModalProps) -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let log: Signal<Vec<LogLine>> = use_signal(Vec::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut finished: Signal<Option<bool>> = use_signal(|| None);
    let mut started: Signal<bool> = use_signal(|| false);

    let component = props.component.clone();
    let version = props.version.clone();

    use_effect(move || {
        if started() {
            return;
        }
        started.set(true);
        let component = component.clone();
        let version = version.clone();
        spawn(async move {
            let req = InstallRequest {
                component: component.clone(),
                version: version.clone(),
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

    let phase_class = |p: &str| match p {
        "download" => "phase phase-download",
        "verify" => "phase phase-verify",
        "install" => "phase phase-install",
        "done" => "phase phase-done",
        "error" => "phase phase-error",
        _ => "phase",
    };

    rsx! {
        div { class: "modal-overlay",
            div { class: "modal install-modal",
                div { class: "modal-header",
                    strong { "Installing {props.component} {props.version}" }
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
                                span { class: "{phase_class(&line.phase)}", "{line.phase}" }
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
                                span { "Install complete." }
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
                                span { "Install failed — check the log above." }
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
/// route incoming events into the log signals. The ES is leaked
/// intentionally — we close it via the JS-side close event handler
/// when the helper sends a final phase=done|error.
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
        // The server emits a custom `event: close` with `data: ok|error`
        // when the install settles; we treat any of these as the
        // explicit close signal.
        es_clone_close.close();
    });
    es.add_event_listener_with_callback("close", on_close.as_ref().unchecked_ref())
        .ok();
    on_close.forget();

    let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_evt: web_sys::Event| {
        // Don't surface a banner-level error here — EventSource can fire
        // `error` on a clean close (last event followed by EOF), and we
        // already track terminal state via the message handler. Just
        // ensure we eventually flip `finished` if the network drops mid
        // install without a final phase event.
        if finished.peek().is_none() {
            // Wait one tick before deciding — message handler may still
            // run after the error event in the same callback batch. To
            // keep it simple, just mark errored.
            error.set(Some("connection lost".into()));
            finished.set(Some(false));
        }
    });
    es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();
}
