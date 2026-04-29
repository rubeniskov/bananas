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
