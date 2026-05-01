//! BanaNAS web UI — Dioxus 0.7 single-page app.
//!
//! Talks to the `crates/webadmin` JSON API at /api/*. The server also serves
//! the bundled wasm/CSS/HTML (the `dist/` directory written by `dx build`).

#![allow(non_snake_case)]

use dioxus::prelude::*;

mod api;
mod browse;
mod components;
mod dashboard_config;
mod exports;
mod icons;
mod login;
mod mfe;
mod mounts;
mod nfs_help;
mod permissions;
mod settings;
mod stats;
mod stats_config;
mod storage;
mod theme;
mod tooltip;
mod updates;
mod users;

const MAIN_CSS: Asset = asset!("/assets/main.css");

fn main() {
    console_error_panic_hook::set_once();
    tracing_wasm::set_as_global_default();
    tooltip::init();
    // Apply persisted theme to <html> before Dioxus mounts so the
    // first paint already reflects the operator's choice — avoids a
    // brief light-flash when reloading on a dark-themed setup.
    theme::apply(theme::load());
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
    /// True while a long-running mutating operation (config restore,
    /// password rotation, etc.) is in flight. Pages observing this
    /// disable their "save", "delete", "run-now" controls and a
    /// modal-style overlay covers the whole admin to prevent racing
    /// API calls against an in-flight import.
    pub busy: Signal<bool>,
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
    Settings,
    Updates,
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
            Page::Settings => "settings",
            Page::Updates => "updates",
        }
    }

    fn from_slug(s: &str) -> Option<Self> {
        match s {
            "stats" => Some(Page::Stats),
            "exports" => Some(Page::Exports),
            "storage" => Some(Page::Storage),
            "users" => Some(Page::Users),
            "cloud" => Some(Page::Cloud),
            "settings" => Some(Page::Settings),
            "updates" => Some(Page::Updates),
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
    let mut reboot_confirm = use_signal(|| false);

    // Installed extensions (read once on mount). The Cloud / future
    // plugin tabs render only when the matching manifest is present in
    // /etc/bananas/extensions.d/, so a lean image without bananas-cloud
    // installed shows no Cloud tab at all. We keep the full Extension
    // records (not just ids) so the MFE loader can read each plugin's
    // `spa_path` to discover its bundle.
    let mut extensions: Signal<Vec<api::Extension>> = use_signal(Vec::new);
    {
        use_effect(move || {
            spawn(async move {
                if let Ok(list) = api::list_extensions().await {
                    extensions.set(list);
                }
            });
        });
    }

    // Per-plugin MFE state. Each entry is one of:
    //   None         — not requested yet (mount div absent)
    //   Some(Ok(())) — loaded; mount div live; toggle hidden via CSS
    //   Some(Err(_)) — load failed; mount div shows the retry panel
    let mut mfe_state: Signal<std::collections::HashMap<String, Result<(), String>>> =
        use_signal(std::collections::HashMap::new);

    // Trigger an MFE load on first activation of a plugin tab. Memoized
    // via `is_loaded`/`mfe_state` so subsequent activations are a no-op.
    let mut activate_plugin = move |id: String, spa_path: String| {
        if mfe::is_loaded(&id) {
            mfe_state.write().insert(id, Ok(()));
            return;
        }
        if matches!(mfe_state.read().get(&id), Some(Ok(()))) {
            return;
        }
        spawn(async move {
            let result = mfe::load(&id, &spa_path).await;
            mfe_state.write().insert(id, result);
        });
    };

    // Reload-on-#cloud handling: if the operator hits refresh while on
    // /#cloud (or pastes the deep link), `page` is already Cloud at
    // mount time but no NavTab click ever fires `activate_plugin`. This
    // effect bridges that gap — it runs whenever extensions resolve or
    // the page changes, and is itself idempotent because activate_plugin
    // short-circuits on already-loaded plugins.
    use_effect(move || {
        if page() != Page::Cloud {
            return;
        }
        let cloud_ext = extensions
            .read()
            .iter()
            .find(|e| e.id.as_str() == "cloud")
            .cloned();
        if let Some(ext) = cloud_ext {
            let spa = ext.spa_path.clone().unwrap_or_else(|| "/cloud".to_string());
            activate_plugin(ext.id.clone(), spa);
        }
    });

    // On mount: discover any in-flight ConfigImport op and reattach the
    // busy overlay to it. This is the "refresh during import" path —
    // without it, the SPA forgets the import is happening and the user
    // sees Exports/Mounts/Users showing pre-import data even though
    // the server is mid-write.
    {
        let auth_ctx = auth_ctx.clone();
        use_effect(move || {
            let mut auth_ctx = auth_ctx.clone();
            spawn(async move {
                match api::list_active_operations().await {
                    Ok(ops) => {
                        for op in ops {
                            if matches!(op.kind, api::OperationKind::ConfigImport) {
                                auth_ctx.busy.set(true);
                                watch_config_import_op(op.id, config_banner, auth_ctx.clone());
                                break;
                            }
                        }
                    }
                    Err(api::ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                    Err(_) => {}
                }
            });
        });
    }
    // Dropdown that consolidates Save/Load config, Reboot, and Sign out
    // behind one menu trigger at the top-right. Closes on backdrop click
    // or after any of the actions fires.
    let mut menu_open = use_signal(|| false);
    // Active theme — persisted in localStorage. Read once at mount; the
    // setter below keeps DOM + storage in sync.
    let mut current_theme = use_signal(theme::load);

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
    let mut load_change = move |_: dioxus::prelude::Event<dioxus::prelude::FormData>| {
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
        // Flip the global busy flag so every page-level control
        // disables and the BusyOverlay fades in. The flag stays set
        // until the OperationManager-tracked op transitions to a
        // terminal status — see `watch_config_import_op` for the poll
        // loop. A browser refresh while the op is in flight will
        // re-discover it via /api/operations/active and resume the
        // overlay (see use_effect at the top of this component).
        auth_ctx.busy.set(true);
        spawn(async move {
            match future.await {
                Ok(val) => {
                    let text = val.as_string().unwrap_or_default();
                    match api::upload_config_toml(&text).await {
                        Ok(accepted) => {
                            watch_config_import_op(accepted.op_id, config_banner, auth_ctx.clone());
                        }
                        Err(api::ApiError::Unauthorized) => {
                            auth_ctx.busy.set(false);
                            auth_ctx.signal_unauthorized();
                        }
                        Err(e) => {
                            auth_ctx.busy.set(false);
                            config_banner.set(Some((BannerKind::Err, e.to_string())));
                        }
                    }
                }
                Err(_) => {
                    auth_ctx.busy.set(false);
                    config_banner.set(Some((BannerKind::Err, "Could not read file".into())));
                }
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
                {
                    // Cloud lives in its own SPA bundle (bananas-cloud-ui)
                    // served by the cloud daemon at /cloud/. Rather than
                    // a full-page nav, we compose it into this same
                    // document — see crate::mfe. The nav tab is rendered
                    // only when the manifest is present.
                    let cloud_ext = extensions
                        .read()
                        .iter()
                        .find(|e| e.id.as_str() == "cloud")
                        .cloned();
                    rsx! {
                        if let Some(ext) = cloud_ext {
                            NavTab {
                                label: "Cloud",
                                icon: "cloud",
                                active: page() == Page::Cloud,
                                on_click: move |_| {
                                    let spa = ext
                                        .spa_path
                                        .clone()
                                        .unwrap_or_else(|| "/cloud".to_string());
                                    activate_plugin(ext.id.clone(), spa);
                                    page.set(Page::Cloud);
                                },
                            }
                        }
                    }
                }
                NavTab { label: "Settings", icon: "settings", active: page() == Page::Settings,
                    on_click: move |_| page.set(Page::Settings) }
                NavTab { label: "Updates", icon: "package", active: page() == Page::Updates,
                    on_click: move |_| page.set(Page::Updates) }
                span { class: "spacer" }
                div { class: "user-menu",
                    span { class: "user-greeting", "Welcome, ", strong { "{props.username}" }, "!" }
                    button {
                        class: "user-menu-trigger ghost",
                        "data-tip": "Account & system actions",
                        disabled: auth_ctx.busy.read().clone(),
                        onclick: move |_| menu_open.set(!menu_open()),
                        "aria-expanded": "{menu_open()}",
                        "aria-haspopup": "menu",
                        icons::Icon { name: "circle-user-round" }
                        icons::Icon { name: "chevron-down", class: "user-menu-chevron" }
                    }
                    if menu_open() {
                        div {
                            class: "user-menu-backdrop",
                            onclick: move |_| menu_open.set(false),
                        }
                        div { class: "user-menu-popover", role: "menu",
                            button {
                                class: "user-menu-item",
                                role: "menuitem",
                                disabled: auth_ctx.busy.read().clone(),
                                onclick: move |_| {
                                    menu_open.set(false);
                                    save_config(());
                                },
                                icons::Icon { name: "download" }
                                span { "Save config" }
                            }
                            {
                                let busy = auth_ctx.busy.read().clone();
                                if busy {
                                    rsx! {
                                        button {
                                            class: "user-menu-item",
                                            role: "menuitem",
                                            disabled: true,
                                            icons::Icon { name: "upload" }
                                            span { "Load config" }
                                        }
                                    }
                                } else {
                                    rsx! {
                                        label {
                                            class: "user-menu-item",
                                            role: "menuitem",
                                            icons::Icon { name: "upload" }
                                            span { "Load config" }
                                            input {
                                                id: "load-config-input",
                                                r#type: "file",
                                                accept: ".toml,application/toml,text/plain",
                                                style: "display: none",
                                                onchange: move |evt: dioxus::prelude::Event<dioxus::prelude::FormData>| {
                                                    menu_open.set(false);
                                                    load_change(evt);
                                                },
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "user-menu-sep" }
                            div { class: "theme-picker", role: "group", "aria-label": "Theme",
                                span { class: "theme-picker-label", "Theme" }
                                {
                                    let active = current_theme();
                                    let opts = [theme::Theme::Auto, theme::Theme::Light, theme::Theme::Dark];
                                    rsx! {
                                        for t in opts {
                                            {
                                                let cls = if active == t {
                                                    "theme-picker-btn active"
                                                } else {
                                                    "theme-picker-btn"
                                                };
                                                rsx! {
                                                    button {
                                                        class: "{cls}",
                                                        r#type: "button",
                                                        "data-tip": "{t.label()}",
                                                        onclick: move |_| {
                                                            theme::apply(t);
                                                            current_theme.set(t);
                                                        },
                                                        icons::Icon { name: t.icon() }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "user-menu-sep" }
                            button {
                                class: "user-menu-item danger",
                                role: "menuitem",
                                disabled: auth_ctx.busy.read().clone(),
                                onclick: move |_| {
                                    menu_open.set(false);
                                    reboot_confirm.set(true);
                                },
                                icons::Icon { name: "power" }
                                span { "Reboot" }
                            }
                            button {
                                class: "user-menu-item",
                                role: "menuitem",
                                disabled: auth_ctx.busy.read().clone(),
                                onclick: move |evt| {
                                    menu_open.set(false);
                                    logout(evt);
                                },
                                icons::Icon { name: "log-out" }
                                span { "Sign out" }
                            }
                        }
                    }
                }
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

            // Whole-shell loading overlay. Rendered inside <main> so it
            // covers nav + banner + page content. .busy-overlay's
            // pointer-events: all blocks every click underneath, so even
            // a row-action button left :enabled cannot fire while the
            // import is in flight. Reuses CircularProgress in its
            // indeterminate (None) form for the spinner.
            if auth_ctx.busy.read().clone() {
                div { class: "busy-overlay",
                    div { class: "busy-card",
                        components::Spinner { size: 32 }
                        span { class: "busy-text", "Applying configuration…" }
                    }
                }
            }

            match page() {
                Page::Stats => rsx! { stats::StatsPage {} },
                Page::Exports => rsx! { exports::ExportsPage {} },
                Page::Storage => rsx! { storage::StoragePage {} },
                Page::Users => rsx! { users::UsersPage {} },
                Page::Settings => rsx! { settings::SettingsPage {} },
                Page::Updates => rsx! { updates::UpdatesPage {} },
                // Cloud renders nothing here — its mount div lives
                // outside the match so it can stay in DOM across tab
                // switches (Dioxus would unmount/remount otherwise,
                // dropping the plugin's runtime state on every flick
                // back to Stats and forcing a full re-fetch on return).
                Page::Cloud => rsx! { },
            }

            // Cloud microfrontend mount + status panel. Always rendered
            // when the cloud manifest is installed; hidden via CSS when
            // a different tab is active. The plugin's `main()` mounts
            // its app into `<div id="cloud-mfe-root">` once the script
            // tag is injected (see crate::mfe).
            {
                let cloud_active = page() == Page::Cloud;
                let cloud_installed = extensions
                    .read()
                    .iter()
                    .any(|e| e.id.as_str() == "cloud");
                let cloud_status = mfe_state.read().get("cloud").cloned();
                let frame_class = if cloud_active {
                    "mfe-frame"
                } else {
                    "mfe-frame hidden"
                };
                rsx! {
                    if cloud_installed {
                        div { class: "{frame_class}",
                            // The plugin replaces these children once
                            // its runtime mounts. Until then, we show
                            // a spinner (or an error+retry panel if the
                            // discovery/inject failed).
                            div {
                                id: "cloud-mfe-root",
                                class: "mfe-mount",
                                match cloud_status {
                                    Some(Err(err)) => rsx! {
                                        div { class: "mfe-loading mfe-failed",
                                            p { class: "mfe-failed-title", "Cloud module failed to load." }
                                            pre { class: "mfe-failed-detail", "{err}" }
                                            button {
                                                class: "primary",
                                                onclick: move |_| {
                                                    if let Some(ext) = extensions
                                                        .read()
                                                        .iter()
                                                        .find(|e| e.id.as_str() == "cloud")
                                                        .cloned()
                                                    {
                                                        let spa = ext
                                                            .spa_path
                                                            .clone()
                                                            .unwrap_or_else(|| "/cloud".to_string());
                                                        mfe_state.write().remove("cloud");
                                                        activate_plugin(ext.id.clone(), spa);
                                                    }
                                                },
                                                "Retry"
                                            }
                                        }
                                    },
                                    _ => rsx! {
                                        div { class: "mfe-loading",
                                            components::Spinner { size: 32 }
                                            span { "Loading Cloud module…" }
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }

            if reboot_confirm() {
                components::ConfirmModal {
                    title: "Reboot BanaNAS?".to_string(),
                    message: "The web UI will drop for ~30 s while systemd reboots the system.".to_string(),
                    details: "Any in-flight cloud-sync runs will be interrupted; cron will pick the schedule back up after boot.".to_string(),
                    confirm_label: "Reboot".to_string(),
                    danger: true,
                    on_cancel: move |_| reboot_confirm.set(false),
                    on_confirm: move |_| {
                        reboot_confirm.set(false);
                        spawn(async move {
                            let _ = api::reboot_system().await;
                            // Don't bother surfacing a success banner —
                            // the server is going down and the browser
                            // will throw a connection error any moment.
                        });
                    },
                }
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

/// Poll the OperationManager-tracked ConfigImport op until it
/// reaches a terminal status, then surface the structured summary as
/// a banner and bump the page-refresh tick. Holds `auth_ctx.busy`
/// at `true` for the duration so the BusyOverlay stays visible.
///
/// This is shared between the click-to-import path (kicks off a new
/// op) and the on-mount discovery path (reattaches to one already in
/// flight from a prior page load).
fn watch_config_import_op(
    op_id: u64,
    mut config_banner: Signal<Option<(BannerKind, String)>>,
    auth_ctx: AuthCtx,
) {
    let mut busy_signal = auth_ctx.busy;
    spawn(async move {
        const POLL_MS: i32 = 500;
        loop {
            match api::get_operation(op_id).await {
                Ok(op) => {
                    let terminal = !matches!(op.status, api::OperationStatus::Running);
                    if terminal {
                        let summary = parse_import_summary(&op.output);
                        let kind = match (op.status.clone(), summary.as_ref()) {
                            (api::OperationStatus::Success, _) => BannerKind::Ok,
                            (_, Some(s)) if s.ok => BannerKind::Ok,
                            _ => BannerKind::Err,
                        };
                        let msg = match summary {
                            Some(s) => {
                                let mut m = format!(
                                    "Imported: {} exports, {} fstab entries, {} users restored ({} skipped).",
                                    s.exports_written,
                                    s.fstab_written,
                                    s.users_created,
                                    s.users_skipped,
                                );
                                if !s.notes.is_empty() {
                                    m.push_str("\n\n");
                                    m.push_str(&s.notes.join("\n"));
                                }
                                m
                            }
                            None => match op.status {
                                api::OperationStatus::Cancelled => "Import cancelled.".into(),
                                api::OperationStatus::Failure => {
                                    "Import failed — see server logs.".into()
                                }
                                _ => "Import complete.".into(),
                            },
                        };
                        config_banner.set(Some((kind, msg)));
                        // Tell pages subscribing to the refresh tick
                        // (Exports, Mounts, Users, Storage) to re-fetch
                        // so the imported data is visible without a
                        // manual refresh.
                        auth_ctx.bump_refresh();
                        busy_signal.set(false);
                        return;
                    }
                }
                Err(api::ApiError::Unauthorized) => {
                    busy_signal.set(false);
                    auth_ctx.signal_unauthorized();
                    return;
                }
                Err(_) => {
                    // Transient (server restart, network blip). Keep
                    // polling — the op state is on disk in
                    // /var/lib/bananas/operations.json.
                }
            }
            sleep_ms(POLL_MS).await;
        }
    });
}

/// Parse the trailing `--- summary ---\n{json}` block the server
/// appends to a ConfigImport op's output when it finishes. Returns
/// None if the marker isn't present (e.g. interrupted / cancelled
/// before the summary line was written).
fn parse_import_summary(output: &str) -> Option<api::ImportSummary> {
    let marker = "--- summary ---\n";
    let (_head, tail) = output.rsplit_once(marker)?;
    serde_json::from_str(tail.trim()).ok()
}

/// Cooperative sleep on the wasm runtime — `tokio::time::sleep` isn't
/// available without the `time` feature on the wasm32 target.
async fn sleep_ms(ms: i32) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = web_sys::window() {
            let cb = Closure::<dyn FnMut()>::new(move || {
                let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
            });
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                ms,
            );
            cb.forget();
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
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
    let busy: Signal<bool> = use_signal(|| false);
    use_context_provider(|| AuthCtx {
        state: auth,
        me,
        refresh,
        busy,
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
