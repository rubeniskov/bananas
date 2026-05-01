//! Small shared components used across pages.

#![allow(non_snake_case)]

use dioxus::prelude::*;

use crate::icons::Icon;

#[derive(Props, Clone, PartialEq)]
pub struct TextareaWithCopyProps {
    pub value: String,
    /// Stable DOM id for the inner textarea; required so the Copy
    /// button can locate it via document.getElementById without us
    /// needing to wire a Dioxus ref.
    pub id: &'static str,
}

/// Read-only preview textarea + a small "Copy" button overlay.
/// Used by the Exports and Mounts pages.
#[component]
pub fn TextareaWithCopy(props: TextareaWithCopyProps) -> Element {
    let mut copied = use_signal(|| false);
    let id_for_btn = props.id;
    let value_for_copy = props.value.clone();

    let do_copy = move |_| {
        let value = value_for_copy.clone();
        spawn(async move {
            match write_clipboard(&value).await {
                Ok(()) => {
                    copied.set(true);
                    gloo_timers::future::TimeoutFuture::new(1500).await;
                    copied.set(false);
                }
                Err(_) => {
                    // Fall back: focus + select the textarea so the user
                    // can hit ⌘/Ctrl-C themselves.
                    if let Some(window) = web_sys::window() {
                        if let Some(document) = window.document() {
                            if let Some(el) = document.get_element_by_id(id_for_btn) {
                                use wasm_bindgen::JsCast;
                                if let Ok(ta) = el.dyn_into::<web_sys::HtmlTextAreaElement>() {
                                    ta.focus().ok();
                                    ta.select();
                                }
                            }
                        }
                    }
                }
            }
        });
    };

    let copy_icon: &'static str = if copied() { "check" } else { "copy" };
    rsx! {
        div { class: "textarea-wrap",
            textarea {
                id: "{props.id}",
                readonly: true,
                "{props.value}"
            }
            button {
                class: "ghost textarea-copy",
                r#type: "button",
                "data-tip": "Copy to clipboard",
                onclick: do_copy,
                Icon { name: copy_icon }
                if copied() { "Copied" } else { "Copy" }
            }
        }
    }
}

/// Promise wrapper for `navigator.clipboard.writeText`. Returns Err on
/// browsers without the Clipboard API or when the page isn't HTTPS / a
/// trusted origin (some browsers block it on bare http://).
async fn write_clipboard(text: &str) -> Result<(), String> {
    use wasm_bindgen_futures::JsFuture;
    let window = web_sys::window().ok_or("no window")?;
    let clipboard = window.navigator().clipboard();
    let promise = clipboard.write_text(text);
    JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

/// In-app confirmation modal — replaces `window.confirm()` so destructive
/// actions get a styled dialog that matches the rest of the UI. Title,
/// message, optional details block, customisable button labels, and a
/// danger variant that paints the confirm button red.
#[derive(Props, Clone, PartialEq)]
pub struct ConfirmModalProps {
    pub title: String,
    pub message: String,
    /// Optional smaller-print follow-up under the main message — useful
    /// for "this also removes X" warnings.
    #[props(default = String::new())]
    pub details: String,
    #[props(default = "Confirm".to_string())]
    pub confirm_label: String,
    #[props(default = "Cancel".to_string())]
    pub cancel_label: String,
    #[props(default = false)]
    pub danger: bool,
    pub on_confirm: EventHandler<()>,
    pub on_cancel: EventHandler<()>,
}

#[component]
pub fn ConfirmModal(props: ConfirmModalProps) -> Element {
    let confirm_class = if props.danger {
        "btn-icon delete confirm-cta"
    } else {
        "primary"
    };
    let icon_name = if props.danger {
        "triangle-alert"
    } else {
        "circle-check"
    };
    let icon_class = if props.danger {
        "confirm-icon danger"
    } else {
        "confirm-icon"
    };
    rsx! {
        div { class: "modal-overlay", onclick: move |_| props.on_cancel.call(()),
            div {
                class: "modal confirm-modal",
                onclick: move |e| e.stop_propagation(),
                onkeydown: move |e| {
                    let key = e.key().to_string();
                    if key == "Escape" { props.on_cancel.call(()); }
                    else if key == "Enter" { props.on_confirm.call(()); }
                },
                tabindex: "-1",
                div { class: "modal-header",
                    h3 {
                        span { class: "{icon_class}", Icon { name: icon_name } }
                        "{props.title}"
                    }
                    button {
                        class: "ghost",
                        "data-tip": "Cancel",
                        onclick: move |_| props.on_cancel.call(()),
                        Icon { name: "x" }
                    }
                }
                div { class: "modal-body confirm-body",
                    p { class: "confirm-message", "{props.message}" }
                    if !props.details.is_empty() {
                        p { class: "confirm-details", "{props.details}" }
                    }
                }
                div { class: "modal-footer",
                    button {
                        onclick: move |_| props.on_cancel.call(()),
                        "{props.cancel_label}"
                    }
                    button {
                        class: "{confirm_class}",
                        autofocus: true,
                        onclick: move |_| props.on_confirm.call(()),
                        "{props.confirm_label}"
                    }
                }
            }
        }
    }
}

/// Shared indeterminate spinner — same SVG construction as the
/// determinate `CircularProgress` in cloud.rs but always renders the
/// rotating quarter-arc form. Intended for whole-page busy overlays
/// (e.g. config restore) where we don't have a percent and just want
/// "something is happening, do not click anything".
#[component]
pub fn Spinner(#[props(default = 24)] size: u32) -> Element {
    const RADIUS: f32 = 8.0;
    let circumference: f32 = 2.0 * std::f32::consts::PI * RADIUS;
    let dash = format!("{} {}", circumference / 4.0, circumference);
    rsx! {
        svg {
            class: "circ-progress spinning",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 20 20",
            role: "img",
            "aria-label": "loading",
            circle {
                class: "track",
                cx: "10", cy: "10", r: "{RADIUS}",
                fill: "none",
            }
            circle {
                class: "fill",
                cx: "10", cy: "10", r: "{RADIUS}",
                fill: "none",
                stroke_dasharray: "{dash}",
            }
        }
    }
}

// --- BannerKind (originally lived in webadmin-ui's main.rs) ---------------

#[derive(Clone, Copy, PartialEq)]
pub enum BannerKind {
    Ok,
    Err,
}
impl BannerKind {
    pub fn css(self) -> &'static str {
        match self {
            BannerKind::Ok => "ok",
            BannerKind::Err => "err",
        }
    }
}
