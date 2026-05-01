//! Generic GET/PUT plumbing for service config files. The helper has
//! a small allowlist (stats / dashboard / cloud / system); each entry
//! maps to a path under /etc/bananas/ and an optional list of systemd
//! units to bounce on write. This module wraps that with two HTTP
//! handlers so each new config name is one route, not duplicate code.

use axum::{Json, extract::State, response::Response};
use bananas_proto::engine::v1::{
    ReadServiceConfigRequest, WriteServiceConfigRequest, engine_service_client::EngineServiceClient,
};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::engine_grpc;
use crate::errors::{err_400, err_500};

/// Read the named config file via the helper. `default_toml` is rendered
/// when the on-disk file is missing or empty so the UI editor always
/// has something concrete to show on a fresh image.
pub async fn read(
    state: &AppState,
    name: &'static str,
    default_toml: impl FnOnce() -> String,
) -> Response {
    let channel = match engine_grpc::channel(&state.helper_grpc_socket).await {
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
                default_toml()
            } else {
                content
            };
            axum::response::IntoResponse::into_response(Json(json!({ "config": body })))
        }
        Err(status) => err_500(format!("ReadServiceConfig({name}) failed: {status}")),
    }
}

#[derive(Debug, Deserialize)]
pub struct PutConfig {
    pub config: String,
}

/// Write the named config file via the helper. Helper handles the
/// atomic-replace + post-write systemctl restart (or no-op restart
/// for configs the consumer reloads in place).
pub async fn write(state: &AppState, name: &'static str, content: String) -> Response {
    let channel = match engine_grpc::channel(&state.helper_grpc_socket).await {
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
        Ok(resp) => axum::response::IntoResponse::into_response(Json(
            json!({ "ok": true, "output": resp.into_inner().output }),
        )),
        // InvalidArgument = bad TOML or unknown name from the
        // caller; everything else is a real engine fs/systemctl
        // problem.
        Err(status) if status.code() == tonic::Code::InvalidArgument => {
            err_400(status.message().to_string())
        }
        Err(status) => err_500(format!("WriteServiceConfig({name}) failed: {status}")),
    }
}

// /api/dashboard/config moved to bananas-dashboard-web in
// commit 7. The `read`/`write` helpers below still serve
// /api/system/config (system.toml is host-level config that
// stays on webadmin).

/// GET handler for /api/system/config.
pub async fn get_system(State(state): State<AppState>) -> Response {
    read(&state, "system", default_system_toml).await
}

/// PUT handler for /api/system/config.
pub async fn put_system(State(state): State<AppState>, Json(req): Json<PutConfig>) -> Response {
    write(&state, "system", req.config).await
}

fn default_system_toml() -> String {
    let mut out = String::new();
    out.push_str("# /etc/bananas/system.toml — host-level settings.\n");
    out.push_str("# timezone is auto-detected on first boot via geoip; you can\n");
    out.push_str("# override here. Use IANA tzdata names (e.g. \"Europe/Madrid\").\n\n");
    out.push_str("[system]\n");
    out.push_str("# timezone = \"UTC\"\n");
    out
}
