//! Runtime for the BanaNAS config-form derive. Plugins shouldn't
//! depend on the proc-macro crate directly — they pull this in,
//! which re-exports the derive plus the row widgets, hooks and
//! save-button shell every generated form needs.
//!
//! The macro emits an inherent `view(props) -> Element` method per
//! struct. The method calls into [`hooks::use_form_state`],
//! [`widgets::*`], and the [`SaveBar`] component this crate provides
//! so the bulk of each form's render code doesn't end up in
//! macro-emitted token streams (smaller diffs in `cargo expand`,
//! easier to debug at runtime, no rsx authoring inside the macro).

#![cfg_attr(target_arch = "wasm32", allow(non_snake_case))]

pub use bananas_config_form_derive::ConfigForm;

#[cfg(target_arch = "wasm32")]
mod hooks;
#[cfg(target_arch = "wasm32")]
mod textarea;
#[cfg(target_arch = "wasm32")]
mod widgets;

#[cfg(target_arch = "wasm32")]
pub use hooks::{FormState, submit_config as __private_submit, use_form_state};
#[cfg(target_arch = "wasm32")]
pub use textarea::TextareaWithCopy;
#[cfg(target_arch = "wasm32")]
pub use widgets::{CsvRow, NumberRow, SaveBar, SectionHeader, SelectRow, TextRow};

#[cfg(target_arch = "wasm32")]
pub use dioxus::prelude::{EventHandler, Props};

/// Props every macro-emitted form accepts. The plugin wraps its
/// generated `view` in a tiny component that resolves the plugin's
/// own auth context and passes `unauthorized` through.
#[cfg(target_arch = "wasm32")]
mod props {
    use super::*;
    use dioxus::prelude::*;

    #[derive(Props, Clone, PartialEq)]
    pub struct ConfigFormProps {
        /// Mirror of `StatsConfigFormProps::inline_save`. When `true`
        /// (default), the macro emits a primary Save button at the
        /// bottom of the form. Modal wrappers pass `false` and render
        /// their own Save button in the modal footer with
        /// `form="<C::FORM_ID>"`.
        #[props(default = true)]
        pub inline_save: bool,
        /// Called when any GET/PUT to the config endpoint returns 401.
        /// The plugin's wrapper component reads its `AuthCtx` and
        /// passes through `auth_ctx.signal_unauthorized` here.
        pub on_unauthorized: EventHandler<()>,
    }
}

#[cfg(target_arch = "wasm32")]
pub use props::ConfigFormProps;

/// Wire protocol — the API endpoints all use the same
/// `{ "config": "<toml>" }` shape on both GET and PUT.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ConfigResp {
    #[serde(default)]
    pub config: String,
}

/// Top-level trait the proc-macro implements for any struct
/// annotated with `#[config_form(api = …)]`. Sub-section structs
/// get a leaner `Section` impl instead.
pub trait ConfigForm: Default + serde::Serialize + serde::de::DeserializeOwned {
    /// API endpoint this form GETs and PUTs. Same path for both.
    const ENDPOINT: &'static str;
    /// HTML `id` of the `<form>` element. Used by modal-footer Save
    /// buttons (`<button form="…">`) so they submit the in-body
    /// form via standard HTML semantics.
    const FORM_ID: &'static str;
    /// Lead paragraph rendered above the form fields. Plain text;
    /// no rsx in attributes.
    const DESCRIPTION: &'static str;
    /// Banner shown after a successful save.
    const SUCCESS_MSG: &'static str;
}
