//! /exports page — table of current rows + modal-based add/edit form +
//! disabled preview of /etc/exports. Each row has pencil + delete
//! buttons; a single ExportFormModal handles both creating new entries
//! and editing existing ones (initial values pre-populated from the row).

#![allow(non_snake_case)]

use dioxus::prelude::*;

use crate::{
    AuthCtx, api,
    api::ApiError,
    browse::Browser,
    components::{ConfirmModal, TextareaWithCopy},
    icons::Icon,
    nfs_help,
    permissions::PermissionsModal,
};

/// What the form modal is currently doing — None means closed; Some
/// carries either an existing row (edit) or a placeholder for create.
#[derive(Clone, PartialEq)]
enum FormMode {
    Create,
    Edit(api::ExportRow),
}

#[component]
pub fn ExportsPage() -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let mut rows: Signal<Vec<api::ExportRow>> = use_signal(Vec::new);
    let mut preview: Signal<String> = use_signal(String::new);
    let mut nfs_status: Signal<String> = use_signal(String::new);
    let mut banner: Signal<Option<(BannerKind, String)>> = use_signal(|| None);
    let mut reload_tick = use_signal(|| 0u32);
    let mut form_mode: Signal<Option<FormMode>> = use_signal(|| None);
    // Path the per-row Permissions modal is editing. None = closed.
    let mut perms_for: Signal<Option<String>> = use_signal(|| None);
    // Index of the export row pending a delete-confirm. None = closed.
    let mut pending_delete: Signal<Option<usize>> = use_signal(|| None);

    use_effect(move || {
        let _ = reload_tick();
        // Re-fetch when the global "load config" flow bumps the refresh
        // counter, so freshly imported exports show up automatically.
        let _ = auth_ctx.refresh.read();
        spawn(async move {
            match api::list_exports().await {
                Ok(list) => {
                    rows.set(list.rows);
                    preview.set(list.preview);
                    nfs_status.set(list.nfs_server_status);
                }
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(err) => banner.set(Some((
                    BannerKind::Err,
                    format!("Loading exports failed: {err}"),
                ))),
            }
        });
    });

    rsx! {
        div { class: "section-header",
            h2 { "NFS exports" }
            NfsServerBadge { status: nfs_status(), has_rows: !rows.read().is_empty() }
            span { class: "spacer" }
            button {
                class: "ghost",
                "data-tip": "Re-fetch from /etc/exports.",
                onclick: move |_| reload_tick.set(reload_tick() + 1),
                Icon { name: "rotate-cw" }
                "Refresh"
            }
            button {
                class: "primary",
                "data-tip": "Add a new NFS export.",
                onclick: move |_| form_mode.set(Some(FormMode::Create)),
                Icon { name: "plus" }
                "Add export"
            }
        }

        if let Some((kind, msg)) = banner() {
            div { class: "banner {kind.css()}",
                pre { "{msg}" }
            }
        }

        if rows.read().is_empty() {
            p { class: "empty", "No exports defined yet — click 'Add export' to create one." }
        } else {
            table { class: "rows",
                thead {
                    tr {
                        th { "Path" } th { "Client" } th { "Options" } th {}
                    }
                }
                tbody {
                    for row in rows.read().iter() {
                        ExportRowView {
                            key: "{row.idx}",
                            row: row.clone(),
                            on_edit: {
                                let r = row.clone();
                                move |_| form_mode.set(Some(FormMode::Edit(r.clone())))
                            },
                            on_perms: {
                                let path = row.path.clone();
                                move |_| perms_for.set(Some(path.clone()))
                            },
                            on_delete: move |idx: usize| pending_delete.set(Some(idx))
                        }
                    }
                }
            }
        }

        h3 { "/etc/exports preview" }
        p { class: "preview-label", "Read-only — column-aligned exactly as written to disk." }
        TextareaWithCopy { value: preview(), id: "exports-preview" }

        if let Some(mode) = form_mode() {
            ExportFormModal {
                mode: mode,
                on_close: move |_| form_mode.set(None),
                on_saved: move |verb: &'static str| {
                    form_mode.set(None);
                    banner.set(Some((BannerKind::Ok, format!("Export {verb}"))));
                    reload_tick.set(reload_tick() + 1);
                },
                on_error: move |msg: String| banner.set(Some((BannerKind::Err, msg))),
                on_unauthorized: move |_| auth_ctx.signal_unauthorized()
            }
        }

        if let Some(path) = perms_for() {
            PermissionsModal {
                path: path,
                on_close: move |_| perms_for.set(None),
                on_saved: move |_| {
                    perms_for.set(None);
                    banner.set(Some((BannerKind::Ok, "Permissions updated".into())));
                }
            }
        }

        if let Some(idx) = pending_delete() {
            ConfirmModal {
                title: "Delete export?".to_string(),
                message: format!("Remove NFS export row #{idx} from /etc/exports."),
                details: "Existing client mounts will be cut on the next exportfs sync.".to_string(),
                confirm_label: "Delete export".to_string(),
                danger: true,
                on_cancel: move |_| pending_delete.set(None),
                on_confirm: move |_| {
                    pending_delete.set(None);
                    spawn(async move {
                        match api::delete_export(idx).await {
                            Ok(()) => {
                                banner.set(Some((BannerKind::Ok, format!("Removed row {idx}"))));
                                reload_tick.set(reload_tick() + 1);
                            }
                            Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                            Err(err) => banner.set(Some((BannerKind::Err, format!("Delete failed: {err}")))),
                        }
                    });
                },
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

#[derive(Props, Clone, PartialEq)]
struct ExportRowViewProps {
    row: api::ExportRow,
    on_edit: EventHandler<()>,
    on_perms: EventHandler<()>,
    on_delete: EventHandler<usize>,
}

#[component]
fn ExportRowView(props: ExportRowViewProps) -> Element {
    let r = &props.row;
    let idx = r.idx;
    rsx! {
        tr {
            td { code { "{r.path}" } }
            td { code { "{r.host}" } }
            td { OptionBadges { opts: r.parsed.clone() } }
            td { class: "row-actions",
                div { class: "actions",
                    button {
                        class: "btn-icon edit",
                        "data-tip": "Edit this export",
                        onclick: move |_| props.on_edit.call(()),
                        Icon { name: "pencil" }
                    }
                    button {
                        class: "btn-icon perms",
                        "data-tip": "Edit owner / group / mode for this directory.",
                        onclick: move |_| props.on_perms.call(()),
                        Icon { name: "lock" }
                    }
                    button {
                        class: "btn-icon delete",
                        "data-tip": "Delete this export",
                        onclick: move |_| props.on_delete.call(idx),
                        Icon { name: "trash-2" }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct OptionBadgesProps {
    opts: api::ExportOpts,
}

#[component]
fn OptionBadges(props: OptionBadgesProps) -> Element {
    let o = &props.opts;
    rsx! {
        div { class: "badges",
            if o.rw {
                span { class: "badge rw", "data-tip": nfs_help::rw(), "rw" }
            } else {
                span { class: "badge ro", "data-tip": nfs_help::ro(), "ro" }
            }
            if o.sync {
                span { class: "badge", "data-tip": nfs_help::sync(), "sync" }
            } else {
                span { class: "badge warn", "data-tip": nfs_help::r#async(), "async" }
            }
            if o.no_subtree_check {
                span { class: "badge", "data-tip": nfs_help::no_subtree_check(), "no_subtree_check" }
            }
            span { class: "badge", "data-tip": nfs_help::squash_label(&o.squash), "{o.squash}" }
            if let Some(uid) = o.anonuid {
                span { class: "badge", "data-tip": nfs_help::anonuid(), "anonuid={uid}" }
            }
            if let Some(gid) = o.anongid {
                span { class: "badge", "data-tip": nfs_help::anongid(), "anongid={gid}" }
            }
            if o.insecure {
                span { class: "badge warn", "data-tip": nfs_help::insecure(), "insecure" }
            }
            for x in o.extra.iter() {
                span { class: "badge", "data-tip": "Unknown option, preserved verbatim from /etc/exports.", "{x}" }
            }
        }
    }
}

/// Allowed characters for an NFS client field. Covers single hosts,
/// CIDRs, wildcards, IPv6 in brackets, and netgroups (`@name`). Spaces
/// and shell metacharacters are excluded so an attacker can't sneak
/// extra entries into /etc/exports through this field.
const CLIENT_PATTERN: &str = r"^[A-Za-z0-9.\-_*?@:/\[\]]+$";

#[derive(Props, Clone, PartialEq)]
struct ExportFormModalProps {
    mode: FormMode,
    on_close: EventHandler<()>,
    on_saved: EventHandler<&'static str>,
    on_error: EventHandler<String>,
    on_unauthorized: EventHandler<()>,
}

#[component]
fn ExportFormModal(props: ExportFormModalProps) -> Element {
    // Pre-fill from the existing row when editing; otherwise sensible
    // defaults that match what most users want.
    let editing_idx: Option<usize> = match &props.mode {
        FormMode::Edit(r) => Some(r.idx),
        FormMode::Create => None,
    };
    let initial: api::ExportRow = match &props.mode {
        FormMode::Edit(r) => r.clone(),
        FormMode::Create => default_export_row(),
    };

    let mut path = use_signal(|| initial.path.clone());
    let mut host = use_signal(|| {
        if initial.host.is_empty() {
            "*".into()
        } else {
            initial.host.clone()
        }
    });
    let mut rw = use_signal(|| initial.parsed.rw);
    let mut sync = use_signal(|| initial.parsed.sync);
    let mut no_subtree_check = use_signal(|| initial.parsed.no_subtree_check);
    let mut squash = use_signal(|| {
        if initial.parsed.squash.is_empty() {
            "all_squash".into()
        } else {
            initial.parsed.squash.clone()
        }
    });
    let mut anonuid = use_signal(|| initial.parsed.anonuid.or(Some(1000)));
    let mut anongid = use_signal(|| initial.parsed.anongid.or(Some(1000)));
    let mut insecure = use_signal(|| initial.parsed.insecure);
    let mut show_browser = use_signal(|| false);
    let mut busy = use_signal(|| false);

    let title = match editing_idx {
        Some(idx) => format!("Edit export — row {idx}"),
        None => "Add a new export".into(),
    };
    let submit_label = if editing_idx.is_some() {
        "Save changes"
    } else {
        "Add export"
    };

    let mut submit = move |_| {
        if busy() {
            return;
        }
        if path().trim().is_empty() {
            props
                .on_error
                .call("Pick a directory via the Browse button".into());
            return;
        }
        if host().trim().is_empty() {
            props.on_error.call("Client is required".into());
            return;
        }
        busy.set(true);
        let body = api::AddExport {
            path: path(),
            host: host(),
            rw: rw(),
            sync: sync(),
            no_subtree_check: no_subtree_check(),
            squash: squash(),
            anonuid: anonuid(),
            anongid: anongid(),
            insecure: insecure(),
        };
        let on_saved = props.on_saved.clone();
        let on_error = props.on_error.clone();
        let on_unauthorized = props.on_unauthorized.clone();
        spawn(async move {
            let result = match editing_idx {
                Some(idx) => api::update_export(idx, &body).await.map(|()| "updated"),
                None => api::add_export(&body).await.map(|()| "added"),
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
                    button {
                        class: "ghost",
                        r#type: "button",
                        onclick: move |_| props.on_close.call(()),
                        "✕"
                    }
                }

                div { class: "modal-body form-modal-body",
                    div { class: "row",
                        label { r#for: "exp-path", class: "hint", "data-tip": nfs_help::path(), "Path" }
                        input {
                            id: "exp-path",
                            r#type: "text",
                            class: "path-display",
                            readonly: true,
                            required: true,
                            placeholder: "Click 'Browse…' to pick a directory",
                            value: "{path()}",
                            onclick: move |_| show_browser.set(true)
                        }
                        button {
                            r#type: "button",
                            "data-tip": "Browse the server filesystem to pick a directory.",
                            onclick: move |_| show_browser.set(true),
                            Icon { name: "folder-open" }
                            "Browse"
                        }
                    }

                    div { class: "row",
                        label { r#for: "exp-host", class: "hint", "data-tip": nfs_help::client(), "Client" }
                        input {
                            id: "exp-host",
                            r#type: "text",
                            placeholder: "*  ·  192.168.1.0/24  ·  *.lan  ·  @netgroup",
                            title: "Hostname, IP/CIDR, wildcard, or @netgroup. No spaces or shell metacharacters.",
                            pattern: CLIENT_PATTERN,
                            required: true,
                            value: "{host()}",
                            oninput: move |e| host.set(e.value())
                        }
                        span {}
                    }

                    fieldset {
                        legend { "Options (hover for details)" }
                        div { class: "opts",
                            label { class: "hint", "data-tip": nfs_help::rw(),
                                input { r#type: "radio", name: "access", checked: rw(), onchange: move |_| rw.set(true) }
                                " rw"
                            }
                            label { class: "hint", "data-tip": nfs_help::ro(),
                                input { r#type: "radio", name: "access", checked: !rw(), onchange: move |_| rw.set(false) }
                                " ro"
                            }
                            label { class: "hint", "data-tip": if sync() { nfs_help::sync() } else { nfs_help::r#async() },
                                input { r#type: "checkbox", checked: sync(), onchange: move |e| sync.set(e.checked()) }
                                " sync"
                            }
                            label { class: "hint", "data-tip": nfs_help::no_subtree_check(),
                                input { r#type: "checkbox", checked: no_subtree_check(),
                                    onchange: move |e| no_subtree_check.set(e.checked()) }
                                " no_subtree_check"
                            }
                            label { class: "hint", "data-tip": if insecure() { nfs_help::insecure() } else { nfs_help::secure() },
                                input { r#type: "checkbox", checked: insecure(), onchange: move |e| insecure.set(e.checked()) }
                                " insecure"
                            }
                            label { class: "hint", "data-tip": nfs_help::squash_field(), "squash:"
                                select {
                                    value: "{squash()}",
                                    "data-tip": nfs_help::squash_label(&squash()),
                                    onchange: move |e| squash.set(e.value()),
                                    option { value: "all_squash", "all_squash" }
                                    option { value: "root_squash", "root_squash" }
                                    option { value: "no_root_squash", "no_root_squash" }
                                }
                            }
                            label { class: "hint", "data-tip": nfs_help::anonuid(), "anonuid:"
                                input {
                                    r#type: "number", min: "0", max: "65535",
                                    value: "{anonuid().map(|n| n.to_string()).unwrap_or_default()}",
                                    oninput: move |e| anonuid.set(e.value().parse().ok())
                                }
                            }
                            label { class: "hint", "data-tip": nfs_help::anongid(), "anongid:"
                                input {
                                    r#type: "number", min: "0", max: "65535",
                                    value: "{anongid().map(|n| n.to_string()).unwrap_or_default()}",
                                    oninput: move |e| anongid.set(e.value().parse().ok())
                                }
                            }
                        }
                    }
                }

                div { class: "modal-footer",
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()), "Cancel" }
                    button { class: "primary", r#type: "submit", disabled: busy(),
                        if busy() { "Saving…" } else { "{submit_label}" }
                    }
                }
            }
        }

        if show_browser() {
            Browser {
                start: path().clone(),
                on_pick: move |p: String| {
                    path.set(p);
                    show_browser.set(false);
                },
                on_close: move |_| show_browser.set(false)
            }
        }
    }
}

fn default_export_row() -> api::ExportRow {
    api::ExportRow {
        idx: 0,
        path: String::new(),
        host: "*".into(),
        options: String::new(),
        parsed: api::ExportOpts {
            rw: true,
            sync: true,
            no_subtree_check: true,
            squash: "all_squash".into(),
            anonuid: Some(1000),
            anongid: Some(1000),
            insecure: false,
            extra: vec![],
        },
    }
}

/// Status badge that sits next to the "NFS exports" heading, mirroring
/// the result of `systemctl is-active nfs-server.service` on the BPI.
/// Surface a yellow warning specifically when there ARE rows but the
/// daemon isn't active — the most common operator-visible failure mode.
#[derive(Props, Clone, PartialEq)]
struct NfsServerBadgeProps {
    status: String,
    has_rows: bool,
}

#[component]
fn NfsServerBadge(props: NfsServerBadgeProps) -> Element {
    let s = props.status.as_str();
    if s.is_empty() {
        return rsx! {};
    }
    let (class, label, tip): (&'static str, &'static str, &'static str) = match s {
        "active" => (
            "badge ok",
            "nfs-server: active",
            "nfs-server.service is running and serving any rows below.",
        ),
        "inactive" if !props.has_rows => (
            "badge",
            "nfs-server: stopped",
            "Idle on purpose — no exports defined. Adding the first row will start nfs-server automatically.",
        ),
        "inactive" => (
            "badge warn",
            "nfs-server: stopped",
            "There are exports below but nfs-server.service isn't running. Save any row to (re)start it.",
        ),
        "failed" => (
            "badge err",
            "nfs-server: failed",
            "nfs-server.service is in the failed state. SSH in and run `journalctl -u nfs-server -b` for details.",
        ),
        "activating" => (
            "badge",
            "nfs-server: starting…",
            "nfs-server.service is in the activating state — refresh in a moment.",
        ),
        "deactivating" => (
            "badge",
            "nfs-server: stopping…",
            "nfs-server.service is in the deactivating state — refresh in a moment.",
        ),
        _ => (
            "badge",
            "nfs-server: unknown",
            "Could not query systemctl is-active. Service state is unknown.",
        ),
    };
    rsx! {
        span { class: "{class}", style: "margin-left: 12px", "data-tip": "{tip}", "{label}" }
    }
}
