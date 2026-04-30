//! System-level settings — currently just timezone, but designed to
//! grow (hostname, NTP servers, locale). Backing store is
//! /etc/bananas/system.toml; live changes that affect kernel state
//! (timezone via timedatectl) go through the helper.
//!
//! On startup, `spawn_first_boot_geoip` looks at system.toml; if no
//! timezone is set yet, it shells out to `curl https://ipapi.co/timezone/`
//! and applies the result via the helper. Failures are non-fatal —
//! the system stays on UTC and the operator can set it manually
//! through Settings → General.

use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use bananas_helper::{Command, Response as HelperResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::stats::{err_400, err_500};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemConfig {
    pub system: SystemBlock,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemBlock {
    /// IANA tzdata zone name (e.g. "Europe/Madrid"). Empty / None means
    /// "use whatever the OS booted with" — typically UTC.
    pub timezone: Option<String>,
}

/// Read /etc/bananas/system.toml via the helper and parse into the
/// strongly-typed SystemConfig. Missing file = default (no timezone).
pub async fn read_system_config(state: &AppState) -> Result<SystemConfig, String> {
    let cmd = Command::ReadServiceConfig {
        name: "system".into(),
    };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => {
            if output.trim().is_empty() {
                Ok(SystemConfig::default())
            } else {
                toml::from_str(&output).map_err(|e| format!("parsing system.toml: {e}"))
            }
        }
        Ok(HelperResponse { error, .. }) => {
            Err(error.unwrap_or_else(|| "helper rejected ReadServiceConfig(system)".into()))
        }
        Err(e) => Err(format!("helper unreachable: {e}")),
    }
}

#[derive(Debug, Deserialize)]
pub struct PutTimezone {
    pub tz: String,
}

/// POST /api/system/timezone — apply the timezone via timedatectl AND
/// persist to system.toml AND bounce bananas-dashboard so the LCD
/// picks up the new local offset. The helper owns the whole sequence
/// so every caller (web UI, bananas-config CLI / TUI) gets identical
/// behaviour without duplicating the persist + restart steps.
pub async fn post_timezone(
    State(state): State<AppState>,
    Json(req): Json<PutTimezone>,
) -> Response {
    let tz = req.tz.trim().to_string();
    if tz.is_empty() {
        return err_400("timezone required".into());
    }
    let cmd = Command::SetTimezone { tz: tz.clone() };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => axum::response::IntoResponse::into_response(Json(
            json!({ "ok": true, "tz": tz, "output": output }),
        )),
        Ok(HelperResponse { error, output, .. }) => err_400(format!(
            "{}\n\n{}",
            error.unwrap_or_else(|| "helper rejected SetTimezone".into()),
            output
        )),
        Err(e) => err_500(format!("helper unreachable: {e}")),
    }
}

/// GET /api/system/timezones — return the helper's view of every IANA
/// zone the OS knows about, as a flat JSON array. Backs the Settings →
/// General timezone picker (datalist autocomplete).
pub async fn get_timezones(State(state): State<AppState>) -> Response {
    let cmd = Command::ListTimezones;
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => {
            // Helper returns a JSON array string; pass it through as-is.
            (
                axum::http::StatusCode::OK,
                [("content-type", "application/json")],
                output,
            )
                .into_response()
        }
        Ok(HelperResponse { error, .. }) => {
            err_500(error.unwrap_or_else(|| "helper rejected ListTimezones".into()))
        }
        Err(e) => err_500(format!("helper unreachable: {e}")),
    }
}

/// Spawn the first-boot geoip → timezone task. No-op if system.toml
/// already carries a timezone. Logs at INFO on success, WARN on any
/// failure (network down, geoip rate-limited, helper unreachable) —
/// the system stays usable on UTC either way.
pub fn spawn_first_boot_geoip(state: crate::AppState) {
    tokio::spawn(async move {
        match read_system_config(&state).await {
            Ok(cfg) if cfg.system.timezone.is_some() => {
                tracing::debug!(
                    tz = ?cfg.system.timezone,
                    "system.toml already has a timezone; skipping geoip lookup"
                );
                return;
            }
            Ok(_) => {
                // No tz yet — proceed.
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not read system.toml; skipping geoip");
                return;
            }
        }

        let tz = match geoip_timezone().await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "geoip lookup failed; staying on UTC");
                return;
            }
        };

        // Helper SetTimezone now does timedatectl + system.toml persist
        // + bananas-dashboard restart in one shot, so a single call is
        // all this needs.
        let set_cmd = Command::SetTimezone { tz: tz.clone() };
        match bananas_helper::call(&state.helper_socket, &set_cmd).await {
            Ok(HelperResponse { ok: true, .. }) => {
                tracing::info!(tz = %tz, "first-boot timezone set via geoip");
            }
            Ok(HelperResponse { error, .. }) => {
                tracing::warn!(
                    tz = %tz, error = ?error,
                    "helper rejected geoip-derived timezone"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "helper unreachable for geoip apply");
            }
        }
    });
}

/// Shell out to `curl https://ipapi.co/timezone/`. Plain text response,
/// no API key, no JSON parsing. 5 s timeout so a flaky network can't
/// stall server startup. Returns the trimmed body or an error.
async fn geoip_timezone() -> Result<String, String> {
    let out = tokio::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "5", "https://ipapi.co/timezone/"])
        .output()
        .await
        .map_err(|e| format!("spawning curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl exit {} ({})",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let body = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // Sanity: ipapi.co returns the raw IANA name (e.g. "Europe/Madrid").
    // Reject anything that looks like an HTML error page or rate-limit
    // message before the helper sees it.
    if body.is_empty() || body.len() > 64 || body.contains(' ') || body.contains('<') {
        return Err(format!("unexpected geoip response: {body:?}"));
    }
    Ok(body)
}
