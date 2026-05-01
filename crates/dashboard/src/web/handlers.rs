//! /api/dashboard/config — GET + PUT plumbing for the
//! `/etc/bananas/dashboard.toml` file the SLINT app reads.
//! Lifted from `crates/webadmin/src/service_config.rs` (the
//! dashboard half).

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bananas_proto::engine::v1::{
    ReadServiceConfigRequest, WriteServiceConfigRequest, engine_service_client::EngineServiceClient,
};
use bananas_proto::engine_client;
use serde::Deserialize;
use serde_json::json;

use super::AppState;

#[derive(Debug, Deserialize)]
pub struct PutConfig {
    pub config: String,
}

/// GET handler for /api/dashboard/config.
pub async fn get_config(State(state): State<AppState>) -> Response {
    read(&state, "dashboard").await
}

/// PUT handler for /api/dashboard/config.
pub async fn put_config(State(state): State<AppState>, Json(req): Json<PutConfig>) -> Response {
    write(&state, "dashboard", req.config).await
}

async fn read(state: &AppState, name: &'static str) -> Response {
    let channel = match engine_client::channel(&state.helper_grpc_socket).await {
        Ok(c) => c,
        Err(e) => return err_500(format!("helper unreachable: {e}")),
    };
    let mut client = EngineServiceClient::new(channel);
    match client
        .read_service_config(ReadServiceConfigRequest { name: name.into() })
        .await
    {
        Ok(resp) => {
            let content = resp.into_inner().content;
            let body = if content.trim().is_empty() {
                default_dashboard_toml()
            } else {
                content
            };
            (StatusCode::OK, Json(json!({ "config": body }))).into_response()
        }
        Err(status) => err_500(format!("ReadServiceConfig({name}) failed: {status}")),
    }
}

async fn write(state: &AppState, name: &'static str, content: String) -> Response {
    let channel = match engine_client::channel(&state.helper_grpc_socket).await {
        Ok(c) => c,
        Err(e) => return err_500(format!("helper unreachable: {e}")),
    };
    let mut client = EngineServiceClient::new(channel);
    match client
        .write_service_config(WriteServiceConfigRequest {
            name: name.into(),
            content,
        })
        .await
    {
        Ok(resp) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "output": resp.into_inner().output })),
        )
            .into_response(),
        Err(status) if status.code() == tonic::Code::InvalidArgument => {
            err_400(status.message().to_string())
        }
        Err(status) => err_500(format!("WriteServiceConfig({name}) failed: {status}")),
    }
}

fn err_400(msg: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "ok": false, "error": msg })),
    )
        .into_response()
}

fn err_500(msg: String) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "ok": false, "error": msg })),
    )
        .into_response()
}

fn default_dashboard_toml() -> String {
    let cfg = serde_json::json!({
        "width": 800u32,
        "height": 480u32,
        "title": "bananas-dashboard",
        "theme": "auto",
        "spark_window": 60u32,
        "refresh_ms": 2000u32,
        "socket": "/run/bananas/stats.sock",
    });
    let body = toml::to_string_pretty(&cfg).unwrap_or_default();
    format!(
        "# /etc/bananas/dashboard.toml — bananas-dashboard appearance + socket subscribe path.\n\
         # Sampler-side fields live in stats.toml.\n\n{body}"
    )
}
