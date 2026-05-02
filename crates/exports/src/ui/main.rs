//! BanaNAS Exports SPA — composed into the webadmin shell at
//! runtime via the MFE flow. Reuses the same session cookie issued
//! by `bananas-webadmin`'s /api/login (same origin, the browser
//! sends it verbatim). On a 401 the AuthCtx bounces the tab back
//! to `/` so the operator can sign in again.
//!
//! Why a separate crate (rather than a tab in the core webadmin SPA):
//! the exports feature is shipped by the optional `bananas-exports.ipk`,
//! which contains both the daemon binary and the wasm SPA. Operators
//! who don't run NFS exports get a leaner default image.

#![allow(non_snake_case)]

use dioxus::prelude::*;

mod api;
mod browse;
mod components;
mod exports;
mod icons;
mod nfs_help;
mod permissions;

fn main() {
    console_error_panic_hook::set_once();
    tracing_wasm::set_as_global_default();

    // MFE mount target — the webadmin shell sets
    // `window.__bananas_mfe_root = "exports-mfe-root"` before
    // injecting our script tag, so we mount inside the host SPA's
    // div instead of the standalone `#main` from index.html.
    // Direct access at /assets/exports/index.html is refused by
    // the daemon, so the only way the standalone fallback fires
    // is a dev server pointing directly at the dist bundle.
    let root = web_sys::window()
        .and_then(|w| w.get("__bananas_mfe_root"))
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| "main".to_string());

    if let Some(window) = web_sys::window() {
        if let Some(document) = window.document() {
            if let Some(el) = document.get_element_by_id(&root) {
                el.set_inner_html("");
                // Stop dioxus-delegated events from bubbling out of
                // this MFE root into the host's #main listener.
                // Without this, host's vdom looks up plugin
                // `data-dioxus-id`s in its own node table and
                // dispatches the wrong handler — visibly: clicking
                // a form input pops the host user-menu dropdown.
                bananas_mfe_runtime::isolate_root(&el);
            }
        }
    }

    // Same MemoryHistory swap we landed for cloud-ui (commit
    // a38b6c3): dioxus-web's default WebHistory rewrites
    // `window.location` on launch using `Dioxus.toml`'s base_path,
    // which produces a `/assets/exports/assets/exports`-shape
    // bug. MemoryHistory keeps the host SPA's URL state intact.
    use std::rc::Rc;
    let history: Rc<dyn dioxus::history::History> =
        Rc::new(dioxus::history::MemoryHistory::default());
    dioxus::LaunchBuilder::new()
        .with_cfg(dioxus::web::Config::new().rootname(root).history(history))
        .launch(App);
}

#[derive(Clone, Copy, PartialEq)]
pub enum AuthState {
    Loading,
    SignedOut,
    SignedIn,
}

#[derive(Clone, Copy)]
pub struct AuthCtx {
    pub state: Signal<AuthState>,
    pub me: Signal<Option<api::Me>>,
    pub refresh: Signal<u32>,
    pub busy: Signal<bool>,
}

impl AuthCtx {
    pub fn signal_unauthorized(self) {
        if let Some(window) = web_sys::window() {
            let _ = window.location().set_href("/");
        }
    }

    pub fn bump_refresh(mut self) {
        let n = self.refresh.peek().wrapping_add(1);
        self.refresh.set(n);
    }
}

#[component]
fn App() -> Element {
    let state = use_signal(|| AuthState::Loading);
    let me: Signal<Option<api::Me>> = use_signal(|| None);
    let refresh = use_signal(|| 0u32);
    let busy = use_signal(|| false);
    let auth_ctx = AuthCtx {
        state,
        me,
        refresh,
        busy,
    };
    use_context_provider(|| auth_ctx);

    {
        let mut state = state;
        let mut me = me;
        use_effect(move || {
            spawn(async move {
                match api::fetch_me().await {
                    Ok(Some(m)) => {
                        me.set(Some(m));
                        state.set(AuthState::SignedIn);
                    }
                    _ => {
                        state.set(AuthState::SignedOut);
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href("/");
                        }
                    }
                }
            });
        });
    }

    rsx! {
        match state() {
            AuthState::Loading | AuthState::SignedOut => rsx! {
                div { class: "loading-shell", p { "Loading…" } }
            },
            AuthState::SignedIn => rsx! { exports::ExportsPage {} }
        }
    }
}
