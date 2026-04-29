//! Cloud-sync configuration UI.
//!
//! Three stacked panels:
//!   - **Accounts** — list of providers the operator has configured,
//!     each shown with its name, provider kind, and a "✓ token" /
//!     "no token" indicator. Add via a modal that asks for a name,
//!     provider (dropdown from /api/cloud/providers), and the
//!     `rclone authorize` token blob (paste).
//!   - **Sync entries** — list of (local path, remote path, direction,
//!     schedule, account) rows. Add / edit / delete / run-now.
//!   - **Recent runs** — last ~20 jobs with status badge, started /
//!     finished timestamps and label. Fed by `/api/cloud/runs`.
//!
//! "Run now" enqueues a job server-side and returns immediately with a
//! job_id; the UI then polls `/api/cloud/runs/{id}` every 2 s and
//! updates the toast banner when the run finishes — so the browser tab
//! can be closed without aborting a long sync.

#![allow(non_snake_case)]

use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

use crate::{AuthCtx, api, api::ApiError, browse::Browser, icons::Icon};

#[component]
pub fn CloudPage() -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let mut providers: Signal<Vec<api::CloudProvider>> = use_signal(Vec::new);
    let mut accounts: Signal<Vec<api::CloudAccount>> = use_signal(Vec::new);
    let mut syncs: Signal<Vec<api::CloudSync>> = use_signal(Vec::new);
    let mut runs: Signal<Vec<api::CloudJob>> = use_signal(Vec::new);
    let mut banner: Signal<Option<(BannerKind, String)>> = use_signal(|| None);
    let mut tick = use_signal(|| 0u32);
    let mut runs_tick = use_signal(|| 0u32);
    let mut account_form: Signal<Option<AccountFormMode>> = use_signal(|| None);
    let mut sync_form: Signal<Option<SyncFormMode>> = use_signal(|| None);

    use_effect(move || {
        let _ = tick();
        let _ = auth_ctx.refresh.read();
        spawn(async move {
            match api::list_cloud_providers().await {
                Ok(list) => providers.set(list),
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => banner.set(Some((
                    BannerKind::Err,
                    format!("Loading providers failed: {e}"),
                ))),
            }
        });
        spawn(async move {
            match api::list_cloud_accounts().await {
                Ok(list) => accounts.set(list),
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => banner.set(Some((
                    BannerKind::Err,
                    format!("Loading accounts failed: {e}"),
                ))),
            }
        });
        spawn(async move {
            match api::list_cloud_syncs().await {
                Ok(list) => syncs.set(list),
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => banner.set(Some((
                    BannerKind::Err,
                    format!("Loading sync entries failed: {e}"),
                ))),
            }
        });
    });

    // Refresh the recent-runs list whenever something bumps `runs_tick`
    // (initial load, after Run-now click, or after a polling task notices
    // a job finished).
    use_effect(move || {
        let _ = runs_tick();
        let _ = auth_ctx.refresh.read();
        spawn(async move {
            match api::list_cloud_runs().await {
                Ok(list) => runs.set(list),
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                // Don't surface load-runs errors as banners — they'd
                // overwrite the user's actual sync feedback.
                Err(_) => {}
            }
        });
    });

    rsx! {
        div { class: "section-header",
            h2 { "Cloud accounts" }
            span { class: "spacer" }
            button {
                class: "ghost",
                "data-tip": "Re-fetch accounts + sync entries from the server.",
                onclick: move |_| tick.set(tick() + 1),
                Icon { name: "rotate-cw" }
                "Refresh"
            }
            button {
                class: "primary",
                "data-tip": "Add a new cloud account. You'll need an rclone-authorize token for the chosen provider.",
                onclick: move |_| account_form.set(Some(AccountFormMode::Create)),
                Icon { name: "plus" }
                "Add account"
            }
        }

        if let Some((kind, msg)) = banner() {
            div { class: "banner {kind.css()}", pre { "{msg}" } }
        }

        if accounts.read().is_empty() {
            p { class: "empty", "No cloud accounts yet. Click 'Add account' to connect one." }
        } else {
            table { class: "rows",
                thead {
                    tr { th { "Name" } th { "Provider" } th { "Token" } th {} }
                }
                tbody {
                    for a in accounts.read().iter() {
                        AccountRow {
                            key: "{a.name}",
                            account: a.clone(),
                            on_edit: {
                                let entry = a.clone();
                                move |_| account_form.set(Some(AccountFormMode::Edit(entry.clone())))
                            },
                            on_delete: {
                                let name = a.name.clone();
                                move |_| {
                                    let nm = name.clone();
                                    spawn(async move {
                                        match api::delete_cloud_account(&nm).await {
                                            Ok(()) => {
                                                banner.set(Some((BannerKind::Ok, format!("Removed account {nm}"))));
                                                tick.set(tick() + 1);
                                            }
                                            Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                                            Err(e) => banner.set(Some((BannerKind::Err, e.to_string()))),
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        div { class: "section-header", style: "margin-top: 1.6em",
            h2 { "Sync entries" }
            span { class: "spacer" }
            button {
                class: "primary",
                "data-tip": "Add a directory to sync to / from a cloud account.",
                disabled: accounts.read().is_empty(),
                onclick: move |_| sync_form.set(Some(SyncFormMode::Create)),
                Icon { name: "plus" }
                "Add sync entry"
            }
        }
        if accounts.read().is_empty() {
            p { class: "preview-label", "Add at least one account before defining sync entries." }
        } else if syncs.read().is_empty() {
            p { class: "empty", "No sync entries yet." }
        } else {
            table { class: "rows",
                thead {
                    tr {
                        th { "Account" }
                        th { "Local" }
                        th { "Remote" }
                        th { "Direction" }
                        th { "Schedule" }
                        th {}
                    }
                }
                tbody {
                    for s in syncs.read().iter() {
                        SyncRow {
                            key: "{s.idx}",
                            sync: s.clone(),
                            on_edit: {
                                let entry = s.clone();
                                move |_| sync_form.set(Some(SyncFormMode::Edit(entry.clone())))
                            },
                            on_run: {
                                let idx = s.idx;
                                move |_| {
                                    banner.set(Some((BannerKind::Ok, format!("Queueing sync row {idx}…"))));
                                    spawn(async move {
                                        let job_id = match api::run_cloud_sync(idx).await {
                                            Ok(id) => id,
                                            Err(ApiError::Unauthorized) => {
                                                auth_ctx.signal_unauthorized();
                                                return;
                                            }
                                            Err(e) => {
                                                banner.set(Some((BannerKind::Err, e.to_string())));
                                                return;
                                            }
                                        };
                                        banner.set(Some((BannerKind::Ok, format!("Sync running (job #{job_id}). You can leave this page; the run continues on the NAS."))));
                                        runs_tick.set(runs_tick() + 1);
                                        // Poll every 2s until the job leaves the Running state.
                                        loop {
                                            TimeoutFuture::new(2000).await;
                                            match api::get_cloud_run(job_id).await {
                                                Ok(job) => match job.status {
                                                    api::CloudJobStatus::Running => continue,
                                                    api::CloudJobStatus::Success => {
                                                        let tail = output_tail(&job.output);
                                                        banner.set(Some((BannerKind::Ok, format!("Sync #{job_id} complete. {tail}"))));
                                                        runs_tick.set(runs_tick() + 1);
                                                        break;
                                                    }
                                                    api::CloudJobStatus::Failure => {
                                                        let tail = output_tail(&job.output);
                                                        banner.set(Some((BannerKind::Err, format!("Sync #{job_id} failed. {tail}"))));
                                                        runs_tick.set(runs_tick() + 1);
                                                        break;
                                                    }
                                                },
                                                Err(ApiError::Unauthorized) => {
                                                    auth_ctx.signal_unauthorized();
                                                    break;
                                                }
                                                Err(_) => {
                                                    // Transient network burp — keep polling.
                                                    continue;
                                                }
                                            }
                                        }
                                    });
                                }
                            },
                            on_delete: {
                                let idx = s.idx;
                                move |_| {
                                    spawn(async move {
                                        match api::delete_cloud_sync(idx).await {
                                            Ok(()) => {
                                                banner.set(Some((BannerKind::Ok, format!("Removed sync {idx}"))));
                                                tick.set(tick() + 1);
                                            }
                                            Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                                            Err(e) => banner.set(Some((BannerKind::Err, e.to_string()))),
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        div { class: "section-header", style: "margin-top: 1.6em",
            h2 { "Recent runs" }
            span { class: "spacer" }
            button {
                class: "ghost",
                "data-tip": "Re-fetch the recent-runs list.",
                onclick: move |_| runs_tick.set(runs_tick() + 1),
                Icon { name: "rotate-cw" }
                "Refresh"
            }
        }
        if runs.read().is_empty() {
            p { class: "empty", "No runs yet. Trigger one with the ↻ button on a sync entry, or wait for a scheduled run to fire." }
        } else {
            table { class: "rows",
                thead {
                    tr {
                        th { "Job" }
                        th { "Status" }
                        th { "Started" }
                        th { "Finished" }
                        th { "Label" }
                    }
                }
                tbody {
                    for j in runs.read().iter().take(20) {
                        RunRow {
                            key: "{j.id}",
                            job: j.clone(),
                            on_cancel: move |idx: usize| {
                                spawn(async move {
                                    match api::cancel_cloud_sync(idx).await {
                                        Ok(()) => {
                                            banner.set(Some((BannerKind::Ok, format!("Cancel signaled to sync {idx}"))));
                                            runs_tick.set(runs_tick() + 1);
                                        }
                                        Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                                        Err(e) => banner.set(Some((BannerKind::Err, e.to_string()))),
                                    }
                                });
                            }
                        }
                    }
                }
            }
        }

        if let Some(mode) = account_form() {
            AccountFormModal {
                mode: mode,
                providers: providers.read().clone(),
                on_close: move |_| account_form.set(None),
                on_saved: move |verb: &'static str| {
                    account_form.set(None);
                    banner.set(Some((BannerKind::Ok, format!("Account {verb}."))));
                    tick.set(tick() + 1);
                },
                on_error: move |msg: String| banner.set(Some((BannerKind::Err, msg))),
                on_unauthorized: move |_| auth_ctx.signal_unauthorized()
            }
        }

        if let Some(mode) = sync_form() {
            SyncFormModal {
                mode: mode,
                accounts: accounts.read().clone(),
                on_close: move |_| sync_form.set(None),
                on_saved: move |verb: &'static str| {
                    sync_form.set(None);
                    banner.set(Some((BannerKind::Ok, format!("Sync entry {verb}"))));
                    tick.set(tick() + 1);
                },
                on_error: move |msg: String| banner.set(Some((BannerKind::Err, msg))),
                on_unauthorized: move |_| auth_ctx.signal_unauthorized()
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
            Self::Ok => "ok",
            Self::Err => "err",
        }
    }
}

// ---------------------------------------------------------------- Account row

#[derive(Props, Clone, PartialEq)]
struct AccountRowProps {
    account: api::CloudAccount,
    on_edit: EventHandler<()>,
    on_delete: EventHandler<()>,
}

#[component]
fn AccountRow(props: AccountRowProps) -> Element {
    let a = &props.account;
    let token_class: &'static str = if a.token_present {
        "badge ok"
    } else {
        "badge warn"
    };
    let token_text: &'static str = if a.token_present {
        "✓ token set"
    } else {
        "no token"
    };
    let pretty_provider = provider_pretty(&a.provider);
    rsx! {
        tr {
            td { code { "{a.name}" } }
            td {
                div { class: "provider-cell",
                    ProviderBadge { provider: a.provider.clone() }
                    span { class: "provider-label", "{pretty_provider}" }
                }
            }
            td { span { class: "{token_class}", "{token_text}" } }
            td { class: "row-actions",
                button {
                    class: "btn-icon edit",
                    "data-tip": "Edit this account (rotate token, change provider).",
                    onclick: move |_| props.on_edit.call(()),
                    Icon { name: "pencil" }
                }
                button {
                    class: "btn-icon delete",
                    "data-tip": "Remove this account (and any sync entries pointing at it).",
                    onclick: move |_| {
                        if web_sys_confirm("Remove this account? Any sync entries using it will also be removed.") {
                            props.on_delete.call(());
                        }
                    },
                    Icon { name: "trash-2" }
                }
            }
        }
    }
}

fn web_sys_confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

// ---------------------------------------------------------------- Sync row

#[derive(Props, Clone, PartialEq)]
struct SyncRowProps {
    sync: api::CloudSync,
    on_edit: EventHandler<()>,
    on_run: EventHandler<()>,
    on_delete: EventHandler<()>,
}

#[component]
fn SyncRow(props: SyncRowProps) -> Element {
    let s = &props.sync;
    let direction_arrow: &'static str = match s.direction.as_str() {
        "pull" => "←",
        "bidirectional" => "↔",
        _ => "→",
    };
    rsx! {
        tr {
            td { code { "{s.account}" } }
            td { code { "{s.local_path}" } }
            td { code { "{s.remote_path}" } }
            td { span { class: "muted", "{direction_arrow} {s.direction}" } }
            td { code { "{s.schedule}" } }
            td { class: "row-actions",
                button {
                    class: "btn-icon ok",
                    "data-tip": "Run this sync now (manual trigger).",
                    onclick: move |_| props.on_run.call(()),
                    Icon { name: "rotate-cw" }
                }
                button {
                    class: "btn-icon edit",
                    "data-tip": "Edit this sync entry",
                    onclick: move |_| props.on_edit.call(()),
                    Icon { name: "pencil" }
                }
                button {
                    class: "btn-icon delete",
                    "data-tip": "Remove this sync entry",
                    onclick: move |_| {
                        if web_sys_confirm("Remove this sync entry?") {
                            props.on_delete.call(());
                        }
                    },
                    Icon { name: "trash-2" }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- Run row

#[derive(Props, Clone, PartialEq)]
struct RunRowProps {
    job: api::CloudJob,
    on_cancel: EventHandler<usize>,
}

#[component]
fn RunRow(props: RunRowProps) -> Element {
    let j = &props.job;
    let status_class: String = format!("badge {}", j.status.css());
    let started = format_unix_short(j.started_unix);
    let finished = j
        .finished_unix
        .map(format_unix_short)
        .unwrap_or_else(|| "—".into());
    let label = if j.label.is_empty() {
        format!("sync row {}", j.sync_idx)
    } else {
        j.label.clone()
    };
    let is_running = j.status == api::CloudJobStatus::Running;
    let sync_idx = j.sync_idx;
    let progress = j.progress;
    rsx! {
        tr {
            td { code { "#{j.id}" } }
            td { span { class: "{status_class}", "{j.status.label()}" } }
            td { code { "{started}" } }
            // Finished cell doubles as the live-progress slot while the
            // job is still running — no finish time yet, so the
            // circular bar lives there. After the run resolves, the
            // cell flips back to the actual finish timestamp.
            td {
                if is_running {
                    CircularProgress { percent: progress }
                } else {
                    code { "{finished}" }
                }
            }
            // Label cell sits on the right and holds the cancel button
            // (only while running) next to the label text — the
            // operator's hand is already heading right when scanning a
            // running row, so the cancel target is closest there.
            td {
                div { class: "label-cell",
                    span { class: "muted label-text", "{label}" }
                    if is_running {
                        button {
                            class: "btn-icon delete",
                            "data-tip": "Cancel this in-flight sync (SIGTERM to rclone).",
                            onclick: move |_| props.on_cancel.call(sync_idx),
                            Icon { name: "x" }
                        }
                    }
                }
            }
        }
    }
}

/// Circular progress indicator. Determinate when `percent` is `Some`
/// (renders an SVG arc filling 0..=100% of the circumference); shows
/// an animated indeterminate spinner otherwise. Sized to fit inline
/// with a status badge — see the matching CSS classes
/// `.circ-progress` / `.circ-progress.spinning` in main.scss.
#[derive(Props, Clone, PartialEq)]
struct CircularProgressProps {
    percent: Option<u32>,
}

#[component]
fn CircularProgress(props: CircularProgressProps) -> Element {
    // 18 px radius / 56.5 circumference. The dasharray = full
    // circumference, dashoffset = (1 - p/100) * circumference yields
    // the standard "stroke fills clockwise" effect.
    const RADIUS: f32 = 8.0;
    let circumference: f32 = 2.0 * std::f32::consts::PI * RADIUS;
    match props.percent {
        Some(p) => {
            let p = p.min(100) as f32;
            let offset = circumference * (1.0 - p / 100.0);
            let label = format!("{}%", p as u32);
            rsx! {
                svg {
                    class: "circ-progress",
                    width: "20",
                    height: "20",
                    view_box: "0 0 20 20",
                    role: "img",
                    "aria-label": "{label}",
                    circle {
                        class: "track",
                        cx: "10", cy: "10", r: "{RADIUS}",
                        fill: "none",
                    }
                    circle {
                        class: "fill",
                        cx: "10", cy: "10", r: "{RADIUS}",
                        fill: "none",
                        stroke_dasharray: "{circumference}",
                        stroke_dashoffset: "{offset}",
                        // Rotate -90deg so the arc starts at 12 o'clock.
                        transform: "rotate(-90 10 10)",
                    }
                }
            }
        }
        None => rsx! {
            svg {
                class: "circ-progress spinning",
                width: "20",
                height: "20",
                view_box: "0 0 20 20",
                role: "img",
                "aria-label": "loading",
                circle {
                    class: "track",
                    cx: "10", cy: "10", r: "{RADIUS}",
                    fill: "none",
                }
                circle {
                    class: "fill",
                    cx: "10", cy: "10", r: "{RADIUS}",
                    fill: "none",
                    // Quarter-arc that animates around (CSS rotates).
                    stroke_dasharray: "{circumference / 4.0} {circumference}",
                }
            }
        },
    }
}

/// Last non-empty line of the job's combined output, trimmed to ~120
/// chars. Used in the toast banner so a successful run shows the
/// "Transferred:" summary, and a failure shows the rclone error.
fn output_tail(output: &str) -> String {
    let line = output
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    if line.len() > 120 {
        format!("{}…", &line[..120])
    } else {
        line.to_string()
    }
}

/// Render a unix timestamp as HH:MM:SS in the browser's local zone.
/// JavaScript's `Date` does the heavy lifting; we just stringify a
/// Date object via `toLocaleTimeString`.
fn format_unix_short(ts: i64) -> String {
    if ts <= 0 {
        return "—".into();
    }
    // JS Date takes milliseconds. Use a minimal locale string so the
    // table doesn't blow out width on long timezones.
    let date = js_sys::Date::new(&((ts * 1000) as f64).into());
    date.to_locale_time_string("en-GB")
        .as_string()
        .unwrap_or_default()
}

// rsx! parses `{...}` in string literals as format args; we use a const
// instead of an inline literal because the JSON example placeholder
// contains both braces and escaped quotes that the macro can't handle.
const TOKEN_PLACEHOLDER: &str = "{\"access_token\":\"…\",\"refresh_token\":\"…\",…}";

// ---------------------------------------------------------------- Account form modal

/// Drives the AccountFormModal. `Create` ⇒ all fields editable; `Edit`
/// pre-fills name+provider, locks the name (it's the rclone remote
/// identifier — renaming would orphan every sync entry pointing at it),
/// and treats an empty token as "leave the existing token alone".
#[derive(Clone, PartialEq)]
enum AccountFormMode {
    Create,
    Edit(api::CloudAccount),
}

#[derive(Props, Clone, PartialEq)]
struct AccountFormModalProps {
    mode: AccountFormMode,
    providers: Vec<api::CloudProvider>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<&'static str>,
    on_error: EventHandler<String>,
    on_unauthorized: EventHandler<()>,
}

#[component]
fn AccountFormModal(props: AccountFormModalProps) -> Element {
    let editing: Option<api::CloudAccount> = match &props.mode {
        AccountFormMode::Edit(a) => Some(a.clone()),
        AccountFormMode::Create => None,
    };
    let initial_provider: String = match &editing {
        Some(a) => a.provider.clone(),
        None => props
            .providers
            .first()
            .map(|p| p.key.clone())
            .unwrap_or_default(),
    };
    let initial_name: String = editing.as_ref().map(|a| a.name.clone()).unwrap_or_default();

    let mut name = use_signal(|| initial_name.clone());
    let mut provider = use_signal(|| initial_provider);
    let mut token = use_signal(String::new);
    let mut busy = use_signal(|| false);

    let is_edit = editing.is_some();
    let title: &'static str = if is_edit {
        "Edit cloud account"
    } else {
        "Add cloud account"
    };
    let submit_label: &'static str = if is_edit {
        "Save changes"
    } else {
        "Add account"
    };
    let busy_label: &'static str = if is_edit { "Saving…" } else { "Adding…" };

    let editing_for_submit = editing.clone();
    let mut submit = move |_| {
        if busy() {
            return;
        }
        let on_saved = props.on_saved.clone();
        let on_error = props.on_error.clone();
        let on_unauthorized = props.on_unauthorized.clone();
        match editing_for_submit.clone() {
            None => {
                if name().trim().is_empty() {
                    props.on_error.call("Name is required.".into());
                    return;
                }
                busy.set(true);
                let body = api::AddCloudAccount {
                    name: name(),
                    provider: provider(),
                    token: token(),
                };
                spawn(async move {
                    let r = api::add_cloud_account(&body).await;
                    busy.set(false);
                    match r {
                        Ok(()) => on_saved.call("added"),
                        Err(ApiError::Unauthorized) => on_unauthorized.call(()),
                        Err(e) => on_error.call(e.to_string()),
                    }
                });
            }
            Some(a) => {
                busy.set(true);
                let body = api::UpdateCloudAccount {
                    provider: provider(),
                    token: token(),
                };
                let nm = a.name.clone();
                spawn(async move {
                    let r = api::update_cloud_account(&nm, &body).await;
                    busy.set(false);
                    match r {
                        Ok(()) => on_saved.call("updated"),
                        Err(ApiError::Unauthorized) => on_unauthorized.call(()),
                        Err(e) => on_error.call(e.to_string()),
                    }
                });
            }
        }
    };

    let name_locked = is_edit;
    let token_hint: &'static str = if is_edit {
        "Paste a fresh rclone-authorize blob to rotate the credential, or leave empty to keep the current token."
    } else {
        "rclone opens an OAuth flow and prints a JSON blob — paste it here. Leave empty to add the account now and connect later."
    };

    rsx! {
        div { class: "modal-overlay", onclick: move |_| props.on_close.call(()),
            form {
                class: "modal user-modal",
                onclick: move |e| e.stop_propagation(),
                onsubmit: move |e| { e.prevent_default(); submit(()); },

                div { class: "modal-header",
                    h3 { "{title}" }
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()),
                        Icon { name: "x" }
                    }
                }

                div { class: "modal-body user-form",
                    label { r#for: "cloud-name", "Name" }
                    input {
                        id: "cloud-name",
                        r#type: "text",
                        autocomplete: "off",
                        autofocus: !name_locked,
                        required: true,
                        readonly: name_locked,
                        disabled: name_locked,
                        pattern: "[A-Za-z0-9_-]+",
                        title: "Lowercase / digits / underscore / hyphen, 1–32 chars.",
                        value: "{name()}",
                        oninput: move |e| name.set(e.value())
                    }
                    p { class: "preview-label",
                        if name_locked {
                            "The name is the rclone remote identifier — changing it would orphan every sync entry. Delete and re-add to rename."
                        } else {
                            "Used as the rclone remote name. Pick something short like ‘personal’ or ‘work-drive’."
                        }
                    }

                    label { r#for: "cloud-provider", "Provider" }
                    select {
                        id: "cloud-provider",
                        value: "{provider()}",
                        onchange: move |e| provider.set(e.value()),
                        for p in props.providers.iter() {
                            option { value: "{p.key}", "{p.label}" }
                        }
                    }

                    label { r#for: "cloud-token", "Token (rclone-authorize JSON)" }
                    textarea {
                        id: "cloud-token",
                        spellcheck: false,
                        autofocus: name_locked,
                        style: "min-height: 120px",
                        placeholder: TOKEN_PLACEHOLDER,
                        value: "{token()}",
                        oninput: move |e| token.set(e.value())
                    }
                    p { class: "preview-label",
                        "On a machine with a browser, run: "
                        code { "rclone authorize \"{provider()}\"" }
                        ". {token_hint}"
                    }
                }

                div { class: "modal-footer",
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()),
                        "Cancel"
                    }
                    button { class: "primary", r#type: "submit", disabled: busy(),
                        if busy() { "{busy_label}" } else { "{submit_label}" }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- Sync form modal

#[derive(Clone, PartialEq)]
enum SyncFormMode {
    Create,
    Edit(api::CloudSync),
}

#[derive(Props, Clone, PartialEq)]
struct SyncFormModalProps {
    mode: SyncFormMode,
    accounts: Vec<api::CloudAccount>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<&'static str>,
    on_error: EventHandler<String>,
    on_unauthorized: EventHandler<()>,
}

#[component]
fn SyncFormModal(props: SyncFormModalProps) -> Element {
    let editing_idx: Option<usize> = match &props.mode {
        SyncFormMode::Edit(s) => Some(s.idx),
        SyncFormMode::Create => None,
    };
    let initial: api::CloudSync = match &props.mode {
        SyncFormMode::Edit(s) => s.clone(),
        SyncFormMode::Create => api::CloudSync {
            idx: 0,
            account: props
                .accounts
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default(),
            local_path: String::new(),
            remote_path: String::new(),
            direction: "push".into(),
            schedule: "manual".into(),
        },
    };

    let mut account = use_signal(|| initial.account.clone());
    let mut local_path = use_signal(|| initial.local_path.clone());
    let mut remote_path = use_signal(|| initial.remote_path.clone());
    let mut direction = use_signal(|| initial.direction.clone());
    let mut schedule = use_signal(|| initial.schedule.clone());
    let mut show_browser = use_signal(|| false);
    let mut busy = use_signal(|| false);

    let title: String = match editing_idx {
        Some(idx) => format!("Edit sync entry — row {idx}"),
        None => "Add sync entry".into(),
    };
    let submit_label: &'static str = if editing_idx.is_some() {
        "Save changes"
    } else {
        "Add sync entry"
    };

    let mut submit = move |_| {
        if busy() {
            return;
        }
        if !local_path().starts_with('/') {
            props.on_error.call("Local path must be absolute.".into());
            return;
        }
        if remote_path().trim().is_empty() {
            props.on_error.call("Remote path is required.".into());
            return;
        }
        busy.set(true);
        let body = api::AddCloudSync {
            account: account(),
            local_path: local_path(),
            remote_path: remote_path(),
            direction: direction(),
            schedule: schedule(),
        };
        let on_saved = props.on_saved.clone();
        let on_error = props.on_error.clone();
        let on_unauthorized = props.on_unauthorized.clone();
        spawn(async move {
            let result = match editing_idx {
                Some(idx) => api::update_cloud_sync(idx, &body).await.map(|()| "updated"),
                None => api::add_cloud_sync(&body).await.map(|()| "added"),
            };
            busy.set(false);
            match result {
                Ok(verb) => on_saved.call(verb),
                Err(ApiError::Unauthorized) => on_unauthorized.call(()),
                Err(e) => on_error.call(e.to_string()),
            }
        });
    };

    rsx! {
        div { class: "modal-overlay", onclick: move |_| props.on_close.call(()),
            form {
                class: "modal form-modal",
                onclick: move |e| e.stop_propagation(),
                onsubmit: move |e| { e.prevent_default(); submit(()); },

                div { class: "modal-header",
                    h3 { "{title}" }
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()),
                        Icon { name: "x" }
                    }
                }

                div { class: "modal-body form-modal-body",
                    div { class: "row",
                        label { class: "hint", "data-tip": "Which connected cloud account to sync against.",
                            "Account" }
                        select {
                            value: "{account()}",
                            onchange: move |e| account.set(e.value()),
                            for a in props.accounts.iter() {
                                option { value: "{a.name}", "{a.name} ({a.provider})" }
                            }
                        }
                        span {}
                    }
                    div { class: "row",
                        label { class: "hint", "data-tip": "Absolute path on this NAS. Click to pick via the directory browser.",
                            "Local path" }
                        input {
                            r#type: "text",
                            class: "path-display",
                            readonly: true,
                            required: true,
                            placeholder: "Click 'Browse…' to pick a directory",
                            value: "{local_path()}",
                            onclick: move |_| show_browser.set(true)
                        }
                        button {
                            r#type: "button",
                            "data-tip": "Pick a directory on the server.",
                            onclick: move |_| show_browser.set(true),
                            Icon { name: "folder-open" }
                            "Browse"
                        }
                    }
                    div { class: "row",
                        label { class: "hint", "data-tip": "Path on the cloud provider. Slashes separate folders.",
                            "Remote path" }
                        input {
                            r#type: "text",
                            placeholder: "BanaNAS-backup/photos",
                            required: true,
                            value: "{remote_path()}",
                            oninput: move |e| remote_path.set(e.value())
                        }
                        span {}
                    }
                    div { class: "row",
                        label { class: "hint", "data-tip": "Push = local to remote. Pull = remote to local. Bidirectional = both, with conflict resolution by mtime.",
                            "Direction" }
                        select {
                            value: "{direction()}",
                            onchange: move |e| direction.set(e.value()),
                            option { value: "push", "push (local → remote)" }
                            option { value: "pull", "pull (remote → local)" }
                            option { value: "bidirectional", "bidirectional" }
                        }
                        span {}
                    }
                    div { class: "row",
                        label { class: "hint", "data-tip": "‘manual’ = run-on-demand only. Or a 5-field cron string like '0 2 * * *' (2 AM daily).",
                            "Schedule" }
                        input {
                            r#type: "text",
                            placeholder: "manual",
                            value: "{schedule()}",
                            oninput: move |e| schedule.set(e.value())
                        }
                        span {}
                    }
                }

                div { class: "modal-footer",
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()),
                        "Cancel"
                    }
                    button { class: "primary", r#type: "submit", disabled: busy(),
                        if busy() { "Saving…" } else { "{submit_label}" }
                    }
                }
            }
        }

        if show_browser() {
            Browser {
                start: if local_path().is_empty() { "/srv".to_string() } else { local_path() },
                on_pick: move |p: String| {
                    local_path.set(p);
                    show_browser.set(false);
                },
                on_close: move |_| show_browser.set(false)
            }
        }
    }
}

/// Compact circular badge identifying the provider — colored disc with
/// the provider's first letter. Picked over inline brand SVGs because
/// keeping per-vendor logo paths up-to-date is its own tax, and a
/// colored letter scans at table-row size just as well.
#[derive(Props, Clone, PartialEq)]
struct ProviderBadgeProps {
    provider: String,
}

#[component]
fn ProviderBadge(props: ProviderBadgeProps) -> Element {
    let key = props.provider.as_str();
    let (initial, color, fg) = match key {
        "drive" => ("G", "#4285F4", "#fff"),
        "dropbox" => ("D", "#0061FF", "#fff"),
        "onedrive" => ("O", "#0078D4", "#fff"),
        "s3" => ("S", "#FF9900", "#1f2937"),
        "webdav" => ("W", "#5b6770", "#fff"),
        "ftp" => ("F", "#1a7f37", "#fff"),
        _ => ("?", "#9ca3af", "#fff"),
    };
    let style = format!("background:{color};color:{fg};",);
    rsx! {
        span {
            class: "provider-badge",
            style: "{style}",
            "aria-label": "{key}",
            "{initial}"
        }
    }
}

/// Human-readable label keyed off the rclone backend name, matching
/// the dropdown labels in the Add-account modal.
fn provider_pretty(key: &str) -> &'static str {
    match key {
        "drive" => "Google Drive",
        "dropbox" => "Dropbox",
        "onedrive" => "OneDrive",
        "s3" => "Amazon S3",
        "webdav" => "WebDAV",
        "ftp" => "FTP / FTPS",
        _ => "Unknown",
    }
}
