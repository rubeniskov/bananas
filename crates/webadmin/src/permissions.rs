//! Permissions editor modal — opened from the directory browser to
//! chown/chmod a path through bananas-helper. Server-side allowlist
//! restricts to /srv, /mnt, /media, /home, /opt, so an operator can't
//! accidentally chmod /etc.

#![allow(non_snake_case)]

use dioxus::prelude::*;

use crate::{AuthCtx, api, api::ApiError, icons::Icon};

const MODE_PATTERN: &str = "[0-7][0-7][0-7][0-7]?";

#[derive(Props, Clone, PartialEq)]
pub struct PermissionsModalProps {
    pub path: String,
    pub on_close: EventHandler<()>,
    pub on_saved: EventHandler<()>,
}

#[component]
pub fn PermissionsModal(props: PermissionsModalProps) -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let path_for_load = props.path.clone();
    let info = use_resource({
        let p = path_for_load.clone();
        move || {
            let p = p.clone();
            async move { api::fetch_perms(&p).await }
        }
    });

    let mut user_or_uid = use_signal(String::new);
    let mut group_or_gid = use_signal(String::new);
    let mut mode = use_signal(String::new);
    let mut recursive = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut busy = use_signal(|| false);
    let mut hydrated = use_signal(|| false);

    // First-load hydrate: pull values from /api/permissions once.
    use_effect(move || {
        if hydrated() {
            return;
        }
        if let Some(Ok(p)) = info.read_unchecked().as_ref() {
            user_or_uid.set(if p.user.is_empty() {
                p.uid.to_string()
            } else {
                p.user.clone()
            });
            group_or_gid.set(if p.group.is_empty() {
                p.gid.to_string()
            } else {
                p.group.clone()
            });
            mode.set(p.mode.clone());
            hydrated.set(true);
        }
    });

    let path_for_submit = props.path.clone();
    let mut submit = move |_| {
        if busy() {
            return;
        }
        error.set(None);
        // Parse owner: accept either a numeric UID or a username — the
        // server's helper currently only takes numeric, so we do a
        // best-effort UID lookup against the loaded info if a name is
        // supplied. Falls back to "must be numeric" hint if we can't
        // parse and the name doesn't match the loaded user.
        let info_now = info
            .read_unchecked()
            .as_ref()
            .and_then(|r| r.as_ref().ok().cloned());
        let uid: Option<u32> = match user_or_uid().trim().parse::<u32>() {
            Ok(n) => Some(n),
            Err(_) => match &info_now {
                Some(p) if user_or_uid().trim() == p.user => Some(p.uid),
                Some(p) if user_or_uid().trim().is_empty() => Some(p.uid),
                _ => {
                    error.set(Some(
                        "Owner must be a numeric UID for now (or the existing username).".into(),
                    ));
                    return;
                }
            },
        };
        let gid: Option<u32> = match group_or_gid().trim().parse::<u32>() {
            Ok(n) => Some(n),
            Err(_) => match &info_now {
                Some(p) if group_or_gid().trim() == p.group => Some(p.gid),
                Some(p) if group_or_gid().trim().is_empty() => Some(p.gid),
                _ => {
                    error.set(Some(
                        "Group must be a numeric GID for now (or the existing group).".into(),
                    ));
                    return;
                }
            },
        };
        let mode_clean = mode().trim().trim_start_matches("0o").to_string();
        if mode_clean.is_empty() || !mode_clean.chars().all(|c| c.is_ascii_digit()) {
            error.set(Some("Mode must be an octal number (e.g. 755, 700).".into()));
            return;
        }
        busy.set(true);
        let req = api::SetPermsRequest {
            path: path_for_submit.clone(),
            uid,
            gid,
            mode: Some(mode_clean),
            recursive: recursive(),
        };
        let on_saved = props.on_saved.clone();
        spawn(async move {
            let result = api::put_perms(&req).await;
            busy.set(false);
            match result {
                Ok(()) => on_saved.call(()),
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    rsx! {
        div { class: "modal-overlay", onclick: move |_| props.on_close.call(()),
            form {
                class: "modal user-modal",
                onclick: move |e| e.stop_propagation(),
                onsubmit: move |e| { e.prevent_default(); submit(()); },

                div { class: "modal-header",
                    h3 { "Permissions" }
                    button {
                        class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()),
                        Icon { name: "x" }
                    }
                }

                div { class: "modal-body user-form",
                    p { class: "preview-label", style: "margin-bottom: .8em",
                        code { "{props.path}" }
                    }

                    if let Some(msg) = error() {
                        div { class: "banner err", pre { "{msg}" } }
                    }

                    match &*info.read_unchecked() {
                        None => rsx! { p { class: "preview-label", "Loading…" } },
                        Some(Err(e)) => rsx! {
                            div { class: "banner err", pre { "{e}" } }
                        },
                        Some(Ok(_)) => rsx! {
                            label { r#for: "perm-user", "Owner (UID or username)" }
                            input {
                                id: "perm-user",
                                r#type: "text",
                                value: "{user_or_uid()}",
                                oninput: move |e| user_or_uid.set(e.value()),
                            }

                            label { r#for: "perm-group", "Group (GID or group name)" }
                            input {
                                id: "perm-group",
                                r#type: "text",
                                value: "{group_or_gid()}",
                                oninput: move |e| group_or_gid.set(e.value()),
                            }

                            label { r#for: "perm-mode", "Mode (octal)" }
                            input {
                                id: "perm-mode",
                                r#type: "text",
                                // dioxus rsx parses `{…}` in string
                                // literals as format args; pass the
                                // regex via a const so the macro doesn't
                                // try to interpret the curly braces.
                                pattern: MODE_PATTERN,
                                value: "{mode()}",
                                oninput: move |e| mode.set(e.value()),
                            }
                            p { class: "preview-label",
                                "Examples: 755 (drwxr-xr-x), 770 (drwxrwx---), 700 (drwx------)."
                            }

                            label { class: "remember admin-toggle",
                                "data-tip": "Apply chown/chmod to all files and subdirectories. Slow on big trees.",
                                input {
                                    r#type: "checkbox",
                                    checked: recursive(),
                                    onchange: move |e| recursive.set(e.checked())
                                }
                                " Apply recursively"
                            }
                        },
                    }
                }

                div { class: "modal-footer",
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()), "Cancel" }
                    button { class: "primary", r#type: "submit", disabled: busy(),
                        if busy() { "Saving…" } else { "Apply" }
                    }
                }
            }
        }
    }
}
