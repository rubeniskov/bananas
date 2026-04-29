//! /api/users — list / create / delete / set-password.
//!
//! All four endpoints proxy to bananas-helper, which has root and is the
//! only thing on the system that can shell out to useradd/userdel/chpasswd.
//! The server's job here is request decoding + status-code mapping.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bananas_helper::{Command, Response as HelperResponse};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;

/// Pass-through helper. Calls the daemon, parses its `output` field as
/// JSON, returns it. Maps helper failures to a 400 with the error string.
async fn proxy(state: &AppState, cmd: Command) -> Response {
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => match serde_json::from_str::<Value>(&output) {
            Ok(value) => Json(value).into_response(),
            // ListUsers returns a JSON blob; the action commands return a
            // human-readable string. Wrap the latter so the client gets a
            // consistent JSON shape.
            Err(_) => Json(json!({ "ok": true, "message": output })).into_response(),
        },
        Ok(HelperResponse { error, output, .. }) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": error.unwrap_or_else(|| "helper rejected the request".into()),
                "output": output,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "ok": false,
                "error": format!("helper unreachable: {e}")
            })),
        )
            .into_response(),
    }
}

pub async fn list(State(state): State<AppState>) -> Response {
    proxy(&state, Command::ListUsers).await
}

#[derive(Debug, Deserialize)]
pub struct CreateUser {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub admin: bool,
}

pub async fn create(State(state): State<AppState>, Json(req): Json<CreateUser>) -> Response {
    proxy(
        &state,
        Command::CreateUser {
            username: req.username,
            password: req.password,
            full_name: req.full_name,
            admin: req.admin,
            password_is_hash: false,
        },
    )
    .await
}

#[derive(Debug, Deserialize)]
pub struct SetAdmin {
    pub admin: bool,
}

pub async fn set_admin(
    State(state): State<AppState>,
    Path(username): Path<String>,
    Json(req): Json<SetAdmin>,
) -> Response {
    proxy(
        &state,
        Command::SetAdmin {
            username,
            admin: req.admin,
        },
    )
    .await
}

pub async fn delete(State(state): State<AppState>, Path(username): Path<String>) -> Response {
    proxy(&state, Command::DeleteUser { username }).await
}

#[derive(Debug, Deserialize)]
pub struct SetPassword {
    pub password: String,
}

pub async fn set_password(
    State(state): State<AppState>,
    Path(username): Path<String>,
    Json(req): Json<SetPassword>,
) -> Response {
    proxy(
        &state,
        Command::SetPassword {
            username,
            password: req.password,
        },
    )
    .await
}
