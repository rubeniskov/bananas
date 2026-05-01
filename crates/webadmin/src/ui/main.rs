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

    // Override dioxus-web's default WebHistory with MemoryHistory.
    // The host SPA owns URL state via direct `history.replaceState`
    // calls in `canonicalize_url(page)`; we don't want dioxus-web's
    // launch path also rewriting `window.location` based on
    // `Dioxus.toml`'s base_path. Today base_path = "/" so the bug
    // (`base_path + base_path` double-prefix from
    // `WebHistory::route_from_location`'s strip-or-fallback) doesn't
    // fire, but the same launch path bit cloud-ui hard. This keeps
    // them aligned defensively.
    use std::rc::Rc;
    let history: Rc<dyn dioxus::history::History> =
        Rc::new(dioxus::history::MemoryHistory::default());
    dioxus::LaunchBuilder::new()
        .with_cfg(dioxus::web::Config::new().history(history))
        .launch(App);
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

/// Either a host-level page (`Settings`, `Updates`) or a plugin
/// SPA mounted via the MFE flow (any installed manifest's `id`).
/// Plugins are string-identified so the host doesn't need to
/// recompile to add one — `/api/extensions` drives the nav, the
/// hash route, and the active-tab predicate.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Page {
    /// `id` from the plugin's manifest. Default landing slug is
    /// "stats" because that's the conventional first plugin in
    /// the bundled image.
    Plugin(String),
    Settings,
    Updates,
}

impl Page {
    /// URL-hash slug used to make the current page survive a
    /// full-page reload. Hash-based (vs path-based) so the static
    /// SPA fallback at `/` doesn't need server-side routing rules.
    fn slug(&self) -> &str {
        match self {
            Page::Plugin(id) => id.as_str(),
            Page::Settings => "settings",
            Page::Updates => "updates",
        }
    }

    fn from_slug(s: &str) -> Self {
        match s {
            "settings" => Page::Settings,
            "updates" => Page::Updates,
            "" => Page::Plugin("stats".to_string()),
            other => Page::Plugin(other.to_string()),
        }
    }
}

/// Resolve the current Page from `window.location.hash`. Stats is
/// the default landing page — only an explicit `/#<slug>` switches
/// to a different tab, so legacy path-style URLs (`/exports`,
/// `/users`, …) from earlier builds land on Stats and get rewritten
/// by the canonicalize step below.
fn read_page_from_url() -> Page {
    let Some(window) = web_sys::window() else {
        return Page::Plugin("stats".to_string());
    };
    let hash = window.location().hash().unwrap_or_default();
    let slug = hash.trim_start_matches('#').trim_start_matches('/');
    Page::from_slug(slug)
}

/// Replace the current entry with `/#<slug>` so the visible URL
/// stays canonical regardless of how the user got here. Uses
/// `replaceState` instead of `set_hash` so the path component is
/// also normalized and nav-tab clicks don't push a history entry
/// per click.
fn canonicalize_url(page: &Page) {
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
        canonicalize_url(&page.read());
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
    // installed shows no Cloud tab at all. The MFE loader queries each
    // plugin's `/api/<id>/__mfe_entry` for its content-hashed entry
    // script, so we only need id + label here.
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
    let mut activate_plugin = move |id: String| {
        if mfe::is_loaded(&id) {
            mfe_state.write().insert(id, Ok(()));
            return;
        }
        if matches!(mfe_state.read().get(&id), Some(Ok(()))) {
            return;
        }
        spawn(async move {
            let result = mfe::load(&id).await;
            mfe_state.write().insert(id, result);
        });
    };

    // Reload-on-#<plugin> handling: if the operator hits refresh
    // while on /#<plugin> (or pastes the deep link), `page` is
    // already `Plugin(id)` at mount time but no NavTab click ever
    // fires `activate_plugin`. This effect bridges that gap — it
    // runs whenever extensions resolve or the page changes, and is
    // itself idempotent because activate_plugin short-circuits on
    // already-loaded plugins.
    use_effect(move || {
        let id = match &*page.read() {
            Page::Plugin(id) => id.clone(),
            _ => return,
        };
        // Only fire MFE for ids backed by a manifest. Host-rendered
        // fallbacks (stats/exports/storage/users until extracted)
        // would 404 the __mfe_entry handshake.
        if extensions.read().iter().any(|e| e.id == id) {
            activate_plugin(id);
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
                {
                    // Manifest-driven nav: render one NavTab per
                    // installed plugin, sorted by `order` then `id`
                    // (the server already sorted, but we re-sort here
                    // because the SPA-side Extension is just a Vec).
                    // Until the per-feature extractions land
                    // (commits 3-7), the host fakes "stats", "exports",
                    // "storage", "users" entries by hardcoding them
                    // alongside any plugin manifests — those will
                    // disappear from this fallback list as their
                    // crates ship.
                    let mut listed = extensions.read().clone();
                    listed.sort_by(|a, b| a.order.cmp(&b.order).then(a.id.cmp(&b.id)));
                    let host_pages: &[(&str, &str, &str, u32)] = &[
                        ("stats", "Stats", "chart-bar", 10),
                        // exports + storage + users moved out to
                        // plugins — their NavTabs come from the
                        // manifest loop.
                    ];
                    let mut nav_items: Vec<(String, String, String, u32)> = host_pages
                        .iter()
                        .filter(|(id, _, _, _)| {
                            // Drop the host-page fallback when a real
                            // plugin manifest with the same id is
                            // installed (so we don't double-render
                            // once the feature graduates to a plugin).
                            !listed.iter().any(|e| e.id == *id)
                        })
                        .map(|(id, label, icon, order)| {
                            (id.to_string(), label.to_string(), icon.to_string(), *order)
                        })
                        .collect();
                    for ext in &listed {
                        nav_items.push((
                            ext.id.clone(),
                            ext.label.clone().unwrap_or_else(|| ext.id.clone()),
                            ext.icon.clone().unwrap_or_else(|| "box".to_string()),
                            ext.order,
                        ));
                    }
                    nav_items.sort_by(|a, b| a.3.cmp(&b.3).then(a.0.cmp(&b.0)));
                    rsx! {
                        for (id, label, icon, _order) in nav_items {
                            NavTab {
                                label: label.clone(),
                                icon: icon.clone(),
                                active: matches!(&*page.read(), Page::Plugin(p) if p == &id),
                                on_click: {
                                    let id = id.clone();
                                    move |_| {
                                        // Only trigger the MFE handshake
                                        // for ids backed by a real
                                        // manifest; host-rendered
                                        // fallbacks (stats/exports/etc.
                                        // pre-extraction) have no
                                        // /api/<id>/__mfe_entry endpoint.
                                        if extensions.read().iter().any(|e| e.id == id) {
                                            activate_plugin(id.clone());
                                        }
                                        page.set(Page::Plugin(id.clone()));
                                    }
                                },
                            }
                        }
                    }
                }
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
                            // Section 1 — theme picker. Light/dark
                            // mode is the most-used menu item, so it
                            // takes the top slot for thumb reach on
                            // mobile.
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
                            // Section 2 — config + nav. Save / load
                            // config sit next to the nav-actions
                            // (Updates, Settings) because all four
                            // are "configuration" affordances and
                            // bunching them keeps the cognitive
                            // grouping clean.
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
                            button {
                                class: "user-menu-item",
                                role: "menuitem",
                                disabled: auth_ctx.busy.read().clone(),
                                onclick: move |_| {
                                    menu_open.set(false);
                                    page.set(Page::Updates);
                                },
                                icons::Icon { name: "package" }
                                span { "Updates" }
                            }
                            button {
                                class: "user-menu-item",
                                role: "menuitem",
                                disabled: auth_ctx.busy.read().clone(),
                                onclick: move |_| {
                                    menu_open.set(false);
                                    page.set(Page::Settings);
                                },
                                icons::Icon { name: "settings" }
                                span { "Settings" }
                            }
                            div { class: "user-menu-sep" }
                            // Section 3 — destructive / session-end.
                            // Reboot stays first (it's a system-level
                            // action) followed by sign-out, both
                            // visually separated by the divider above
                            // so accidental clicks need the slight
                            // travel.
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

            // The Settings + Updates pages are host-rendered. Plugins
            // (cloud today, others as they extract) own their own
            // wasm bundles and mount via the MFE frame below — for
            // those, this match contributes nothing, leaving the
            // empty `()` branch and letting the MFE frame fill in.
            //
            // During the per-feature extraction (commits 3-7) the
            // explicit branches "stats" / "exports" / "storage" /
            // "users" disappear one at a time as each feature moves
            // out of webadmin into its own plugin daemon.
            match &*page.read() {
                Page::Settings => rsx! { settings::SettingsPage {} },
                Page::Updates => rsx! { updates::UpdatesPage {} },
                Page::Plugin(id) => match id.as_str() {
                    "stats" => rsx! { stats::StatsPage {} },
                    // Empty branch for MFE plugins (cloud, exports,
                    // storage, users today). The mount frame below
                    // renders unconditionally and toggles via CSS;
                    // keeping the runtime mount alive across tab
                    // switches preserves plugin state on flick-back.
                    _ => rsx! {},
                },
            }

            // One MFE mount frame per installed plugin manifest.
            // Each frame stays in the DOM across tab switches —
            // toggled via `display:none` so Dioxus doesn't unmount
            // the plugin's runtime and we don't lose its state on
            // every flick back to a host page. The plugin's
            // `main()` finds its frame by `id="<plugin>-mfe-root"`
            // (see mfe::mount_id) once the host injects its script
            // tag (see crate::mfe::inject).
            {
                let installed: Vec<api::Extension> = extensions.read().clone();
                let active_id = match &*page.read() {
                    Page::Plugin(id) => Some(id.clone()),
                    _ => None,
                };
                rsx! {
                    for ext in installed {
                        {
                            let id = ext.id.clone();
                            let label = ext.label.clone().unwrap_or_else(|| id.clone());
                            let mount_id = mfe::mount_id(&id);
                            let active = active_id.as_deref() == Some(id.as_str());
                            let status = mfe_state.read().get(&id).cloned();
                            let frame_class = if active {
                                "mfe-frame"
                            } else {
                                "mfe-frame hidden"
                            };
                            rsx! {
                                div { class: "{frame_class}",
                                    div {
                                        id: "{mount_id}",
                                        class: "mfe-mount",
                                        match status {
                                            Some(Err(err)) => {
                                                let retry_id = id.clone();
                                                let retry_label = label.clone();
                                                rsx! {
                                                    div { class: "mfe-loading mfe-failed",
                                                        p { class: "mfe-failed-title", "{retry_label} module failed to load." }
                                                        pre { class: "mfe-failed-detail", "{err}" }
                                                        button {
                                                            class: "primary",
                                                            onclick: move |_| {
                                                                mfe_state.write().remove(&retry_id);
                                                                activate_plugin(retry_id.clone());
                                                            },
                                                            "Retry"
                                                        }
                                                    }
                                                }
                                            }
                                            _ => {
                                                let loading_label = label.clone();
                                                rsx! {
                                                    div { class: "mfe-loading",
                                                        components::Spinner { size: 32 }
                                                        span { "Loading {loading_label} module…" }
                                                    }
                                                }
                                            }
                                        }
                                    }
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
                                    "Imported: {} exports, {} mounts, {} users restored ({} skipped).",
                                    s.exports_written,
                                    s.storage_written,
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
    #[props(default)]
    icon: String,
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
            if !props.icon.is_empty() { icons::Icon { name: props.icon.clone() } }
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
