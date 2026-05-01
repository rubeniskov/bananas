//! BanaNAS Users SPA — composed into the webadmin shell at runtime
//! via the MFE flow. Owns user/admin/password management.

#![allow(non_snake_case)]

use dioxus::prelude::*;

mod api;
mod components;
mod icons;
mod users;

const MAIN_CSS: Asset = asset!("/assets/main.css");

fn main() {
    console_error_panic_hook::set_once();
    tracing_wasm::set_as_global_default();

    let root = web_sys::window()
        .and_then(|w| w.get("__bananas_mfe_root"))
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| "main".to_string());

    if let Some(window) = web_sys::window() {
        if let Some(document) = window.document() {
            if let Some(el) = document.get_element_by_id(&root) {
                el.set_inner_html("");
            }
        }
    }

    use std::rc::Rc;
    let history: Rc<dyn dioxus::history::History> =
        Rc::new(dioxus::history::MemoryHistory::default());
    dioxus::LaunchBuilder::new()
        .with_cfg(dioxus::web::Config::new().rootname(root).history(history))
        .launch(App);
}

fn is_mfe() -> bool {
    web_sys::window()
        .and_then(|w| w.get("__bananas_mfe_root"))
        .is_some()
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

    let mfe = is_mfe();
    rsx! {
        if !mfe {
            document::Stylesheet { href: MAIN_CSS }
        }
        match state() {
            AuthState::Loading | AuthState::SignedOut => rsx! {
                if mfe {
                    div { class: "loading-shell", p { "Loading…" } }
                } else {
                    main { class: "loading-shell", p { "Loading…" } }
                }
            },
            AuthState::SignedIn => rsx! {
                if mfe {
                    users::UsersPage {}
                } else {
                    main {
                        nav { class: "app-nav",
                            h1 { class: "app-title", "BanaNAS Users" }
                            span { class: "spacer" }
                            a {
                                class: "user-menu-trigger ghost",
                                href: "/",
                                "← Back to BanaNAS"
                            }
                        }
                        users::UsersPage {}
                    }
                }
            }
        }
    }
}
