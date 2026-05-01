//! Field-row widgets used by macro-generated `view` methods.
//!
//! Each widget renders `<div class="row"> <label> <input/select> <span/> </div>`
//! — same shape the hand-written forms used, so the existing CSS
//! (`.settings-form`, `.row`, `.hint`) styles them without changes.
//! The macro emits one widget call per non-skipped struct field.

#![allow(non_snake_case)]

use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct NumberRowProps {
    pub label: &'static str,
    #[props(default = "")]
    pub tooltip: &'static str,
    #[props(default = None)]
    pub min: Option<i64>,
    #[props(default = None)]
    pub max: Option<i64>,
    #[props(default = None)]
    pub step: Option<i64>,
    #[props(default = "")]
    pub placeholder: &'static str,
    #[props(default = true)]
    pub required: bool,
    pub value: Signal<String>,
}

#[component]
pub fn NumberRow(props: NumberRowProps) -> Element {
    let mut v = props.value;
    let min = props.min.map(|n| n.to_string());
    let max = props.max.map(|n| n.to_string());
    let step = props.step.map(|n| n.to_string());
    rsx! {
        div { class: "row",
            label { class: "hint", "data-tip": "{props.tooltip}", "{props.label}" }
            input {
                r#type: "number",
                required: props.required,
                placeholder: "{props.placeholder}",
                value: "{v()}",
                oninput: move |e| v.set(e.value()),
                min: min.as_deref().unwrap_or(""),
                max: max.as_deref().unwrap_or(""),
                step: step.as_deref().unwrap_or(""),
            }
            span {}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct TextRowProps {
    pub label: &'static str,
    #[props(default = "")]
    pub tooltip: &'static str,
    #[props(default = "")]
    pub placeholder: &'static str,
    #[props(default = true)]
    pub required: bool,
    pub value: Signal<String>,
}

#[component]
pub fn TextRow(props: TextRowProps) -> Element {
    let mut v = props.value;
    rsx! {
        div { class: "row",
            label { class: "hint", "data-tip": "{props.tooltip}", "{props.label}" }
            input {
                r#type: "text",
                required: props.required,
                placeholder: "{props.placeholder}",
                value: "{v()}",
                oninput: move |e| v.set(e.value()),
            }
            span {}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SelectRowProps {
    pub label: &'static str,
    #[props(default = "")]
    pub tooltip: &'static str,
    /// `(value, label)` pairs. The macro emits this as a static
    /// slice so the runtime can iterate without allocation.
    pub options: &'static [(&'static str, &'static str)],
    pub value: Signal<String>,
}

#[component]
pub fn SelectRow(props: SelectRowProps) -> Element {
    let mut v = props.value;
    rsx! {
        div { class: "row",
            label { class: "hint", "data-tip": "{props.tooltip}", "{props.label}" }
            select {
                value: "{v()}",
                onchange: move |e| v.set(e.value()),
                for (val, lab) in props.options.iter() {
                    option { value: "{val}", "{lab}" }
                }
            }
            span {}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct CsvRowProps {
    pub label: &'static str,
    #[props(default = "")]
    pub tooltip: &'static str,
    #[props(default = "")]
    pub placeholder: &'static str,
    pub value: Signal<String>,
}

/// Comma-separated text input. The signal value is the raw
/// `"a, b, c"` string; the macro-emitted `compose` parses it back
/// to `Vec<String>` on submit.
#[component]
pub fn CsvRow(props: CsvRowProps) -> Element {
    let mut v = props.value;
    rsx! {
        div { class: "row",
            label { class: "hint", "data-tip": "{props.tooltip}", "{props.label}" }
            input {
                r#type: "text",
                placeholder: "{props.placeholder}",
                value: "{v()}",
                oninput: move |e| v.set(e.value()),
            }
            span {}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SectionHeaderProps {
    pub legend: &'static str,
}

/// Macro emits one `<fieldset>` per group; this component is the
/// `<legend>` only — the macro wraps the field rows itself so it
/// can keep the children list typed.
#[component]
pub fn SectionHeader(props: SectionHeaderProps) -> Element {
    rsx! { legend { "{props.legend}" } }
}

#[derive(Props, Clone, PartialEq)]
pub struct SaveBarProps {
    pub busy: Signal<bool>,
    pub hydrated: Signal<bool>,
    /// Static label for the inline Save button. The two existing
    /// forms use "Save" (dashboard) and "Save & restart service"
    /// (stats). The macro reads
    /// `#[config_form(save_label = "…")]`.
    pub label: &'static str,
}

/// The inline `<div class="settings-actions">` Save block. The macro
/// emits this only when `inline_save = true`. Modal wrappers skip
/// it and provide their own Save button via the `form="…"` HTML
/// attribute pointing at the form's id.
#[component]
pub fn SaveBar(props: SaveBarProps) -> Element {
    let busy = props.busy;
    let hydrated = props.hydrated;
    rsx! {
        div { class: "settings-actions",
            button {
                class: "primary",
                r#type: "submit",
                disabled: busy() || !hydrated(),
                if busy() { "Saving…" } else { "{props.label}" }
            }
        }
    }
}
