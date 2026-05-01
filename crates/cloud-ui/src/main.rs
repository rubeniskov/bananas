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

    // Microfrontend mount target — the webadmin shell sets
    // `window.__bananas_mfe_root = "cloud-mfe-root"` before injecting
    // our script tag, so we mount inside the host SPA's div instead of
    // the standalone `#main` from index.html. Direct access at
    // `/cloud/` (no host) leaves the global unset and we fall back to
    // `"main"` — useful for debugging the cloud SPA in isolation.
    let root = web_sys::window()
        .and_then(|w| w.get("__bananas_mfe_root"))
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| "main".to_string());

    // Dioxus's web bootstrap appends its render output as children of
    // the rootname element rather than replacing them — that leaves
    // the host's "Loading Cloud module…" placeholder visible next to
    // our render. Wipe the mount target's children before we launch
    // so the host's placeholder vanishes the moment we take over.
    if let Some(window) = web_sys::window() {
        if let Some(document) = window.document() {
            if let Some(el) = document.get_element_by_id(&root) {
                el.set_inner_html("");
            }
        }
    }

    dioxus::LaunchBuilder::new()
        .with_cfg(dioxus::web::Config::new().rootname(root))
        .launch(App);
}

/// True when this SPA is being composed into the webadmin shell as a
/// microfrontend (i.e. `window.__bananas_mfe_root` is set). The shell
/// already paints the BanaNAS top-nav, so we hide our own
/// "BanaNAS Cloud / ← Back to BanaNAS" bar in that mode and let the
/// host own page chrome.
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

    let mfe = is_mfe();
    rsx! {
        // In MFE mode the host already injects its own stylesheet and
        // wraps the content in <main>. Suppress both here so we don't
        // get double-loaded CSS or a nested <main><main> that doubles
        // the page-level padding/margin.
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
                    // Host owns <main> — render content directly so
                    // CSS rules on `main` don't apply twice.
                    cloud::CloudPage {}
                } else {
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
}
