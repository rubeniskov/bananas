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

fn main() {
    console_error_panic_hook::set_once();
    tracing_wasm::set_as_global_default();

    // Microfrontend mount target — the webadmin shell sets
    // `window.__bananas_mfe_root = "cloud-mfe-root"` before injecting
    // our script tag, so we mount inside the host SPA's div. The
    // plugin only ever loads through the host's MFE handshake; direct
    // navigation to `/assets/cloud/index.html` is rejected at the
    // daemon. If the global is unset we fall back to `"main"` so a
    // raw `dx serve` against this crate still has somewhere to render.
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

    // Override dioxus-web's default WebHistory provider — at launch,
    // it auto-discovers `Dioxus.toml`'s `base_path` via
    // `dioxus_cli_config::web_base_path()` and calls
    // `history.replaceState(null, "", base_path + current_route)`.
    // For us that produces the bug `/assets/cloud/assets/cloud`:
    // the host SPA had just set the URL to `/#cloud`, then cloud-ui's
    // bootstrap rewrites it to `<base_path>/<route_from_location>`,
    // and `route_from_location` returns the prefix verbatim when the
    // current path doesn't start with it (see dioxus-web 0.7.6's
    // `WebHistory::route_from_location` strip-or-fallback logic).
    //
    // Cloud-ui has no Router; it's composed into the host via MFE
    // and the host owns URL state. Swap in `MemoryHistory` so the
    // launch path never touches `window.history`.
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
        match state() {
            AuthState::Loading | AuthState::SignedOut => rsx! {
                div { class: "loading-shell", p { "Loading…" } }
            },
            AuthState::SignedIn => rsx! { cloud::CloudPage {} }
        }
    }
}
