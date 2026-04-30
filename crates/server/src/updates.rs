//! opkg-backed update endpoints.
//!
//! The server is a thin proxy onto `bananas-helper`'s opkg surface.
//! It owns no install state of its own — that all lives behind the
//! helper, on disk in `/var/lib/bananas-helper/opkg.log`. This is
//! deliberate: the server gets restarted as part of the upgrade it
//! initiated, so any in-process state would be lost. Polling the
//! helper instead means the SSE stream resumes naturally after the
//! restart.
//!
//!   GET  /api/version          — installed `bananas-*` packages
//!   GET  /api/updates/check    — upgradable `bananas-*` packages
//!   POST /api/updates/install  — kick off `opkg upgrade <packages>`
//!   GET  /api/updates/status   — SSE stream of the active install
//!
//! Filtering is on `bananas-*` prefix: opkg's view of the system
//! includes ~1000 base-OS packages (libc, busybox, …) that the
//! webadmin doesn't manage. Operators who want raw opkg can SSH in.

use std::{convert::Infallible, time::Duration};

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
};
use bananas_helper::{Command as HelperCommand, Response as HelperResponse};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};

use crate::AppState;

/// Only packages whose name starts with this prefix are exposed via
/// the API. Keeps the SPA's view focused on what BanaNAS itself
/// ships and avoids surfacing every libc / busybox upgrade.
const PACKAGE_PREFIX: &str = "bananas-";

/// How often the SSE handler polls the helper for new log content
/// while the upgrade is active. opkg writes maybe 1–3 lines per
/// package step, so 500 ms is plenty smooth without being chatty.
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(500);

// ─── /api/version ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct InstalledVersionsResponse {
    pub packages: Vec<InstalledPackage>,
}

pub async fn get_version(State(state): State<AppState>) -> Json<InstalledVersionsResponse> {
    let mut packages = installed_packages(&state).await.unwrap_or_default();
    packages.retain(|p| p.name.starts_with(PACKAGE_PREFIX));
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Json(InstalledVersionsResponse { packages })
}

async fn installed_packages(state: &AppState) -> Option<Vec<InstalledPackage>> {
    let resp = bananas_helper::call(&state.helper_socket, &HelperCommand::OpkgListInstalled)
        .await
        .ok()?;
    if !resp.ok {
        tracing::warn!(error=?resp.error, "opkg list-installed failed");
        return None;
    }
    serde_json::from_str(&resp.output).ok()
}

// ─── /api/updates/check ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradablePackage {
    pub name: String,
    pub installed: String,
    pub candidate: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdatesCheckResponse {
    pub packages: Vec<UpgradablePackage>,
    /// Set when the upstream `opkg update` failed (offline feed,
    /// rate-limited, DNS hiccup). The list-upgradable still runs
    /// against cached metadata, so `packages` may be populated even
    /// when `error` is set.
    pub error: Option<String>,
}

pub async fn get_updates_check(
    State(state): State<AppState>,
) -> Result<Json<UpdatesCheckResponse>, (StatusCode, String)> {
    let mut error: Option<String> = None;
    if let Err(e) = run_helper_simple(&state, HelperCommand::OpkgUpdate).await {
        error = Some(format!("opkg update: {e}"));
    }

    let upgradable: Vec<UpgradablePackage> = match bananas_helper::call(
        &state.helper_socket,
        &HelperCommand::OpkgListUpgradable,
    )
    .await
    {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => serde_json::from_str(&output).unwrap_or_default(),
        Ok(HelperResponse { error: e, .. }) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "opkg list-upgradable: {}",
                    e.unwrap_or_else(|| "unknown error".into())
                ),
            ));
        }
        Err(e) => {
            return Err((StatusCode::BAD_GATEWAY, format!("helper unreachable: {e}")));
        }
    };

    let mut packages: Vec<UpgradablePackage> = upgradable
        .into_iter()
        .filter(|p| p.name.starts_with(PACKAGE_PREFIX))
        .collect();
    packages.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Json(UpdatesCheckResponse { packages, error }))
}

// ─── POST /api/updates/install + SSE stream ─────────────────────────

/// SSE event payload. Phase is one of `install` (mid-stream log line),
/// `done` (final OK), `error` (final failure). The webadmin matches on
/// these strings to decide when to close the modal.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub seq: u64,
    pub phase: &'static str,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct UpgradeRequest {
    /// One or more package names. Must each start with the
    /// `bananas-` prefix; anything else is rejected so the API can't
    /// be used to opkg-upgrade arbitrary system packages.
    pub packages: Vec<String>,
}

/// Helper status payload — mirrors `bananas_helper::opkg::UpgradeStatus`
/// without dragging the helper crate's serde shape into the server
/// surface. State strings come from the helper unchanged.
#[derive(Debug, Deserialize)]
struct UpgradeStatus {
    state: String,
    log: String,
    log_offset: u64,
    #[serde(default)]
    exit_code: Option<i32>,
}

pub async fn post_updates_install(
    State(state): State<AppState>,
    Json(req): Json<UpgradeRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if req.packages.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no packages specified".to_string()));
    }
    for p in &req.packages {
        if !p.starts_with(PACKAGE_PREFIX) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("package {p:?} is not in the {PACKAGE_PREFIX}* allowlist"),
            ));
        }
    }

    let cmd = HelperCommand::OpkgUpgrade {
        packages: req.packages.clone(),
    };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse { ok: true, .. }) => Ok(StatusCode::ACCEPTED),
        Ok(HelperResponse { error, .. }) => Err((
            StatusCode::CONFLICT,
            error.unwrap_or_else(|| "helper rejected upgrade".into()),
        )),
        Err(e) => Err((StatusCode::BAD_GATEWAY, format!("helper unreachable: {e}"))),
    }
}

pub async fn get_updates_status(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        let mut since: u64 = 0;
        let mut seq: u64 = 0;
        let socket = state.helper_socket.clone();

        loop {
            let cmd = HelperCommand::OpkgUpgradeStatus { since };
            let status = match bananas_helper::call(&socket, &cmd).await {
                Ok(HelperResponse { ok: true, output, .. }) => {
                    match serde_json::from_str::<UpgradeStatus>(&output) {
                        Ok(s) => s,
                        Err(e) => {
                            yield emit(&mut seq, "error", format!("malformed helper status: {e}"));
                            yield close_event("error");
                            break;
                        }
                    }
                }
                Ok(HelperResponse { error, .. }) => {
                    // Likely transient: helper is restarting too. Keep
                    // polling — the webadmin's higher-level retry
                    // bound covers the case where it never comes back.
                    yield keepalive_comment();
                    tokio::time::sleep(STATUS_POLL_INTERVAL).await;
                    let _ = error;
                    continue;
                }
                Err(e) => {
                    yield keepalive_comment();
                    tokio::time::sleep(STATUS_POLL_INTERVAL).await;
                    tracing::debug!(?e, "helper unreachable during status poll, retrying");
                    continue;
                }
            };

            since = status.log_offset;

            for line in status.log.lines() {
                if line.is_empty() {
                    continue;
                }
                if line.starts_with("[bananas-opkg-exit=") {
                    // Sentinel from the wrapper script — don't surface to the UI.
                    continue;
                }
                yield emit(&mut seq, "install", line.to_string());
            }

            match status.state.as_str() {
                "done" => {
                    yield emit(&mut seq, "done", "Upgrade complete.".to_string());
                    yield close_event("ok");
                    break;
                }
                "failed" => {
                    let detail = status
                        .exit_code
                        .map(|c| format!("opkg exited {c}"))
                        .unwrap_or_else(|| "opkg failed".to_string());
                    yield emit(&mut seq, "error", detail);
                    yield close_event("error");
                    break;
                }
                "idle" => {
                    yield emit(&mut seq, "error", "no upgrade in progress".to_string());
                    yield close_event("error");
                    break;
                }
                _ => {
                    // "active" — keep polling for new log content
                    tokio::time::sleep(STATUS_POLL_INTERVAL).await;
                }
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn emit(seq: &mut u64, phase: &'static str, text: String) -> Result<Event, Infallible> {
    let line = LogLine {
        seq: *seq,
        phase,
        text,
    };
    *seq += 1;
    Ok(Event::default()
        .json_data(&line)
        .unwrap_or_else(|_| Event::default()))
}

fn close_event(payload: &'static str) -> Result<Event, Infallible> {
    Ok(Event::default().event("close").data(payload))
}

fn keepalive_comment() -> Result<Event, Infallible> {
    // SSE comment line — keeps the connection warm without delivering
    // a message to onmessage. Fine for opaque retry windows.
    Ok(Event::default().comment("retry"))
}

// ─── helpers ────────────────────────────────────────────────────────

async fn run_helper_simple(state: &AppState, cmd: HelperCommand) -> anyhow::Result<String> {
    let resp = bananas_helper::call(&state.helper_socket, &cmd).await?;
    if !resp.ok {
        anyhow::bail!(resp.error.unwrap_or_else(|| "helper rejected".into()));
    }
    Ok(resp.output)
}
