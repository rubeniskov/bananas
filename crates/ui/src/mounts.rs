//! /etc/fstab editor — table with per-row edit + delete buttons,
//! single FstabFormModal handling both create and edit. Same UX pattern
//! as the Exports page.

#![allow(non_snake_case)]

use dioxus::prelude::*;
use web_sys::window;

use crate::{AuthCtx, api, api::ApiError, browse::Browser, icons::Icon, permissions::PermissionsModal};

#[derive(Clone, PartialEq)]
enum FormMode {
    Create,
    Edit(api::FstabRow),
}

#[component]
pub fn MountsSection() -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let mut rows: Signal<Vec<api::FstabRow>> = use_signal(Vec::new);
    let mut preview: Signal<String> = use_signal(String::new);
    let mut banner: Signal<Option<(BannerKind, String)>> = use_signal(|| None);
    let mut tick = use_signal(|| 0u32);
    let mut form_mode: Signal<Option<FormMode>> = use_signal(|| None);
    // Mountpoint the per-row Permissions modal is editing. None = closed.
    let mut perms_for: Signal<Option<String>> = use_signal(|| None);

    use_effect(move || {
        let _ = tick();
        let _ = auth_ctx.refresh.read();
        spawn(async move {
            match api::list_fstab().await {
                Ok(list) => {
                    rows.set(list.rows);
                    preview.set(list.preview);
                }
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => banner.set(Some((BannerKind::Err, format!("Loading fstab failed: {e}")))),
            }
        });
    });

    rsx! {
        div { class: "section-header",
            h2 { "Mount points" }
            span { class: "spacer" }
            button {
                class: "ghost",
                "data-tip": "Re-fetch from /etc/fstab.",
                onclick: move |_| tick.set(tick() + 1),
                Icon { name: "rotate-cw" }
                "Refresh"
            }
            button {
                class: "primary",
                "data-tip": "Add a new entry to /etc/fstab.",
                onclick: move |_| form_mode.set(Some(FormMode::Create)),
                Icon { name: "plus" }
                "Add mount"
            }
        }
        p { class: "preview-label",
            "Edits land in /etc/fstab and trigger a systemd daemon-reload. Existing mounts stay mounted — unmount or reboot to actually drop a removed entry."
        }

        if let Some((kind, msg)) = banner() {
            div { class: "banner {kind.css()}", pre { "{msg}" } }
        }

        if rows.read().is_empty() {
            p { class: "empty", "No fstab entries — click 'Add mount' to create one." }
        } else {
            table { class: "rows",
                thead {
                    tr {
                        th { "Source" }
                        th { "Mountpoint" }
                        th { "Type" }
                        th { "Options" }
                        th { "Dump" }
                        th { "Pass" }
                        th {}
                    }
                }
                tbody {
                    for r in rows.read().iter() {
                        FstabRowView {
                            key: "{r.idx}",
                            row: r.clone(),
                            on_edit: {
                                let row = r.clone();
                                move |_| form_mode.set(Some(FormMode::Edit(row.clone())))
                            },
                            on_perms: {
                                let mp = r.mountpoint.clone();
                                move |_| perms_for.set(Some(mp.clone()))
                            },
                            on_delete: move |idx| {
                                if !confirm("Remove this fstab entry? Existing mount will stay until reboot.") { return; }
                                spawn(async move {
                                    match api::delete_fstab(idx).await {
                                        Ok(()) => {
                                            banner.set(Some((BannerKind::Ok, format!("Removed row {idx}"))));
                                            tick.set(tick() + 1);
                                        }
                                        Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                                        Err(e) => banner.set(Some((BannerKind::Err, format!("Delete failed: {e}")))),
                                    }
                                });
                            }
                        }
                    }
                }
            }
        }

        h3 { "/etc/fstab preview" }
        p { class: "preview-label", "Read-only — column-aligned exactly as written to disk." }
        textarea { readonly: true, disabled: true, "{preview()}" }

        if let Some(mode) = form_mode() {
            FstabFormModal {
                mode: mode,
                on_close: move |_| form_mode.set(None),
                on_saved: move |verb: &'static str| {
                    form_mode.set(None);
                    banner.set(Some((BannerKind::Ok, format!("Mount {verb}"))));
                    tick.set(tick() + 1);
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
    }
}

fn confirm(msg: &str) -> bool {
    window().and_then(|w| w.confirm_with_message(msg).ok()).unwrap_or(false)
}

#[derive(Clone, Copy, PartialEq)]
enum BannerKind { Ok, Err }
impl BannerKind {
    fn css(self) -> &'static str { match self { BannerKind::Ok => "ok", BannerKind::Err => "err" } }
}

#[derive(Props, Clone, PartialEq)]
struct FstabRowViewProps {
    row: api::FstabRow,
    on_edit: EventHandler<()>,
    on_perms: EventHandler<()>,
    on_delete: EventHandler<usize>,
}

#[component]
fn FstabRowView(props: FstabRowViewProps) -> Element {
    let r = &props.row;
    let idx = r.idx;
    let row_class = if r.protected { "row-protected" } else { "" };
    rsx! {
        tr { class: "{row_class}",
            td { class: "source-cell",
                code { "{r.source}" }
                if r.protected {
                    span { class: "badge sys",
                        "data-tip": "System mount managed by the OS — the server refuses to edit or delete this row.",
                        Icon { name: "lock" }
                        " system"
                    }
                }
            }
            td { code { "{r.mountpoint}" } }
            td { code { "{r.fstype}" } }
            td { FstabOptionBadges { opts: r.parsed.clone() } }
            td { code { "{r.dump}" } }
            td { code { "{r.pass}" } }
            td { class: "row-actions",
                if r.protected {
                    button {
                        class: "btn-icon perms",
                        "data-tip": "Edit owner / group / mode for this mountpoint. Fstab row itself is locked.",
                        onclick: move |_| props.on_perms.call(()),
                        Icon { name: "lock" }
                    }
                } else {
                    button {
                        class: "btn-icon edit",
                        "data-tip": "Edit this fstab entry",
                        onclick: move |_| props.on_edit.call(()),
                        Icon { name: "pencil" }
                    }
                    button {
                        class: "btn-icon perms",
                        "data-tip": "Edit owner / group / mode for this mountpoint.",
                        onclick: move |_| props.on_perms.call(()),
                        Icon { name: "lock" }
                    }
                    button {
                        class: "btn-icon delete",
                        "data-tip": "Delete this fstab entry",
                        onclick: move |_| props.on_delete.call(idx),
                        Icon { name: "trash-2" }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct FstabOptionBadgesProps { opts: api::FstabOpts }

#[component]
fn FstabOptionBadges(props: FstabOptionBadgesProps) -> Element {
    let o = &props.opts;
    rsx! {
        div { class: "badges",
            if o.defaults { span { class: "badge", "data-tip": "rw, suid, dev, exec, auto, nouser, async — sane baseline.", "defaults" } }
            if o.ro { span { class: "badge ro", "data-tip": "Mount read-only.", "ro" } }
            if o.noatime { span { class: "badge", "data-tip": "Skip atime updates — easier on the disk.", "noatime" } }
            if o.nofail { span { class: "badge", "data-tip": "Don't fail boot if the device isn't there.", "nofail" } }
            if o.discard { span { class: "badge", "data-tip": "SSD trim on delete — only useful on SSDs.", "discard" } }
            if o.noexec { span { class: "badge warn", "data-tip": "Block execution of binaries on this filesystem.", "noexec" } }
            if o.nosuid { span { class: "badge warn", "data-tip": "Ignore setuid bits on this filesystem.", "nosuid" } }
            if o.nodev { span { class: "badge warn", "data-tip": "Block device-node files on this filesystem.", "nodev" } }
            if let Some(t) = o.device_timeout {
                span { class: "badge", "data-tip": "systemd waits this long for the device before failing.",
                    "x-systemd.device-timeout={t}s" }
            }
            for x in o.extra.iter() {
                span { class: "badge", "data-tip": "Option not exposed as a checkbox; preserved verbatim.", "{x}" }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SourceKind { Label, Uuid, Path, Other }

impl SourceKind {
    fn from_str(s: &str) -> Self {
        match s {
            "label" => Self::Label,
            "uuid" => Self::Uuid,
            "path" => Self::Path,
            _ => Self::Other,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Label => "label",
            Self::Uuid => "uuid",
            Self::Path => "path",
            Self::Other => "other",
        }
    }
    fn placeholder(self) -> &'static str {
        match self {
            Self::Label => "Pick a labeled filesystem",
            Self::Uuid => "Pick a filesystem by UUID",
            Self::Path => "Pick a /dev/ path",
            Self::Other => "tmpfs, hostname:/share, etc.",
        }
    }
}

/// Decompose a fstab source string into (kind, value) so an existing row
/// can pre-populate the form. Mirrors `format_source` going the other way.
fn parse_source(source: &str) -> (SourceKind, String) {
    if let Some(rest) = source.strip_prefix("LABEL=") {
        (SourceKind::Label, rest.to_string())
    } else if let Some(rest) = source.strip_prefix("UUID=") {
        (SourceKind::Uuid, rest.to_string())
    } else if source.starts_with("/dev/") {
        (SourceKind::Path, source.to_string())
    } else {
        (SourceKind::Other, source.to_string())
    }
}

/// Build the canonical fstab source string from the kind + selected value.
fn format_source(kind: SourceKind, value: &str) -> String {
    let v = value.trim();
    match kind {
        SourceKind::Label if !v.is_empty() => format!("LABEL={v}"),
        SourceKind::Uuid if !v.is_empty() => format!("UUID={v}"),
        _ => v.to_string(),
    }
}

/// Flat option list derived from /api/storage for the active source kind.
/// Each entry is (value-to-emit, label-to-display).
fn source_options(report: &api::StorageReport, kind: SourceKind) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for disk in &report.disks {
        for part in &disk.partitions {
            match kind {
                SourceKind::Label => {
                    if let Some(l) = &part.label {
                        out.push((l.clone(), format!("{l} ({})", part.kname)));
                    }
                }
                SourceKind::Uuid => {
                    if let Some(u) = &part.uuid {
                        let pretty = format!(
                            "{u}{}",
                            part.label
                                .as_ref()
                                .map(|l| format!(" — {l}"))
                                .unwrap_or_default()
                        );
                        out.push((u.clone(), pretty));
                    }
                }
                SourceKind::Path => {
                    let path = format!("/dev/{}", part.kname);
                    let extras: Vec<String> = [
                        part.label.clone(),
                        part.fstype.clone(),
                        part.mountpoint.clone(),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_empty())
                    .collect();
                    let pretty = if extras.is_empty() {
                        path.clone()
                    } else {
                        format!("{path} ({})", extras.join(", "))
                    };
                    out.push((path, pretty));
                }
                SourceKind::Other => {}
            }
        }
        if matches!(kind, SourceKind::Path) {
            let path = format!("/dev/{}", disk.kname);
            out.push((path.clone(), format!("{path} (whole disk)")));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

#[derive(Props, Clone, PartialEq)]
struct FstabFormModalProps {
    mode: FormMode,
    on_close: EventHandler<()>,
    on_saved: EventHandler<&'static str>,
    on_error: EventHandler<String>,
    on_unauthorized: EventHandler<()>,
}

#[component]
fn FstabFormModal(props: FstabFormModalProps) -> Element {
    let editing_idx: Option<usize> = match &props.mode {
        FormMode::Edit(r) => Some(r.idx),
        FormMode::Create => None,
    };
    let initial: api::FstabRow = match &props.mode {
        FormMode::Edit(r) => r.clone(),
        FormMode::Create => default_fstab_row(),
    };

    let (init_kind, init_value) = parse_source(&initial.source);
    let init_kind_for_choice = init_kind;

    // For LABEL/UUID/Path we put the raw value into source_choice; for Other
    // we put it into source_custom. Keeping them in separate signals lets
    // the user toggle modes without losing typed text.
    let mut source_kind = use_signal(|| init_kind);
    let mut source_choice = use_signal(|| {
        if matches!(init_kind_for_choice, SourceKind::Other) { String::new() } else { init_value.clone() }
    });
    let mut source_custom = use_signal(|| {
        if matches!(init_kind_for_choice, SourceKind::Other) { init_value } else { String::new() }
    });
    let mut mountpoint = use_signal(|| initial.mountpoint.clone());
    let mut fstype = use_signal(|| initial.fstype.clone());
    let mut defaults = use_signal(|| initial.parsed.defaults);
    let mut noatime = use_signal(|| initial.parsed.noatime);
    let mut nofail = use_signal(|| initial.parsed.nofail);
    let mut ro = use_signal(|| initial.parsed.ro);
    let mut discard = use_signal(|| initial.parsed.discard);
    let mut noexec = use_signal(|| initial.parsed.noexec);
    let mut nosuid = use_signal(|| initial.parsed.nosuid);
    let mut nodev = use_signal(|| initial.parsed.nodev);
    let mut device_timeout = use_signal(|| initial.parsed.device_timeout);
    let mut dump = use_signal(|| initial.dump);
    let mut pass = use_signal(|| initial.pass);
    let mut show_browser = use_signal(|| false);
    let mut busy = use_signal(|| false);

    let storage = use_resource(|| async move { api::fetch_storage().await });

    let computed_source = move || -> String {
        match source_kind() {
            SourceKind::Other => source_custom(),
            kind => format_source(kind, &source_choice()),
        }
    };

    let title = match editing_idx {
        Some(idx) => format!("Edit mount — row {idx}"),
        None => "Add a mount".into(),
    };
    let submit_label = if editing_idx.is_some() { "Save changes" } else { "Add mount" };

    let mut submit = move |_| {
        if busy() { return; }
        let src = computed_source();
        if src.trim().is_empty() {
            props.on_error.call("Source/device is required".into());
            return;
        }
        if !mountpoint().starts_with('/') {
            props.on_error.call("Mountpoint must be an absolute path".into());
            return;
        }
        if fstype().trim().is_empty() {
            props.on_error.call("Filesystem type is required".into());
            return;
        }
        busy.set(true);
        let body = api::AddFstab {
            source: src,
            mountpoint: mountpoint(),
            fstype: fstype(),
            defaults: defaults(),
            noatime: noatime(),
            nofail: nofail(),
            ro: ro(),
            discard: discard(),
            noexec: noexec(),
            nosuid: nosuid(),
            nodev: nodev(),
            device_timeout: device_timeout(),
            dump: dump(),
            pass: pass(),
        };
        let on_saved = props.on_saved.clone();
        let on_error = props.on_error.clone();
        let on_unauthorized = props.on_unauthorized.clone();
        spawn(async move {
            let result = match editing_idx {
                Some(idx) => api::update_fstab(idx, &body).await.map(|()| "updated"),
                None => api::add_fstab(&body).await.map(|()| "added"),
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
                        label { class: "hint",
                            "data-tip": "How to identify the filesystem. LABEL/UUID are stable across kernel renumbering; /dev/ paths are not.",
                            "Source kind"
                        }
                        select {
                            value: source_kind().as_str(),
                            onchange: move |e| {
                                source_kind.set(SourceKind::from_str(&e.value()));
                                source_choice.set(String::new());
                            },
                            option { value: "label", "LABEL=" }
                            option { value: "uuid", "UUID=" }
                            option { value: "path", "/dev/ path" }
                            option { value: "other", "Other (free text)" }
                        }
                        span {}
                    }

                    div { class: "row",
                        label { class: "hint", "data-tip": source_kind().placeholder(), "Source" }
                        {
                            let kind = source_kind();
                            if kind == SourceKind::Other {
                                rsx! {
                                    input {
                                        r#type: "text",
                                        placeholder: "tmpfs  ·  192.168.1.5:/srv/x  ·  none",
                                        pattern: r"^[A-Za-z0-9._=:/@\-]+$",
                                        title: "Free text. No spaces or shell metacharacters.",
                                        required: true,
                                        value: "{source_custom()}",
                                        oninput: move |e| source_custom.set(e.value())
                                    }
                                }
                            } else {
                                match &*storage.read_unchecked() {
                                    None => rsx! { span { class: "muted", "Loading devices…" } },
                                    Some(Err(e)) => rsx! { span { class: "muted", "Could not load devices: {e}" } },
                                    Some(Ok(report)) => {
                                        let opts = source_options(report, kind);
                                        if opts.is_empty() {
                                            rsx! {
                                                span { class: "muted",
                                                    { match kind {
                                                        SourceKind::Label => "No labeled filesystems found. Format a partition with -L <name> first.",
                                                        SourceKind::Uuid => "No filesystems with a UUID found.",
                                                        SourceKind::Path => "No block devices found.",
                                                        SourceKind::Other => "",
                                                    } }
                                                }
                                            }
                                        } else {
                                            rsx! {
                                                select {
                                                    value: "{source_choice()}",
                                                    required: true,
                                                    onchange: move |e| source_choice.set(e.value()),
                                                    option { value: "", disabled: true, selected: source_choice().is_empty(),
                                                        "{kind.placeholder()}" }
                                                    for (val, lbl) in opts.iter() {
                                                        option { value: "{val}", selected: source_choice() == *val, "{lbl}" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        span {}
                    }

                    div { class: "row",
                        label { r#for: "fs-mp", "data-tip": "Absolute path where the filesystem is mounted. Use Browse to pick an existing directory.", class: "hint", "Mountpoint" }
                        input {
                            id: "fs-mp",
                            r#type: "text",
                            class: "path-display",
                            readonly: true,
                            required: true,
                            placeholder: "Click 'Browse…' to pick a directory",
                            title: "Absolute mountpoint",
                            value: "{mountpoint()}",
                            onclick: move |_| show_browser.set(true)
                        }
                        button {
                            r#type: "button",
                            "data-tip": "Pick an existing directory on the server.",
                            onclick: move |_| show_browser.set(true),
                            Icon { name: "folder-open" }
                            "Browse"
                        }
                    }

                    div { class: "row",
                        label { r#for: "fs-type", "data-tip": "Filesystem type. Pick the kernel module name (ext4, vfat, tmpfs, ntfs, …).", class: "hint", "Type" }
                        select {
                            id: "fs-type",
                            value: "{fstype()}",
                            onchange: move |e| fstype.set(e.value()),
                            option { value: "ext4", "ext4" }
                            option { value: "ext3", "ext3" }
                            option { value: "btrfs", "btrfs" }
                            option { value: "xfs", "xfs" }
                            option { value: "vfat", "vfat" }
                            option { value: "exfat", "exfat" }
                            option { value: "ntfs", "ntfs" }
                            option { value: "tmpfs", "tmpfs" }
                            option { value: "nfs", "nfs" }
                            option { value: "auto", "auto" }
                        }
                        span {}
                    }

                    fieldset {
                        legend { "Options (hover for details)" }
                        div { class: "opts",
                            label { class: "hint", "data-tip": "rw, suid, dev, exec, auto, nouser, async — sane baseline.",
                                input { r#type: "checkbox", checked: defaults(), onchange: move |e| defaults.set(e.checked()) }
                                " defaults"
                            }
                            label { class: "hint", "data-tip": "Skip atime updates — easier on the disk.",
                                input { r#type: "checkbox", checked: noatime(), onchange: move |e| noatime.set(e.checked()) }
                                " noatime"
                            }
                            label { class: "hint", "data-tip": "Don't fail boot if the device isn't present.",
                                input { r#type: "checkbox", checked: nofail(), onchange: move |e| nofail.set(e.checked()) }
                                " nofail"
                            }
                            label { class: "hint", "data-tip": "Mount read-only.",
                                input { r#type: "checkbox", checked: ro(), onchange: move |e| ro.set(e.checked()) }
                                " ro"
                            }
                            label { class: "hint", "data-tip": "SSD trim on delete — only useful on SSDs.",
                                input { r#type: "checkbox", checked: discard(), onchange: move |e| discard.set(e.checked()) }
                                " discard"
                            }
                            label { class: "hint", "data-tip": "Block execution of binaries on this filesystem.",
                                input { r#type: "checkbox", checked: noexec(), onchange: move |e| noexec.set(e.checked()) }
                                " noexec"
                            }
                            label { class: "hint", "data-tip": "Ignore setuid bits on this filesystem.",
                                input { r#type: "checkbox", checked: nosuid(), onchange: move |e| nosuid.set(e.checked()) }
                                " nosuid"
                            }
                            label { class: "hint", "data-tip": "Block device-node files on this filesystem.",
                                input { r#type: "checkbox", checked: nodev(), onchange: move |e| nodev.set(e.checked()) }
                                " nodev"
                            }
                            label { class: "hint", "data-tip": "x-systemd.device-timeout — seconds systemd waits for the device before giving up.",
                                "device-timeout (s):"
                                input {
                                    r#type: "number", min: "0", max: "300", style: "width: 80px",
                                    value: "{device_timeout().map(|n| n.to_string()).unwrap_or_default()}",
                                    oninput: move |e| device_timeout.set(e.value().parse().ok())
                                }
                            }
                            label { class: "hint", "data-tip": "fs_freq — used by `dump`. 0 disables. Almost everyone leaves this at 0.",
                                "dump:"
                                input {
                                    r#type: "number", min: "0", max: "9", style: "width: 60px",
                                    value: "{dump()}",
                                    oninput: move |e| dump.set(e.value().parse().unwrap_or(0))
                                }
                            }
                            label { class: "hint", "data-tip": "fs_passno — fsck order at boot. 1 = root, 2 = others, 0 = skip.",
                                "pass:"
                                select {
                                    value: "{pass()}",
                                    onchange: move |e| pass.set(e.value().parse().unwrap_or(2)),
                                    option { value: "0", "0 (skip fsck)" }
                                    option { value: "1", "1 (root)" }
                                    option { value: "2", "2 (others)" }
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
                start: if mountpoint().is_empty() { "/srv".to_string() } else { mountpoint() },
                on_pick: move |p: String| {
                    mountpoint.set(p);
                    show_browser.set(false);
                },
                on_close: move |_| show_browser.set(false)
            }
        }
    }
}

fn default_fstab_row() -> api::FstabRow {
    api::FstabRow {
        idx: 0,
        source: String::new(),
        mountpoint: String::new(),
        fstype: "ext4".into(),
        options: String::new(),
        dump: 0,
        pass: 2,
        parsed: api::FstabOpts {
            defaults: true,
            noatime: true,
            nofail: true,
            ro: false,
            discard: false,
            noexec: false,
            nosuid: false,
            nodev: false,
            device_timeout: Some(10),
            extra: vec![],
        },
        protected: false,
    }
}

