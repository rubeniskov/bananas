//! BanaNAS web UI — Dioxus 0.7 single-page app.
//!
//! Talks to the `crates/server` JSON API at /api/*. The server also serves
//! the bundled wasm/CSS/HTML (the `dist/` directory written by `dx build`).

#![allow(non_snake_case)]

use dioxus::prelude::*;

mod api;
mod browse;
mod cloud;
mod components;
mod exports;
mod icons;
mod login;
mod mounts;
mod nfs_help;
mod permissions;
mod stats;
mod stats_config;
mod storage;
mod tooltip;
mod users;

const MAIN_CSS: Asset = asset!("/assets/main.css");

fn main() {
    console_error_panic_hook::set_once();
    tracing_wasm::set_as_global_default();
    tooltip::init();
    dioxus::launch(App);
}

#[derive(Clone, Copy, PartialEq)]
pub enum AuthState {
    Loading,
    SignedOut,
    SignedIn,
}

/// Context carried by `App` so any descendant can flip auth state when an
/// API call returns 401. Lets `ExportsPage` and friends bounce the user
/// back to the login screen without prop-drilling a callback through every
/// component layer.
#[derive(Clone, Copy)]
pub struct AuthCtx {
    pub state: Signal<AuthState>,
    pub me: Signal<Option<api::Me>>,
    /// Bump to force every page that observes it to re-fetch its data.
    /// Used by the "Load config" flow so freshly imported exports/fstab/
    /// users show up without a manual refresh.
    pub refresh: Signal<u32>,
}

impl AuthCtx {
    /// Call from any component that received `ApiError::Unauthorized`.
    pub fn signal_unauthorized(mut self) {
        self.me.set(None);
        self.state.set(AuthState::SignedOut);
    }

    /// Tell every page-level fetcher to re-run. Pages that observe the
    /// returned tick re-query their respective endpoints.
    pub fn bump_refresh(mut self) {
        let n = self.refresh.peek().wrapping_add(1);
        self.refresh.set(n);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Stats,
    Exports,
    Storage,
    Users,
    Cloud,
}

impl Page {
    /// URL-hash slug used to make the current page survive a full-page
    /// reload. Hash-based (vs path-based) so the static SPA fallback at
    /// `/` doesn't need server-side routing rules.
    fn slug(self) -> &'static str {
        match self {
            Page::Stats => "stats",
            Page::Exports => "exports",
            Page::Storage => "storage",
            Page::Users => "users",
            Page::Cloud => "cloud",
        }
    }

    fn from_slug(s: &str) -> Option<Self> {
        match s {
            "stats" => Some(Page::Stats),
            "exports" => Some(Page::Exports),
            "storage" => Some(Page::Storage),
            "users" => Some(Page::Users),
            "cloud" => Some(Page::Cloud),
            _ => None,
        }
    }
}

/// Resolve the current Page from `window.location.hash`. Stats is the
/// default landing page — only an explicit `/#<slug>` switches to a
/// different tab, so legacy path-style URLs (`/exports`, `/users`, …)
/// from earlier builds land on Stats and get rewritten by the
/// canonicalize step below.
fn read_page_from_url() -> Page {
    let Some(window) = web_sys::window() else {
        return Page::Stats;
    };
    let hash = window.location().hash().unwrap_or_default();
    let slug = hash.trim_start_matches('#').trim_start_matches('/');
    Page::from_slug(slug).unwrap_or(Page::Stats)
}

/// Replace the current entry with `/#<slug>` so the visible URL stays
/// canonical regardless of how the user got here (typed `/exports`,
/// followed an old `/users#users` bookmark, etc.). Uses `replaceState`
/// instead of `set_hash` so the path component is also normalized and
/// nav-tab clicks don't push a history entry per click.
fn canonicalize_url(page: Page) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let target = format!("/#{}", page.slug());
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&target));
    }
}

#[derive(Props, Clone, PartialEq)]
struct SignedInShellProps {
    username: String,
}

#[component]
fn SignedInShell(props: SignedInShellProps) -> Element {
    let mut auth_ctx = use_context::<AuthCtx>();
    let mut page: Signal<Page> = use_signal(read_page_from_url);

    // Mirror page changes into the URL via `history.replaceState` so
    // the visible URL is always `/#<slug>` (no `/exports#exports`
    // amalgams) and reloads preserve the current tab.
    use_effect(move || {
        canonicalize_url(page());
    });
    use_effect(move || {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;
        let Some(window) = web_sys::window() else {
            return;
        };
        let cb = Closure::<dyn FnMut()>::new(move || {
            let next = read_page_from_url();
            if *page.peek() != next {
                page.set(next);
            }
        });
        let _ = window.add_event_listener_with_callback("hashchange", cb.as_ref().unchecked_ref());
        cb.forget();
    });
    let mut config_banner: Signal<Option<(BannerKind, String)>> = use_signal(|| None);

    let logout = move |_| {
        spawn(async move {
            let _ = api::logout().await;
            auth_ctx.me.set(None);
            auth_ctx.state.set(AuthState::SignedOut);
        });
    };

    let save_config = move |_| {
        spawn(async move {
            match api::fetch_config_toml().await {
                Ok(toml) => match download_text(&toml, "application/toml") {
                    Ok(()) => config_banner.set(Some((
                        BannerKind::Ok,
                        "Saved config to your downloads.".into(),
                    ))),
                    Err(e) => {
                        config_banner.set(Some((BannerKind::Err, format!("Download failed: {e}"))))
                    }
                },
                Err(api::ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => config_banner.set(Some((BannerKind::Err, e.to_string()))),
            }
        });
    };

    // File input → read selected file via web_sys (Dioxus' FileData
    // abstraction is finicky in 0.7, and we already use web_sys for the
    // download). Look up the input element by id from the global DOM.
    let load_change = move |_: dioxus::prelude::Event<dioxus::prelude::FormData>| {
        use wasm_bindgen::JsCast;
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let Some(el) = document.get_element_by_id("load-config-input") else {
            return;
        };
        let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() else {
            return;
        };
        let Some(files) = input.files() else { return };
        let Some(file) = files.get(0) else { return };
        let promise = file.text();
        let future = wasm_bindgen_futures::JsFuture::from(promise);
        // Reset the input so the same file can be picked again next time.
        input.set_value("");
        spawn(async move {
            match future.await {
                Ok(val) => {
                    let text = val.as_string().unwrap_or_default();
                    match api::upload_config_toml(&text).await {
                        Ok(summary) => {
                            let mut msg = format!(
                                "Imported: {} exports, {} fstab entries, {} users restored ({} skipped).",
                                summary.exports_written,
                                summary.fstab_written,
                                summary.users_created,
                                summary.users_skipped,
                            );
                            if !summary.notes.is_empty() {
                                msg.push_str("\n\n");
                                msg.push_str(&summary.notes.join("\n"));
                            }
                            let kind = if summary.ok {
                                BannerKind::Ok
                            } else {
                                BannerKind::Err
                            };
                            config_banner.set(Some((kind, msg)));
                            // Tell every page subscribing to the refresh
                            // tick (Exports, Mounts, Users, Storage) to
                            // re-fetch so the imported data is visible
                            // without a manual hit on the Refresh button.
                            auth_ctx.bump_refresh();
                        }
                        Err(api::ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                        Err(e) => config_banner.set(Some((BannerKind::Err, e.to_string()))),
                    }
                }
                Err(_) => config_banner.set(Some((BannerKind::Err, "Could not read file".into()))),
            }
        });
    };

    rsx! {
        main {
            nav { class: "app-nav",
                h1 { class: "app-title", "BanaNAS" }
                NavTab { label: "Stats", icon: "chart-bar", active: page() == Page::Stats,
                    on_click: move |_| page.set(Page::Stats) }
                NavTab { label: "Exports", icon: "share-2", active: page() == Page::Exports,
                    on_click: move |_| page.set(Page::Exports) }
                NavTab { label: "Storage", icon: "hard-drive", active: page() == Page::Storage,
                    on_click: move |_| page.set(Page::Storage) }
                NavTab { label: "Users", icon: "users", active: page() == Page::Users,
                    on_click: move |_| page.set(Page::Users) }
                NavTab { label: "Cloud", icon: "cloud", active: page() == Page::Cloud,
                    on_click: move |_| page.set(Page::Cloud) }
                span { class: "spacer" }
                div { class: "nav-actions",
                    button {
                        class: "ghost",
                        "data-tip": "Download the current exports + fstab + users as a TOML backup.",
                        onclick: move |_| save_config(()),
                        icons::Icon { name: "download" }
                        "Save config"
                    }
                    label {
                        class: "ghost button-like",
                        "data-tip": "Pick a TOML file to apply. Restores exports + fstab + users.",
                        icons::Icon { name: "upload" }
                        "Load config"
                        input {
                            id: "load-config-input",
                            r#type: "file",
                            accept: ".toml,application/toml,text/plain",
                            style: "display: none",
                            onchange: load_change,
                        }
                    }
                    button { class: "ghost", onclick: logout,
                        icons::Icon { name: "log-out" }
                        "Sign out"
                    }
                }
                span { class: "nav-user", "{props.username}" }
            }

            if let Some((kind, msg)) = config_banner() {
                div { class: "banner {kind.css()} config-banner",
                    pre { "{msg}" }
                    button { class: "ghost",
                        onclick: move |_| config_banner.set(None),
                        "✕"
                    }
                }
            }

            match page() {
                Page::Stats => rsx! { stats::StatsPage {} },
                Page::Exports => rsx! { exports::ExportsPage {} },
                Page::Storage => rsx! { storage::StoragePage {} },
                Page::Users => rsx! { users::UsersPage {} },
                Page::Cloud => rsx! { cloud::CloudPage {} },
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum BannerKind {
    Ok,
    Err,
}
impl BannerKind {
    fn css(self) -> &'static str {
        match self {
            BannerKind::Ok => "ok",
            BannerKind::Err => "err",
        }
    }
}

/// Trigger a browser download of `text` as `application/toml`. Builds a
/// Blob, gets a temporary object URL, attaches a hidden anchor with
/// `download="…"`, clicks it, then revokes the URL. Pure web_sys — no
/// extra deps.
fn download_text(text: &str, mime: &str) -> Result<(), String> {
    use wasm_bindgen::{JsCast, JsValue};

    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;

    let parts = js_sys::Array::new();
    parts.push(&JsValue::from_str(text));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type(mime);
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &bag)
        .map_err(|_| "blob construction failed".to_string())?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|_| "URL.createObjectURL failed".to_string())?;

    let anchor = document
        .create_element("a")
        .map_err(|_| "create_element failed")?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| "anchor cast failed")?;
    anchor.set_href(&url);
    anchor.set_download(&format!("bananas-config-{}.toml", date_stamp()));
    document.body().ok_or("no body")?.append_child(&anchor).ok();
    anchor.click();
    document.body().ok_or("no body")?.remove_child(&anchor).ok();
    web_sys::Url::revoke_object_url(&url).ok();
    Ok(())
}

fn date_stamp() -> String {
    // Local-time YYYYMMDD-HHmmss using the JS Date object — no chrono.
    let date = js_sys::Date::new_0();
    format!(
        "{}{:02}{:02}-{:02}{:02}{:02}",
        date.get_full_year(),
        (date.get_month() + 1) as u32,
        date.get_date() as u32,
        date.get_hours() as u32,
        date.get_minutes() as u32,
        date.get_seconds() as u32,
    )
}

#[derive(Props, Clone, PartialEq)]
struct NavTabProps {
    label: String,
    #[props(default = "")]
    icon: &'static str,
    active: bool,
    on_click: EventHandler<()>,
}

#[component]
fn NavTab(props: NavTabProps) -> Element {
    let class = if props.active { "active" } else { "" };
    rsx! {
        a {
            class: "{class}",
            href: "javascript:void(0)",
            onclick: move |_| props.on_click.call(()),
            if !props.icon.is_empty() { icons::Icon { name: props.icon } }
            "{props.label}"
        }
    }
}

#[component]
fn App() -> Element {
    let mut auth: Signal<AuthState> = use_signal(|| AuthState::Loading);
    let mut me: Signal<Option<api::Me>> = use_signal(|| None);
    let refresh: Signal<u32> = use_signal(|| 0);
    use_context_provider(|| AuthCtx {
        state: auth,
        me,
        refresh,
    });

    use_effect(move || {
        spawn(async move {
            match api::fetch_me().await {
                Ok(Some(user)) => {
                    me.set(Some(user));
                    auth.set(AuthState::SignedIn);
                }
                Ok(None) => auth.set(AuthState::SignedOut),
                Err(_) => auth.set(AuthState::SignedOut),
            }
        });
    });

    rsx! {
        document::Stylesheet { href: MAIN_CSS }
        match auth() {
            AuthState::Loading => rsx! {
                main { class: "loading-shell", p { "Loading…" } }
            },
            AuthState::SignedOut => rsx! {
                login::Login {
                    on_signed_in: move |user| {
                        me.set(Some(user));
                        auth.set(AuthState::SignedIn);
                    }
                }
            },
            AuthState::SignedIn => {
                let username = me().map(|m| m.username).unwrap_or_default();
                rsx! { SignedInShell { username } }
            }
        }
    }
}
