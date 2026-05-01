//! BanaNAS Cloud SPA — small Dioxus app served by `bananas-cloud` at
//! `/cloud/`. Reuses the same session cookie issued by
//! `bananas-webadmin`'s /api/login (same origin, the browser sends it
//! verbatim). On a 401 from any /api/* call, the AuthCtx bounces the
//! tab back to `/` so the operator can sign in again.
//!
//! Why a separate crate (rather than a tab in the core webadmin SPA):
//! the cloud feature is shipped by the optional `bananas-cloud.ipk`,
//! which RDEPENDS this crate's `bananas-cloud-ui.ipk`. Operators who
//! don't run cloud sync get a leaner default image (no rclone, no
//! cloud wasm).

#![allow(non_snake_case)]

use dioxus::prelude::*;

mod api;
mod browse;
mod cloud;
mod components;
mod icons;

const MAIN_CSS: Asset = asset!("/assets/main.css");

fn main() {
    console_error_panic_hook::set_once();
    tracing_wasm::set_as_global_default();
    dioxus::launch(App);
}

#[derive(Clone, Copy, PartialEq)]
pub enum AuthState {
    Loading,
    SignedOut,
    SignedIn,
}

/// Cloud-SPA-local auth context. Smaller than webadmin-ui's because
/// the cloud SPA can't actually sign anyone in — login lives at /,
/// served by bananas-webadmin. On 401 we just bounce back there.
#[derive(Clone, Copy)]
pub struct AuthCtx {
    pub state: Signal<AuthState>,
    pub me: Signal<Option<api::Me>>,
    pub refresh: Signal<u32>,
    pub busy: Signal<bool>,
}

impl AuthCtx {
    /// Bounce back to `/` (where bananas-webadmin's login flow is).
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

    // Verify session before rendering the cloud page. /api/me hits the
    // gateway → webadmin (catch-all "/"), which returns 200 + Me on a
    // valid cookie or 401. Anything other than 200 sends us back to /.
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
        document::Stylesheet { href: MAIN_CSS }
        match state() {
            AuthState::Loading | AuthState::SignedOut => rsx! {
                main { class: "loading-shell",
                    p { "Loading…" }
                }
            },
            AuthState::SignedIn => rsx! {
                main {
                    nav { class: "app-nav",
                        h1 { class: "app-title", "BanaNAS Cloud" }
                        span { class: "spacer" }
                        a {
                            class: "user-menu-trigger ghost",
                            href: "/",
                            "← Back to BanaNAS"
                        }
                    }
                    cloud::CloudPage {}
                }
            }
        }
    }
}
