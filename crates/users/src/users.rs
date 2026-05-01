//! /api/users — list / create / delete / set-password.
//!
//! All four endpoints proxy to bananas-engine, which has root and is the
//! only thing on the system that can shell out to useradd/userdel/chpasswd.
//! The server's job here is request decoding + status-code mapping.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bananas_proto::engine::v1::{
    CreateUserRequest, DeleteUserRequest, ListUsersRequest, SetAdminRequest, SetPasswordRequest,
    engine_service_client::EngineServiceClient,
};
use bananas_proto::engine_client;
use serde::Deserialize;
use serde_json::{Value, json};
use tonic::transport::Channel;

use crate::AppState;

/// Builds an engine gRPC client. On transport failure returns a
/// pre-rendered 502 response so the call sites stay terse.
async fn engine_client(state: &AppState) -> Result<EngineServiceClient<Channel>, Response> {
    match engine_client::channel(&state.helper_grpc_socket).await {
        Ok(c) => Ok(EngineServiceClient::new(c)),
        Err(e) => Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "ok": false,
                "error": format!("helper unreachable: {e}")
            })),
        )
            .into_response()),
    }
}

/// Map a tonic `Status` failure to the JSON shape the SPA expects.
/// InvalidArgument / AlreadyExists / NotFound surface as 400; the
/// rest are 502.
fn status_to_response(status: tonic::Status) -> Response {
    use tonic::Code;
    let http = match status.code() {
        Code::InvalidArgument | Code::AlreadyExists | Code::NotFound => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_GATEWAY,
    };
    (
        http,
        Json(json!({
            "ok": false,
            "error": status.message().to_string(),
        })),
    )
        .into_response()
}

pub async fn list(State(state): State<AppState>) -> Response {
    let mut client = match engine_client(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match client.list_users(ListUsersRequest {}).await {
        Ok(resp) => match serde_json::from_str::<Value>(&resp.into_inner().users_json) {
            Ok(value) => Json(value).into_response(),
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "ok": false, "error": format!("list_users payload: {e}") })),
            )
                .into_response(),
        },
        Err(status) => status_to_response(status),
    }
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
    let mut client = match engine_client(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match client
        .create_user(CreateUserRequest {
            username: req.username,
            password: req.password,
            full_name: req.full_name.unwrap_or_default(),
            admin: req.admin,
            password_is_hash: false,
        })
        .await
    {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(status) => status_to_response(status),
    }
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
    let mut client = match engine_client(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match client
        .set_admin(SetAdminRequest {
            username,
            admin: req.admin,
        })
        .await
    {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(status) => status_to_response(status),
    }
}

pub async fn delete(State(state): State<AppState>, Path(username): Path<String>) -> Response {
    let mut client = match engine_client(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match client.delete_user(DeleteUserRequest { username }).await {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(status) => status_to_response(status),
    }
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
    let mut client = match engine_client(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match client
        .set_password(SetPasswordRequest {
            username,
            password: req.password,
        })
        .await
    {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(status) => status_to_response(status),
    }
}
