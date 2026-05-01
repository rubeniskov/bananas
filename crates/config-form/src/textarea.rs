//! Self-contained read-only textarea + Copy button. Lifted from
//! the per-plugin `components::TextareaWithCopy` (every plugin
//! had its own copy) so generated forms can render the
//! "Generated TOML" preview block without depending on the
//! plugin's local components module.
//!
//! The clipboard call falls back to focus+select when
//! `navigator.clipboard.writeText` is unavailable (HTTP origins,
//! older browsers).

#![allow(non_snake_case)]

use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct TextareaWithCopyProps {
    pub value: String,
    /// Stable DOM id on the inner textarea so the Copy fallback
    /// path can locate it via `document.getElementById`.
    pub id: &'static str,
}

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
                    if let Some(window) = web_sys::window()
                        && let Some(document) = window.document()
                        && let Some(el) = document.get_element_by_id(id_for_btn)
                    {
                        use wasm_bindgen::JsCast;
                        if let Ok(ta) = el.dyn_into::<web_sys::HtmlTextAreaElement>() {
                            ta.focus().ok();
                            ta.select();
                        }
                    }
                }
            }
        });
    };

    let label: &'static str = if copied() { "Copied" } else { "Copy" };
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
                "{label}"
            }
        }
    }
}

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
