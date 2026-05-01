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
use bananas_engine::{Command, Response as HelperResponse};
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
    let cmd = Command::ReadServiceConfig { name: name.into() };
    match bananas_engine::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => {
            let body = if output.trim().is_empty() {
                default_dashboard_toml()
            } else {
                output
            };
            (StatusCode::OK, Json(json!({ "config": body }))).into_response()
        }
        Ok(HelperResponse { error, .. }) => {
            err_500(error.unwrap_or_else(|| format!("helper rejected ReadServiceConfig({name})")))
        }
        Err(e) => err_500(format!("helper unreachable: {e}")),
    }
}

async fn write(state: &AppState, name: &'static str, content: String) -> Response {
    let cmd = Command::WriteServiceConfig {
        name: name.into(),
        content,
    };
    match bananas_engine::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "output": output })),
        )
            .into_response(),
        Ok(HelperResponse { error, output, .. }) => err_400(format!(
            "{}\n\n{}",
            error.unwrap_or_else(|| format!("helper rejected WriteServiceConfig({name})")),
            output
        )),
        Err(e) => err_500(format!("helper unreachable: {e}")),
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
