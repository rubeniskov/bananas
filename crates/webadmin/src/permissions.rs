//! /api/permissions — stat + chown/chmod through the helper.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bananas_engine::{Command, Response as HelperResponse};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct PermsQuery {
    pub path: String,
}

pub async fn get_perms(State(state): State<AppState>, Query(q): Query<PermsQuery>) -> Response {
    proxy(&state, Command::Stat { path: q.path }).await
}

#[derive(Debug, Deserialize)]
pub struct SetPerms {
    pub path: String,
    #[serde(default)]
    pub uid: Option<u32>,
    #[serde(default)]
    pub gid: Option<u32>,
    /// Numeric mode as a string ("755", "0o755", or decimal). The
    /// helper validates and rejects non-numeric input.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub recursive: bool,
}

pub async fn put_perms(State(state): State<AppState>, Json(req): Json<SetPerms>) -> Response {
    proxy(
        &state,
        Command::SetPermissions {
            path: req.path,
            uid: req.uid,
            gid: req.gid,
            mode: req.mode,
            recursive: req.recursive,
        },
    )
    .await
}

async fn proxy(state: &AppState, cmd: Command) -> Response {
    match bananas_engine::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => match serde_json::from_str::<Value>(&output) {
            Ok(value) => Json(value).into_response(),
            // Action commands return a human-readable string; wrap so
            // the UI gets a consistent JSON shape either way.
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
            Json(json!({ "ok": false, "error": format!("helper unreachable: {e}") })),
        )
            .into_response(),
    }
}
