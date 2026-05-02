//! opkg-backed update endpoints.
//!
//! Now backed by the unified `OperationManager` (`crates/webadmin/src/operations`).
//! The webadmin's *Upgrade* button goes through `POST /api/updates/install`,
//! which calls `operations::opkg::start` to:
//!   1. validate package names (must start with `bananas-`)
//!   2. tell the helper to spawn the transient `bananas-opkg-upgrade.service`
//!   3. register an `OpkgUpgrade` op in the manager
//!   4. spawn a background watcher that polls the helper for log delta
//!
//! `GET /api/updates/status` is kept as a back-compat shim for SPA
//! bundles cached in browsers; it just locates the latest OpkgUpgrade
//! op and tails it via `/api/operations/{id}/log`. New clients should
//! use the operations endpoints directly.
//!
//!   GET  /api/version          — installed `bananas-*` packages
//!   GET  /api/updates/check    — upgradable `bananas-*` packages
//!   POST /api/updates/install  — kick off `opkg upgrade <packages>`
//!   GET  /api/updates/status   — SSE stream of the active install
//!                                (legacy shim → operations/{id}/log)
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
use bananas_proto::engine::v1::{
    OpkgListInstalledRequest, OpkgListUpgradableRequest, OpkgUpdateRequest,
    engine_service_client::EngineServiceClient,
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};

use crate::{AppState, engine_grpc, operations};

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
    let channel = engine_grpc::channel(&state.helper_grpc_socket).await.ok()?;
    let mut client = EngineServiceClient::new(channel);
    match client
        .opkg_list_installed(OpkgListInstalledRequest {})
        .await
    {
        Ok(resp) => Some(
            resp.into_inner()
                .packages
                .into_iter()
                .map(|p| InstalledPackage {
                    name: p.name,
                    version: p.version,
                })
                .collect(),
        ),
        Err(status) => {
            tracing::warn!(error = %status, "opkg list-installed failed");
            None
        }
    }
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
    let channel = engine_grpc::channel(&state.helper_grpc_socket)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("helper unreachable: {e}")))?;
    let mut client = EngineServiceClient::new(channel);

    // Refresh the metadata cache; failures (offline feed, rate
    // limit, DNS hiccup) are non-fatal — list_upgradable still
    // works against whatever's cached, and the SPA shows the
    // soft-error banner alongside the (possibly stale) list.
    let mut error: Option<String> = None;
    if let Err(status) = client.opkg_update(OpkgUpdateRequest {}).await {
        error = Some(format!("opkg update: {status}"));
    }

    let upgradable = client
        .opkg_list_upgradable(OpkgListUpgradableRequest {})
        .await
        .map_err(|status| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("opkg list-upgradable: {status}"),
            )
        })?
        .into_inner()
        .packages;

    let mut packages: Vec<UpgradablePackage> = upgradable
        .into_iter()
        .filter(|p| p.name.starts_with(PACKAGE_PREFIX))
        .map(|p| UpgradablePackage {
            name: p.name,
            installed: p.installed,
            candidate: p.candidate,
        })
        .collect();
    packages.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Json(UpdatesCheckResponse { packages, error }))
}

// ─── POST /api/updates/install + SSE stream ─────────────────────────

/// SSE event payload for the legacy `/api/updates/status` shim. Phase
/// is one of `install` (mid-stream log line), `done`, `error`. The
/// webadmin still matches these strings to drive the install modal.
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

#[derive(Debug, Serialize)]
pub struct InstallAccepted {
    /// New clients can subscribe to `/api/operations/{op_id}/log`.
    /// Old clients ignore this field.
    pub op_id: u64,
}

pub async fn post_updates_install(
    State(state): State<AppState>,
    Json(req): Json<UpgradeRequest>,
) -> Result<(StatusCode, Json<InstallAccepted>), (StatusCode, String)> {
    let op_id = operations::opkg::start(
        state.operations.clone(),
        (*state.helper_grpc_socket).clone(),
        req.packages,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(InstallAccepted { op_id })))
}

/// Legacy SSE shim. Locates the latest OpkgUpgrade op and emits its
/// log incrementally + the terminal close event in the same shape the
/// previous handler used. New webadmin builds bypass this entirely
/// and subscribe to `/api/operations/{id}/log`.
pub async fn get_updates_status(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let manager = state.operations.clone();
    let stream = async_stream::stream! {
        let mut seq: u64 = 0;
        let mut sent_len: usize = 0;

        // Find the op to tail. If none ever ran, emit a terminal error.
        let op_id = match operations::opkg::latest(&manager).await {
            Some(op) => op.id,
            None => {
                yield emit(&mut seq, "error", "no upgrade has been started".into());
                yield close_event("error");
                return;
            }
        };

        loop {
            let op = match manager.get(op_id).await {
                Some(o) => o,
                None => break,
            };
            if op.output.len() > sent_len {
                let chunk = &op.output[sent_len..];
                sent_len = op.output.len();
                for line in chunk.lines() {
                    if line.is_empty() || line.starts_with("[bananas-opkg-exit=") {
                        continue;
                    }
                    yield emit(&mut seq, "install", line.to_string());
                }
            }
            match op.status {
                crate::operations::OperationStatus::Running => {
                    tokio::time::sleep(STATUS_POLL_INTERVAL).await;
                }
                crate::operations::OperationStatus::Success => {
                    yield emit(&mut seq, "done", "Upgrade complete.".into());
                    yield close_event("ok");
                    break;
                }
                crate::operations::OperationStatus::Failure
                | crate::operations::OperationStatus::Cancelled => {
                    let tail = op
                        .output
                        .lines()
                        .last()
                        .filter(|l| !l.starts_with("[bananas-opkg-exit="))
                        .unwrap_or("upgrade failed");
                    yield emit(&mut seq, "error", tail.to_string());
                    yield close_event("error");
                    break;
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
