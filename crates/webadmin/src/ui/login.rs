//! Login page. Renders when /api/me returns 401 at app load.
//!
//! Two modes:
//!   - **Sign-in** (default) — username + password + remember-me.
//!   - **Set new password** (entered automatically when the server
//!     returns `password_expired`) — the current password is carried
//!     over from the previous attempt, the user picks a new one and
//!     retypes it. On success the server issues the session cookie
//!     directly; we drop straight into the admin shell.

#![allow(non_snake_case)]

use dioxus::prelude::*;

use crate::api;

#[derive(Props, Clone, PartialEq)]
pub struct LoginProps {
    pub on_signed_in: EventHandler<api::Me>,
}

/// State of the login card. The expired branch holds the credentials
/// proven valid by the just-failed login so the user doesn't have to
/// retype their old password.
#[derive(Clone, PartialEq)]
enum Stage {
    SignIn,
    Expired {
        username: String,
        old_password: String,
        remember: bool,
    },
}

#[component]
pub fn Login(props: LoginProps) -> Element {
    let mut stage = use_signal(|| Stage::SignIn);
    let mut username = use_signal(|| "root".to_string());
    let mut password = use_signal(String::new);
    let mut remember = use_signal(|| true);
    let mut new_password = use_signal(String::new);
    let mut new_password_confirm = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let mut submit_login = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        error.set(None);
        let req = api::LoginRequest {
            username: username(),
            password: password(),
            remember: remember(),
        };
        let on_signed_in = props.on_signed_in.clone();
        spawn(async move {
            let result = api::login(&req).await;
            match result {
                Ok(me) => {
                    password.set(String::new());
                    busy.set(false);
                    on_signed_in.call(me);
                }
                Err(e) if e == api::PASSWORD_EXPIRED => {
                    // Bridge into the "set new password" form. Carry the
                    // (already-validated) old password over so the user
                    // doesn't retype it.
                    busy.set(false);
                    stage.set(Stage::Expired {
                        username: req.username.clone(),
                        old_password: req.password.clone(),
                        remember: req.remember,
                    });
                    error.set(None);
                }
                Err(e) => {
                    password.set(String::new());
                    busy.set(false);
                    error.set(Some(e));
                }
            }
        });
    };

    let mut submit_change = move |_| {
        if busy() {
            return;
        }
        let Stage::Expired {
            username: u,
            old_password: op,
            remember: rem,
        } = stage()
        else {
            return;
        };
        let new = new_password();
        let confirm = new_password_confirm();
        if new.is_empty() {
            error.set(Some("New password cannot be empty.".into()));
            return;
        }
        if new != confirm {
            error.set(Some("Passwords don't match.".into()));
            return;
        }
        if new == op {
            error.set(Some(
                "New password must differ from the current one.".into(),
            ));
            return;
        }
        busy.set(true);
        error.set(None);
        let req = api::ChangePasswordRequest {
            username: u.clone(),
            old_password: op,
            new_password: new,
            remember: rem,
        };
        let on_signed_in = props.on_signed_in.clone();
        spawn(async move {
            let result = api::change_password(&req).await;
            // Don't keep new-password material in signals once the
            // request resolves either way.
            new_password.set(String::new());
            new_password_confirm.set(String::new());
            password.set(String::new());
            match result {
                Ok(me) => {
                    busy.set(false);
                    on_signed_in.call(me);
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e));
                }
            }
        });
    };

    rsx! {
        main { class: "login-shell",
            div { class: "login-card",
                h1 { "BanaNAS" }
                {
                    let stage_now = stage();
                    let sub = match &stage_now {
                        Stage::SignIn => "Sign in with a system account",
                        Stage::Expired { .. } => "Set a new password to continue",
                    };
                    rsx! {
                        p { class: "login-sub", "{sub}" }
                    }
                }

                if let Some(msg) = error() {
                    div { class: "banner err", pre { "{msg}" } }
                }

                {
                    match stage() {
                        Stage::SignIn => rsx! {
                            form {
                                class: "login-form",
                                onsubmit: move |e| { e.prevent_default(); submit_login(()); },

                                label { r#for: "user", "Username" }
                                input {
                                    id: "user",
                                    r#type: "text",
                                    autocomplete: "username",
                                    autofocus: true,
                                    required: true,
                                    disabled: busy(),
                                    value: "{username()}",
                                    oninput: move |e| username.set(e.value())
                                }

                                label { r#for: "pass", "Password" }
                                input {
                                    id: "pass",
                                    r#type: "password",
                                    autocomplete: "current-password",
                                    required: true,
                                    disabled: busy(),
                                    value: "{password()}",
                                    oninput: move |e| password.set(e.value())
                                }

                                label { class: "remember",
                                    input {
                                        r#type: "checkbox",
                                        checked: remember(),
                                        disabled: busy(),
                                        onchange: move |e| remember.set(e.checked())
                                    }
                                    " Remember me on this device"
                                }

                                button {
                                    class: "primary",
                                    r#type: "submit",
                                    disabled: busy(),
                                    if busy() { "Signing in…" } else { "Sign in" }
                                }
                            }
                        },
                        Stage::Expired { username: u, .. } => rsx! {
                            form {
                                class: "login-form",
                                onsubmit: move |e| { e.prevent_default(); submit_change(()); },

                                p { class: "preview-label",
                                    "The password for "
                                    code { "{u}" }
                                    " was flagged for rotation on first sign-in. Pick a new one to continue."
                                }

                                label { r#for: "newpass", "New password" }
                                input {
                                    id: "newpass",
                                    r#type: "password",
                                    autocomplete: "new-password",
                                    autofocus: true,
                                    required: true,
                                    disabled: busy(),
                                    value: "{new_password()}",
                                    oninput: move |e| new_password.set(e.value())
                                }

                                label { r#for: "newpass2", "Confirm new password" }
                                input {
                                    id: "newpass2",
                                    r#type: "password",
                                    autocomplete: "new-password",
                                    required: true,
                                    disabled: busy(),
                                    value: "{new_password_confirm()}",
                                    oninput: move |e| new_password_confirm.set(e.value())
                                }

                                button {
                                    class: "primary",
                                    r#type: "submit",
                                    disabled: busy(),
                                    if busy() { "Saving…" } else { "Set password & sign in" }
                                }
                                button {
                                    class: "ghost",
                                    r#type: "button",
                                    disabled: busy(),
                                    onclick: move |_| {
                                        stage.set(Stage::SignIn);
                                        new_password.set(String::new());
                                        new_password_confirm.set(String::new());
                                        password.set(String::new());
                                        error.set(None);
                                    },
                                    "Cancel"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
