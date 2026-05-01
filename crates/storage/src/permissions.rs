//! /api/permissions — stat + chown/chmod through the engine over gRPC.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bananas_proto::engine::v1::{
    SetPermissionsRequest, StatRequest, engine_service_client::EngineServiceClient,
};
use bananas_proto::engine_client;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct PermsQuery {
    pub path: String,
}

pub async fn get_perms(State(state): State<AppState>, Query(q): Query<PermsQuery>) -> Response {
    let mut client = match client_for(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match client.stat(StatRequest { path: q.path }).await {
        Ok(resp) => json_response(&resp.into_inner().stat_json),
        Err(status) => status_to_response(status),
    }
}

#[derive(Debug, Deserialize)]
pub struct SetPerms {
    pub path: String,
    #[serde(default)]
    pub uid: Option<u32>,
    #[serde(default)]
    pub gid: Option<u32>,
    /// Numeric mode as a string ("755", "0o755", or decimal). The
    /// engine validates and rejects non-numeric input.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub recursive: bool,
}

pub async fn put_perms(State(state): State<AppState>, Json(req): Json<SetPerms>) -> Response {
    let mut client = match client_for(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let uid = req.uid.unwrap_or(0);
    let gid = req.gid.unwrap_or(0);
    let mode_str = req.mode.clone().unwrap_or_default();
    let rpc = client
        .set_permissions(SetPermissionsRequest {
            path: req.path,
            uid,
            uid_set: req.uid.is_some(),
            gid,
            gid_set: req.gid.is_some(),
            mode: mode_str,
            mode_set: req.mode.is_some(),
            recursive: req.recursive,
        })
        .await;
    match rpc {
        Ok(resp) => {
            Json(json!({ "ok": true, "message": resp.into_inner().output })).into_response()
        }
        Err(status) => status_to_response(status),
    }
}

async fn client_for(
    state: &AppState,
) -> Result<EngineServiceClient<tonic::transport::Channel>, Response> {
    match engine_client::channel(&state.helper_grpc_socket).await {
        Ok(c) => Ok(EngineServiceClient::new(c)),
        Err(e) => Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("helper unreachable: {e}") })),
        )
            .into_response()),
    }
}

fn json_response(payload: &str) -> Response {
    match serde_json::from_str::<Value>(payload) {
        Ok(v) => Json(v).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("malformed engine reply: {e}") })),
        )
            .into_response(),
    }
}

fn status_to_response(status: tonic::Status) -> Response {
    use tonic::Code;
    let http = match status.code() {
        Code::InvalidArgument | Code::AlreadyExists | Code::NotFound => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_GATEWAY,
    };
    (
        http,
        Json(json!({ "ok": false, "error": status.message().to_string() })),
    )
        .into_response()
}
