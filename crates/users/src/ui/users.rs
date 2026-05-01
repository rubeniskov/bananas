//! Users page — list, add, change-password, delete.
//!
//! "Show system users" toggle exposes UID < 1000 accounts (root, daemon,
//! the bananas service user, etc.) in case the operator needs to see
//! them; they're hidden by default to keep the table focused on real
//! humans.

#![allow(non_snake_case)]

use dioxus::prelude::*;

use crate::{AuthCtx, api, api::ApiError, components::ConfirmModal, icons::Icon};

/// Pending confirm for a user action — either deleting the account or
/// toggling its admin flag (`bool` is the new desired admin state).
#[derive(Clone, PartialEq)]
enum PendingUserAction {
    Delete(String),
    ToggleAdmin { name: String, admin: bool },
}

#[component]
pub fn UsersPage() -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let mut users: Signal<Vec<api::UserAccount>> = use_signal(Vec::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut info: Signal<Option<String>> = use_signal(|| None);
    let mut show_system = use_signal(|| false);
    let mut add_open = use_signal(|| false);
    let mut pw_for: Signal<Option<String>> = use_signal(|| None);
    let mut tick = use_signal(|| 0u32);
    let mut pending: Signal<Option<PendingUserAction>> = use_signal(|| None);

    use_effect(move || {
        let _ = tick();
        let _ = auth_ctx.refresh.read();
        spawn(async move {
            match api::list_users().await {
                Ok(list) => {
                    error.set(None);
                    users.set(list.users);
                }
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    let visible: Vec<api::UserAccount> = if show_system() {
        users.read().clone()
    } else {
        users.read().iter().filter(|u| !u.system).cloned().collect()
    };

    let mut clear_messages = move || {
        error.set(None);
        info.set(None);
    };

    rsx! {
        div { class: "section-header",
            h2 { "Users" }
            label { class: "muted-toggle",
                input {
                    r#type: "checkbox",
                    checked: show_system(),
                    onchange: move |e| show_system.set(e.checked())
                }
                " Show system users (UID < 1000)"
            }
            span { class: "spacer" }
            button {
                class: "ghost",
                onclick: move |_| tick.set(tick() + 1),
                Icon { name: "rotate-cw" }
                "Refresh"
            }
            button {
                class: "primary",
                onclick: move |_| {
                    clear_messages();
                    add_open.set(true);
                },
                Icon { name: "user-plus" }
                "Add user"
            }
        }

        if let Some(msg) = error() { div { class: "banner err", pre { "{msg}" } } }
        if let Some(msg) = info() { div { class: "banner ok", pre { "{msg}" } } }

        if visible.is_empty() {
            p { class: "empty", "No accounts to show." }
        } else {
            table { class: "rows",
                thead {
                    tr {
                        th { "Name" }
                        th { "UID" }
                        th { "Full name" }
                        th { "Home" }
                        th { "Shell" }
                        th { "Groups" }
                        th { "Status" }
                        th {}
                    }
                }
                tbody {
                    for u in visible.iter() {
                        UserRow {
                            key: "{u.name}",
                            user: u.clone(),
                            on_password: {
                                let name = u.name.clone();
                                move |_| { clear_messages(); pw_for.set(Some(name.clone())); }
                            },
                            on_delete: {
                                let name = u.name.clone();
                                move |_| {
                                    clear_messages();
                                    pending.set(Some(PendingUserAction::Delete(name.clone())));
                                }
                            },
                            on_toggle_admin: {
                                let name = u.name.clone();
                                move |admin: bool| {
                                    clear_messages();
                                    pending.set(Some(PendingUserAction::ToggleAdmin {
                                        name: name.clone(),
                                        admin,
                                    }));
                                }
                            }
                        }
                    }
                }
            }
        }

        if add_open() {
            AddUserModal {
                on_close: move |_| add_open.set(false),
                on_created: move |username: String| {
                    add_open.set(false);
                    info.set(Some(format!("Created user {username}")));
                    tick.set(tick() + 1);
                },
                on_error: move |msg: String| error.set(Some(msg)),
                on_unauthorized: move |_| auth_ctx.signal_unauthorized()
            }
        }

        if let Some(name) = pw_for() {
            ChangePasswordModal {
                username: name.clone(),
                on_close: move |_| pw_for.set(None),
                on_saved: move |_| {
                    pw_for.set(None);
                    info.set(Some("Password updated".into()));
                },
                on_error: move |msg: String| error.set(Some(msg)),
                on_unauthorized: move |_| auth_ctx.signal_unauthorized()
            }
        }

        if let Some(action) = pending() {
            {
                let (title, message, details, label, danger) = match &action {
                    PendingUserAction::Delete(name) => (
                        "Delete user?".to_string(),
                        format!("Remove account '{name}' and its home directory."),
                        "Active SSH sessions are not closed; future logins are blocked.".to_string(),
                        "Delete user".to_string(),
                        true,
                    ),
                    PendingUserAction::ToggleAdmin { name, admin } => {
                        let (verb, label) = if *admin {
                            ("Grant", "Grant admin")
                        } else {
                            ("Revoke", "Revoke admin")
                        };
                        (
                            format!("{verb} admin access?"),
                            format!("{verb} BanaNAS admin access for {name}."),
                            "Admins can edit exports, mounts, users, and trigger reboots.".to_string(),
                            label.to_string(),
                            !*admin,
                        )
                    }
                };
                rsx! {
                    ConfirmModal {
                        title: title,
                        message: message,
                        details: details,
                        confirm_label: label,
                        danger: danger,
                        on_cancel: move |_| pending.set(None),
                        on_confirm: move |_| {
                            let action = action.clone();
                            pending.set(None);
                            spawn(async move {
                                match action {
                                    PendingUserAction::Delete(name) => {
                                        match api::delete_user(&name).await {
                                            Ok(()) => {
                                                info.set(Some(format!("Deleted user {name}")));
                                                tick.set(tick() + 1);
                                            }
                                            Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                                            Err(e) => error.set(Some(e.to_string())),
                                        }
                                    }
                                    PendingUserAction::ToggleAdmin { name, admin } => {
                                        match api::set_user_admin(&name, admin).await {
                                            Ok(()) => {
                                                info.set(Some(format!(
                                                    "{name} is {} an admin",
                                                    if admin { "now" } else { "no longer" }
                                                )));
                                                tick.set(tick() + 1);
                                            }
                                            Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                                            Err(e) => error.set(Some(e.to_string())),
                                        }
                                    }
                                }
                            });
                        },
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct UserRowProps {
    user: api::UserAccount,
    on_password: EventHandler<()>,
    on_delete: EventHandler<()>,
    on_toggle_admin: EventHandler<bool>,
}

#[component]
fn UserRow(props: UserRowProps) -> Element {
    let u = &props.user;
    let is_admin = u.name == "root" || u.groups.iter().any(|g| g == "bananas-admin");
    let admin_locked = u.name == "root";
    let groups = if u.groups.is_empty() {
        "—".into()
    } else {
        u.groups.join(", ")
    };
    let full = u.full_name.clone().unwrap_or_default();
    let status_class = if u.locked { "badge ro" } else { "badge rw" };
    let status_label = if u.locked { "locked" } else { "active" };
    let admin_btn_class: &'static str = if is_admin {
        "btn-icon warn"
    } else {
        "btn-icon ok"
    };
    let admin_btn_tip: &'static str = if is_admin {
        "Revoke this user's BanaNAS sign-in privilege."
    } else {
        "Add this user to bananas-admin so they can sign in."
    };
    let admin_btn_icon: &'static str = if is_admin { "lock" } else { "shield-check" };
    rsx! {
        tr {
            td { code { "{u.name}" } }
            td { code { "{u.uid}" } }
            td { "{full}" }
            td { code { "{u.home}" } }
            td { code { "{u.shell}" } }
            td { class: "muted", "{groups}" }
            td {
                span { class: "{status_class}", "{status_label}" }
                if is_admin {
                    span {
                        class: "badge",
                        style: "margin-left: 4px; background: #fff5cc; color: #856400",
                        "data-tip": if admin_locked { "Root is implicitly an admin." } else { "Member of bananas-admin — can sign in to this UI." },
                        "admin"
                    }
                }
                if u.system {
                    span { class: "badge warn", style: "margin-left: 4px", "system" }
                }
            }
            td { class: "row-actions",
                div { class: "actions",
                    if !u.system && !admin_locked {
                        button {
                            class: "{admin_btn_class}",
                            "data-tip": "{admin_btn_tip}",
                            onclick: move |_| props.on_toggle_admin.call(!is_admin),
                            Icon { name: admin_btn_icon }
                        }
                    }
                    button {
                        class: "btn-icon edit",
                        "data-tip": "Change this user's password",
                        onclick: move |_| props.on_password.call(()),
                        Icon { name: "key-round" }
                    }
                    if !u.system {
                        button {
                            class: "btn-icon delete",
                            "data-tip": "Delete user (and home dir)",
                            onclick: move |_| props.on_delete.call(()),
                            Icon { name: "trash-2" }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct AddUserModalProps {
    on_close: EventHandler<()>,
    on_created: EventHandler<String>,
    on_error: EventHandler<String>,
    on_unauthorized: EventHandler<()>,
}

#[component]
fn AddUserModal(props: AddUserModalProps) -> Element {
    let mut username = use_signal(String::new);
    let mut full_name = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut confirm_pw = use_signal(String::new);
    let mut grant_admin = use_signal(|| false);
    let mut busy = use_signal(|| false);

    let mut submit = move |_| {
        if busy() {
            return;
        }
        if username().is_empty() || password().is_empty() {
            props
                .on_error
                .call("username and password are required".into());
            return;
        }
        if password() != confirm_pw() {
            props.on_error.call("passwords do not match".into());
            return;
        }
        busy.set(true);
        let body = api::CreateUser {
            username: username(),
            password: password(),
            full_name: if full_name().is_empty() {
                None
            } else {
                Some(full_name())
            },
            admin: grant_admin(),
        };
        let on_created = props.on_created.clone();
        let on_error = props.on_error.clone();
        let on_unauthorized = props.on_unauthorized.clone();
        spawn(async move {
            let result = api::create_user(&body).await;
            password.set(String::new());
            confirm_pw.set(String::new());
            busy.set(false);
            match result {
                Ok(()) => on_created.call(body.username),
                Err(ApiError::Unauthorized) => on_unauthorized.call(()),
                Err(e) => on_error.call(e.to_string()),
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
                    h3 { "Add user" }
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()), "✕" }
                }

                div { class: "modal-body user-form",
                    label { r#for: "u-name", "Username" }
                    input {
                        id: "u-name",
                        r#type: "text",
                        autocomplete: "off",
                        autofocus: true,
                        required: true,
                        value: "{username()}",
                        oninput: move |e| username.set(e.value())
                    }
                    p { class: "preview-label",
                        "Lowercase letters, digits, underscore, hyphen. 1–32 chars."
                    }

                    label { r#for: "u-full", "Full name (optional)" }
                    input {
                        id: "u-full",
                        r#type: "text",
                        value: "{full_name()}",
                        oninput: move |e| full_name.set(e.value())
                    }

                    label { r#for: "u-pw", "Password" }
                    input {
                        id: "u-pw",
                        r#type: "password",
                        autocomplete: "new-password",
                        required: true,
                        value: "{password()}",
                        oninput: move |e| password.set(e.value())
                    }

                    label { r#for: "u-pw2", "Confirm password" }
                    input {
                        id: "u-pw2",
                        r#type: "password",
                        autocomplete: "new-password",
                        required: true,
                        value: "{confirm_pw()}",
                        oninput: move |e| confirm_pw.set(e.value())
                    }

                    label { class: "remember admin-toggle",
                        "data-tip": "Add the user to bananas-admin so they can sign in to this UI. Without this, they're a system account only.",
                        input {
                            r#type: "checkbox",
                            checked: grant_admin(),
                            onchange: move |e| grant_admin.set(e.checked())
                        }
                        " Grant admin access (sign-in to BanaNAS)"
                    }
                }

                div { class: "modal-footer",
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()), "Cancel" }
                    button { class: "primary", r#type: "submit", disabled: busy(),
                        if busy() { "Creating…" } else { "Create user" }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ChangePasswordModalProps {
    username: String,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
    on_error: EventHandler<String>,
    on_unauthorized: EventHandler<()>,
}

#[component]
fn ChangePasswordModal(props: ChangePasswordModalProps) -> Element {
    let mut password = use_signal(String::new);
    let mut confirm = use_signal(String::new);
    let mut busy = use_signal(|| false);

    let username_for_submit = props.username.clone();
    let mut submit = move |_| {
        if busy() {
            return;
        }
        if password().is_empty() {
            props.on_error.call("password must not be empty".into());
            return;
        }
        if password() != confirm() {
            props.on_error.call("passwords do not match".into());
            return;
        }
        busy.set(true);
        let username = username_for_submit.clone();
        let pw = password();
        let on_saved = props.on_saved.clone();
        let on_error = props.on_error.clone();
        let on_unauthorized = props.on_unauthorized.clone();
        spawn(async move {
            let result = api::set_user_password(&username, &pw).await;
            password.set(String::new());
            confirm.set(String::new());
            busy.set(false);
            match result {
                Ok(()) => on_saved.call(()),
                Err(ApiError::Unauthorized) => on_unauthorized.call(()),
                Err(e) => on_error.call(e.to_string()),
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
                    h3 { "Change password — {props.username}" }
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()), "✕" }
                }

                div { class: "modal-body user-form",
                    label { r#for: "pw-new", "New password" }
                    input {
                        id: "pw-new",
                        r#type: "password",
                        autocomplete: "new-password",
                        autofocus: true,
                        required: true,
                        value: "{password()}",
                        oninput: move |e| password.set(e.value())
                    }
                    label { r#for: "pw-conf", "Confirm" }
                    input {
                        id: "pw-conf",
                        r#type: "password",
                        autocomplete: "new-password",
                        required: true,
                        value: "{confirm()}",
                        oninput: move |e| confirm.set(e.value())
                    }
                }

                div { class: "modal-footer",
                    button { class: "ghost", r#type: "button",
                        onclick: move |_| props.on_close.call(()), "Cancel" }
                    button { class: "primary", r#type: "submit", disabled: busy(),
                        if busy() { "Saving…" } else { "Update password" }
                    }
                }
            }
        }
    }
}
