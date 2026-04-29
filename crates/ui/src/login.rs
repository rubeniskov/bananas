//! Login page. Renders when /api/me returns 401 at app load.

#![allow(non_snake_case)]

use dioxus::prelude::*;

use crate::api;

#[derive(Props, Clone, PartialEq)]
pub struct LoginProps {
    pub on_signed_in: EventHandler<api::Me>,
}

#[component]
pub fn Login(props: LoginProps) -> Element {
    let mut username = use_signal(|| "root".to_string());
    let mut password = use_signal(String::new);
    let mut remember = use_signal(|| true);
    let mut busy = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let mut submit = move |_| {
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
            // Clear the password from the signal once the request resolves,
            // regardless of outcome — keeps it out of devtools / heap dumps
            // longer than necessary.
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
                p { class: "login-sub", "Sign in with a system account" }

                if let Some(msg) = error() {
                    div { class: "banner err", pre { "{msg}" } }
                }

                form {
                    class: "login-form",
                    onsubmit: move |e| { e.prevent_default(); submit(()); },

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
            }
        }
    }
}
