//! Generic GET/PUT plumbing for service config files. The helper has
//! a small allowlist (stats / dashboard / cloud / system); each entry
//! maps to a path under /etc/bananas/ and an optional list of systemd
//! units to bounce on write. This module wraps that with two HTTP
//! handlers so each new config name is one route, not duplicate code.

use axum::{Json, extract::State, response::Response};
use bananas_engine::{Command, Response as HelperResponse};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::stats::{err_400, err_500};

/// Read the named config file via the helper. `default_toml` is rendered
/// when the on-disk file is missing or empty so the UI editor always
/// has something concrete to show on a fresh image.
pub async fn read(
    state: &AppState,
    name: &'static str,
    default_toml: impl FnOnce() -> String,
) -> Response {
    let cmd = Command::ReadServiceConfig { name: name.into() };
    match bananas_engine::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => {
            let body = if output.trim().is_empty() {
                default_toml()
            } else {
                output
            };
            axum::response::IntoResponse::into_response(Json(json!({ "config": body })))
        }
        Ok(HelperResponse { error, .. }) => {
            err_500(error.unwrap_or_else(|| format!("helper rejected ReadServiceConfig({name})")))
        }
        Err(e) => err_500(format!("helper unreachable: {e}")),
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
    let cmd = Command::WriteServiceConfig {
        name: name.into(),
        content,
    };
    match bananas_engine::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => axum::response::IntoResponse::into_response(Json(
            json!({ "ok": true, "output": output }),
        )),
        Ok(HelperResponse { error, output, .. }) => err_400(format!(
            "{}\n\n{}",
            error.unwrap_or_else(|| format!("helper rejected WriteServiceConfig({name})")),
            output
        )),
        Err(e) => err_500(format!("helper unreachable: {e}")),
    }
}

/// GET handler for /api/dashboard/config.
pub async fn get_dashboard(State(state): State<AppState>) -> Response {
    read(&state, "dashboard", default_dashboard_toml).await
}

/// PUT handler for /api/dashboard/config.
pub async fn put_dashboard(State(state): State<AppState>, Json(req): Json<PutConfig>) -> Response {
    write(&state, "dashboard", req.config).await
}

fn default_dashboard_toml() -> String {
    // Render just the [ui] + [live_socket] sections from the shared
    // Config — those are the only pieces dashboard.toml owns.
    let ui = bananas_stats::config::Ui::default();
    let live = bananas_stats::config::LiveSocket::default();
    let mut out = String::new();
    out.push_str(
        "# /etc/bananas/dashboard.toml — bananas-dashboard appearance + socket subscribe path.\n",
    );
    out.push_str("# Sampler-side fields live in stats.toml.\n\n");
    out.push_str("[ui]\n");
    out.push_str(&toml::to_string(&ui).unwrap_or_default());
    out.push('\n');
    out.push_str("[live_socket]\n");
    out.push_str(&toml::to_string(&live).unwrap_or_default());
    out
}

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
