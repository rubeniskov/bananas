//! Directory-picker modal. Loads /api/browse on each navigation and lets
//! the user click into directories until they pick one. Files are shown
//! disabled (the picker is a directory chooser only).

#![allow(non_snake_case)]

use dioxus::prelude::*;

use crate::{api, icons::Icon};

#[derive(Props, Clone, PartialEq)]
pub struct BrowserProps {
    pub start: String,
    pub on_pick: EventHandler<String>,
    pub on_close: EventHandler<()>,
}

/// Where the picker drops the user when no starting path was provided
/// (or the caller passed an empty string, which the server's canonicalize
/// treats as "no path" and rejects with 400).
const DEFAULT_START: &str = "/srv";

#[component]
pub fn Browser(props: BrowserProps) -> Element {
    let initial = if props.start.is_empty() || !props.start.starts_with('/') {
        DEFAULT_START.to_string()
    } else {
        props.start.clone()
    };
    let mut current = use_signal(|| initial);
    let listing = use_resource({
        let current = current.clone();
        move || {
            let path = current().clone();
            async move { api::browse(&path).await }
        }
    });

    rsx! {
        div { class: "modal-overlay", onclick: move |_| props.on_close.call(()),
            div {
                class: "modal",
                // Stop click bubbling so clicking inside the modal doesn't close it.
                onclick: move |e| e.stop_propagation(),

                div { class: "modal-header",
                    h3 { "Pick a directory" }
                    button {
                        class: "ghost",
                        onclick: move |_| props.on_close.call(()),
                        "✕"
                    }
                }

                match &*listing.read_unchecked() {
                    None => rsx! {
                        div { class: "modal-body",
                            p { style: "padding: 1em; color: #888", "Loading…" }
                        }
                    },
                    Some(Err(err)) => rsx! {
                        div { class: "breadcrumb",
                            BreadcrumbLink { name: "/".to_string(), path: "/".to_string(), on_nav: move |p: String| current.set(p) }
                        }
                        div { class: "modal-body",
                            div { class: "banner err", pre { "{err}" } }
                            // 401 here means the session lapsed while the modal was
                            // open; close so the underlying app can flip to login.
                            { if matches!(err, api::ApiError::Unauthorized) {
                                rsx! { p { class: "preview-label", style: "padding: 1em",
                                    button { class: "primary",
                                        onclick: move |_| props.on_close.call(()),
                                        "Sign in again" }
                                } }
                            } else { rsx! {} } }
                            // Manual fallback so the user can still navigate when canonicalize fails.
                            p { style: "padding: 0 1em",
                                "Try a different path: "
                                input {
                                    r#type: "text",
                                    style: "width: 60%; font-family: ui-monospace, monospace; padding: 4px 6px",
                                    value: "{current()}",
                                    onchange: move |e| current.set(e.value())
                                }
                            }
                        }
                    },
                    Some(Ok(l)) => rsx! {
                        div { class: "breadcrumb",
                            for (i, c) in l.breadcrumb.iter().enumerate() {
                                // Skip the separator before crumb #1 — the root
                                // crumb's name is already "/", so adding a
                                // separator after it produced "//srv".
                                if i > 1 {
                                    span { class: "sep", "/" }
                                }
                                BreadcrumbLink {
                                    name: c.name.clone(),
                                    path: c.path.clone(),
                                    on_nav: move |p: String| current.set(p)
                                }
                            }
                        }
                        div { class: "modal-body",
                            ul { class: "entries",
                                if let Some(parent) = &l.parent {
                                    {
                                        let parent = parent.clone();
                                        rsx! {
                                            li {
                                                onclick: move |_| current.set(parent.clone()),
                                                span { class: "icon", "↰" }
                                                span { class: "name", ".." }
                                            }
                                        }
                                    }
                                }
                                for entry in l.entries.iter() {
                                    EntryRow {
                                        key: "{entry.path}",
                                        entry: entry.clone(),
                                        on_open: {
                                            let path = entry.path.clone();
                                            let is_dir = entry.is_dir;
                                            move |_| if is_dir { current.set(path.clone()) }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "modal-footer",
                            span { style: "flex: 1; font: 12px ui-monospace, monospace; color: #666; align-self: center",
                                "{l.path}"
                            }
                            button {
                                onclick: move |_| props.on_close.call(()),
                                "Cancel"
                            }
                            button {
                                class: "primary",
                                onclick: {
                                    let path = l.path.clone();
                                    let on_pick = props.on_pick.clone();
                                    move |_| on_pick.call(path.clone())
                                },
                                "Pick this directory"
                            }
                        }
                    },
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct BreadcrumbLinkProps {
    name: String,
    path: String,
    on_nav: EventHandler<String>,
}

#[component]
fn BreadcrumbLink(props: BreadcrumbLinkProps) -> Element {
    let path = props.path.clone();
    rsx! {
        a {
            href: "javascript:void(0)",
            onclick: move |_| props.on_nav.call(path.clone()),
            "{props.name}"
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct EntryRowProps {
    entry: api::Entry,
    on_open: EventHandler<()>,
}

#[component]
fn EntryRow(props: EntryRowProps) -> Element {
    let class = if props.entry.is_dir { "" } else { "file" };
    let icon = if props.entry.is_dir { "📁" } else { "·" };
    rsx! {
        li {
            class: "{class}",
            onclick: move |_| if props.entry.is_dir { props.on_open.call(()) },
            span { class: "icon", "{icon}" }
            span { class: "name", "{props.entry.name}" }
        }
    }
}
