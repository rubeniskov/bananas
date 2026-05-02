//! `OperationKind::OpkgUpgrade` handler.
//!
//! Authoritative state lives on the helper side at
//! `/var/lib/bananas-engine/opkg.log`; the server's role is to expose
//! that work to the webadmin via the unified OperationManager surface.
//!
//! Two entry points:
//!
//! * `start(...)` — called by `POST /api/updates/install`. Asks the
//!   helper to spawn opkg, registers an op in the manager, and spawns
//!   a background watcher task that polls the helper for log delta
//!   and finishes the op when the transient unit terminates.
//! * `reinstate(...)` — called once at server startup. If the helper
//!   reports state == "active", finds the most recent Running
//!   OpkgUpgrade op in the journal (or creates a new one if none
//!   exists, e.g. a wiped journal) and reattaches a watcher. Without
//!   this, a server self-restart mid-upgrade would leave the journal
//!   showing Failure even though the upgrade is still progressing.

use std::path::PathBuf;
use std::time::Duration;

use bananas_proto::engine::v1::{
    OpkgUpgradeRequest, OpkgUpgradeStatusRequest, engine_service_client::EngineServiceClient,
};
use serde_json::json;

use super::{OperationKind, OperationManager, OperationState, OperationStatus};
use crate::engine_grpc;

const POLL: Duration = Duration::from_millis(500);
const PACKAGE_PREFIX: &str = "bananas-";

/// Locally-typed mirror of the gRPC `OpkgUpgradeStatusResponse`,
/// with `exit_code` re-collapsed to `Option<i32>` so the rest of
/// the watcher reads the same way it did under the JSON path.
struct UpgradeStatus {
    state: String,
    log: String,
    log_offset: u64,
    exit_code: Option<i32>,
}

/// Validate + dispatch an `opkg upgrade <packages>` through the
/// helper, register an op in the manager, and spawn the watcher.
/// Returns the new op_id immediately.
pub async fn start(
    manager: OperationManager,
    helper_grpc_socket: PathBuf,
    packages: Vec<String>,
) -> Result<u64, (axum::http::StatusCode, String)> {
    use axum::http::StatusCode;
    if packages.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no packages specified".into()));
    }
    for p in &packages {
        if !p.starts_with(PACKAGE_PREFIX) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("package {p:?} is not in the {PACKAGE_PREFIX}* allowlist"),
            ));
        }
    }

    let channel = engine_grpc::channel(&helper_grpc_socket)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("helper unreachable: {e}")))?;
    let mut client = EngineServiceClient::new(channel);
    if let Err(status) = client
        .opkg_upgrade(OpkgUpgradeRequest {
            packages: packages.clone(),
        })
        .await
    {
        // FailedPrecondition = a unit is already active. Map to
        // 409 Conflict so the SPA's existing handler keeps working.
        let code = if status.code() == tonic::Code::FailedPrecondition {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_GATEWAY
        };
        return Err((code, status.message().to_string()));
    }

    let label = format!("Upgrading {} package(s)", packages.len());
    let op_id = manager
        .enqueue(
            OperationKind::OpkgUpgrade,
            label,
            json!({ "packages": packages }),
        )
        .await;

    spawn_watcher(manager, helper_grpc_socket, op_id, 0);
    Ok(op_id)
}

/// On startup, ask the helper if an upgrade is still active. If yes,
/// reattach a watcher to the matching journal entry so the SPA's
/// `/api/operations/active` view picks it up cleanly. If no, do
/// nothing — `flush_orphan_running` will mop up any Running
/// OpkgUpgrade in the journal.
pub async fn reinstate(manager: &OperationManager, helper_grpc_socket: &PathBuf) {
    let status = match poll_status(helper_grpc_socket, 0).await {
        Some(s) => s,
        None => {
            tracing::debug!("helper unreachable at startup; skipping opkg reinstate");
            return;
        }
    };
    if status.state != "active" {
        return;
    }

    let existing = manager
        .most_recent_running_of(OperationKind::OpkgUpgrade)
        .await;
    let (op_id, since) = match existing {
        Some(op) => {
            tracing::info!(op_id=%op.id, "reattaching to running opkg upgrade after restart");
            (op.id, op.output.len() as u64)
        }
        None => {
            tracing::info!(
                "helper has an opkg upgrade in flight but no journal entry — creating a fresh op"
            );
            let id = manager
                .enqueue(
                    OperationKind::OpkgUpgrade,
                    "Upgrading (reattached after restart)".into(),
                    json!({}),
                )
                .await;
            (id, 0)
        }
    };

    spawn_watcher(manager.clone(), helper_grpc_socket.clone(), op_id, since);
}

/// Single helper that dials the engine's gRPC socket and pulls the
/// upgrade status. Returns `None` on transport / RPC errors —
/// callers decide whether to retry or fail loudly.
async fn poll_status(helper_grpc_socket: &PathBuf, since: u64) -> Option<UpgradeStatus> {
    let channel = engine_grpc::channel(helper_grpc_socket).await.ok()?;
    let mut client = EngineServiceClient::new(channel);
    let resp = client
        .opkg_upgrade_status(OpkgUpgradeStatusRequest { since })
        .await
        .ok()?
        .into_inner();
    Some(UpgradeStatus {
        state: resp.state,
        log: resp.log,
        log_offset: resp.log_offset,
        exit_code: resp.has_exit_code.then_some(resp.exit_code),
    })
}

/// Background task that polls helper.OpkgUpgradeStatus, appends new
/// log bytes to the op via the manager, and calls `finish()` when the
/// transient unit terminates. Survives helper unavailability windows
/// (server restarts the helper as part of the upgrade itself) by
/// retrying with a small backoff.
fn spawn_watcher(
    manager: OperationManager,
    helper_grpc_socket: PathBuf,
    op_id: u64,
    mut since: u64,
) {
    tokio::spawn(async move {
        // Track consecutive helper-unreachable polls so we can give up
        // eventually rather than poll forever if the helper has gone
        // away for good (e.g. it was uninstalled during the upgrade —
        // shouldn't happen, but defence in depth).
        let mut consecutive_fail: u32 = 0;
        const MAX_CONSEC_FAIL: u32 = 240; // ~2 min at 500 ms

        // Tolerate transient `idle` for ~5 s after spawn — the helper
        // seeds the log file before invoking `systemd-run --no-block`,
        // but if systemd-run is slow to acknowledge (or the seed write
        // races the watcher's very first poll on a slow disk), state
        // can still briefly read as idle. Only fail after the
        // tolerance budget is exhausted; a real desync stays idle
        // indefinitely.
        let mut idle_polls: u32 = 0;
        const MAX_IDLE_POLLS: u32 = 20; // ~10 s at 500 ms

        loop {
            let status = match poll_status(&helper_grpc_socket, since).await {
                Some(s) => {
                    consecutive_fail = 0;
                    s
                }
                None => {
                    consecutive_fail += 1;
                    if consecutive_fail >= MAX_CONSEC_FAIL {
                        manager
                            .finish(
                                op_id,
                                OperationStatus::Failure,
                                Some("helper unreachable for >2 min, giving up".into()),
                            )
                            .await;
                        return;
                    }
                    tokio::time::sleep(POLL).await;
                    continue;
                }
            };

            if !status.log.is_empty() {
                // Strip the wrapper-script sentinel before recording —
                // it leaks helper internals to the UI otherwise.
                let cleaned: String = status
                    .log
                    .lines()
                    .filter(|l| !l.starts_with("[bananas-opkg-exit="))
                    .fold(String::new(), |mut acc, l| {
                        acc.push_str(l);
                        acc.push('\n');
                        acc
                    });
                if !cleaned.is_empty() {
                    manager.append_output(op_id, &cleaned).await;
                }
            }
            since = status.log_offset;

            match status.state.as_str() {
                "done" => {
                    manager
                        .finish(
                            op_id,
                            OperationStatus::Success,
                            Some("Upgrade complete.".into()),
                        )
                        .await;
                    return;
                }
                "failed" => {
                    let summary = status
                        .exit_code
                        .map(|c| format!("opkg exited {c}"))
                        .unwrap_or_else(|| "opkg failed".into());
                    manager
                        .finish(op_id, OperationStatus::Failure, Some(summary))
                        .await;
                    return;
                }
                "idle" => {
                    idle_polls += 1;
                    if idle_polls >= MAX_IDLE_POLLS {
                        // Sustained idle: real server-helper desync,
                        // not just a startup race. Fail loudly so the
                        // UI shows it.
                        manager
                            .finish(
                                op_id,
                                OperationStatus::Failure,
                                Some(format!(
                                    "helper reports no upgrade in progress after {} polls",
                                    idle_polls
                                )),
                            )
                            .await;
                        return;
                    }
                    tokio::time::sleep(POLL).await;
                }
                _ => {
                    // "active" — keep polling. Reset the idle budget
                    // because we're no longer in the startup-race
                    // window: any later transition back to idle would
                    // be a real desync.
                    idle_polls = 0;
                    tokio::time::sleep(POLL).await;
                }
            }
        }
    });
}

/// Find the latest OpkgUpgrade op in the journal — running first,
/// otherwise the most recently finished. Used by the legacy
/// `/api/updates/status` endpoint to figure out which op to tail when
/// no id was provided. Returns None if no OpkgUpgrade has ever run.
pub async fn latest(manager: &OperationManager) -> Option<OperationState> {
    if let Some(op) = manager
        .most_recent_running_of(OperationKind::OpkgUpgrade)
        .await
    {
        return Some(op);
    }
    manager
        .list()
        .await
        .into_iter()
        .rev()
        .find(|op| op.kind == OperationKind::OpkgUpgrade)
}
