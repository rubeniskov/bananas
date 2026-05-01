//! `OperationKind::ConfigImport` handler.
//!
//! Wraps the multi-step config-bundle apply (exports → fstab → users
//! → cloud → dashboard.toml → system.toml) in an OperationManager-tracked
//! op. The helper does the privileged writes; the operation's `output`
//! buffer carries one progress line per step plus the final summary as
//! JSON, so a webadmin reconnecting mid-import sees the same log a
//! mid-import open of `/api/operations/{id}/log` would deliver.
//!
//! Unlike opkg / cloud sync, the work itself is in-process: there's no
//! off-server log file to tail. A server crash mid-import leaves the
//! Running op for `flush_orphan_running` to flip to Failure with the
//! `[interrupted]` note. That's intentional — the partial-import state
//! on disk (e.g., exports written but fstab not yet) is what the
//! operator gets, and the SPA notes via the journal that the previous
//! attempt didn't complete cleanly.

use std::path::PathBuf;

use axum::http::StatusCode;
use bananas_engine::{Command as HelperCommand, Response as HelperResponse};
use bananas_proto::engine::v1::{
    WriteExportsRequest, WriteFstabRequest, WriteServiceConfigRequest,
    engine_service_client::EngineServiceClient,
};
use serde_json::json;

use super::{OperationKind, OperationManager, OperationStatus};
use crate::config::{ConfigBundle, ImportSummary};
use crate::{exports, fstab};

/// Parse + validate the bundle, register the op, spawn the runner.
/// Returns 4xx if the body is bad TOML or carries an unsupported
/// version — those are caller errors that shouldn't pollute the op
/// journal. Any helper-side step failure is recorded inside the op
/// itself (`status = Failure` with notes) and surfaced to the SPA via
/// the SSE log.
pub async fn start(
    manager: OperationManager,
    helper_socket: PathBuf,
    helper_grpc_socket: PathBuf,
    body: String,
) -> Result<u64, (StatusCode, String)> {
    let bundle: ConfigBundle = toml::from_str(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid TOML: {e}")))?;
    if bundle.version > crate::config::default_version() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "unsupported config version {} (this server understands up to {})",
                bundle.version,
                crate::config::default_version()
            ),
        ));
    }

    let summary_label = format!(
        "Importing config: {} export(s), {} mount(s), {} user(s)",
        bundle.exports.len(),
        bundle.storage.len(),
        bundle.users.len(),
    );
    let op_id = manager
        .enqueue(OperationKind::ConfigImport, summary_label, json!({}))
        .await;

    let mgr = manager.clone();
    tokio::spawn(async move {
        run_import(mgr, helper_socket, helper_grpc_socket, op_id, bundle).await;
    });
    Ok(op_id)
}

/// The body of the legacy `import_config` handler, refactored to
/// stream progress through the OperationManager. Each step appends a
/// short status line; the final terminal call carries the
/// `ImportSummary` JSON for old clients that want the structured
/// shape.
async fn run_import(
    manager: OperationManager,
    helper_socket: PathBuf,
    helper_grpc_socket: PathBuf,
    op_id: u64,
    bundle: ConfigBundle,
) {
    let mut summary = ImportSummary::default();

    macro_rules! log {
        ($($arg:tt)*) => {{
            let line = format!($($arg)*);
            tracing::debug!(op_id, line=%line, "config-import step");
            manager.append_output(op_id, &format!("{line}\n")).await;
        }};
    }

    // ---- exports ---------------------------------------------------------
    log!("Applying {} export(s)…", bundle.exports.len());
    let export_rows: Vec<exports::Row> = bundle
        .exports
        .iter()
        .map(|e| exports::Row {
            path: e.path.clone(),
            host: e.host.clone(),
            options: e.options.clone(),
        })
        .collect();
    let exports_content = exports::serialize(&export_rows);
    match grpc_write_exports(&helper_grpc_socket, exports_content).await {
        Ok(()) => {
            summary.exports_written = export_rows.len();
            log!("  → exports written ({} row(s))", summary.exports_written);
        }
        Err(note) => {
            summary.ok = false;
            log!("  → exports FAILED: {note}");
            summary.notes.push(note);
        }
    }

    // ---- storage (mounts) -----------------------------------------------
    log!("Applying {} mount entr(y|ies)…", bundle.storage.len());
    let bundle_rows: Vec<fstab::Row> = bundle
        .storage
        .iter()
        .filter(|e| !fstab::is_protected_target(&e.mountpoint, &e.fstype, &e.source))
        .map(|e| fstab::Row {
            source: e.source.clone(),
            mountpoint: e.mountpoint.clone(),
            fstype: e.fstype.clone(),
            options: e.options.clone(),
            dump: e.dump,
            pass: e.pass,
        })
        .collect();
    let raw_fstab = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    let mut header = String::new();
    for line in raw_fstab.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            header.push_str(line);
            header.push('\n');
        } else {
            break;
        }
    }
    let protected_rows: Vec<fstab::Row> = fstab::rows(&raw_fstab)
        .into_iter()
        .filter(fstab::is_protected)
        .collect();
    let bundle_count = bundle_rows.len();
    let mut fstab_rows = protected_rows;
    fstab_rows.extend(bundle_rows);
    let fstab_content = format!("{}{}", header, fstab::serialize(&fstab_rows));
    match grpc_write_fstab(&helper_grpc_socket, fstab_content).await {
        Ok(()) => {
            summary.storage_written = bundle_count;
            log!(
                "  → storage written ({} user row(s))",
                summary.storage_written
            );
        }
        Err(note) => {
            summary.ok = false;
            log!("  → storage FAILED: {note}");
            summary.notes.push(note);
        }
    }

    // ---- users -----------------------------------------------------------
    log!("Restoring {} user account(s)…", bundle.users.len());
    for entry in &bundle.users {
        let Some(hash) = &entry.password_hash else {
            summary.users_skipped += 1;
            let note = format!("{}: skipped (no password_hash in backup)", entry.username);
            log!("  → {note}");
            summary.notes.push(note);
            continue;
        };
        let cmd = HelperCommand::CreateUser {
            username: entry.username.clone(),
            password: hash.clone(),
            full_name: entry.full_name.clone(),
            admin: entry.admin,
            password_is_hash: true,
        };
        match bananas_engine::call(&helper_socket, &cmd).await {
            Ok(HelperResponse { ok: true, .. }) => {
                summary.users_created += 1;
                log!("  → {} created", entry.username);
            }
            Ok(HelperResponse { error, .. }) => {
                summary.users_skipped += 1;
                let note = format!(
                    "{}: {}",
                    entry.username,
                    error.unwrap_or_else(|| "helper rejected create".into())
                );
                log!("  → {note}");
                summary.notes.push(note);
            }
            Err(e) => {
                summary.ok = false;
                summary.users_skipped += 1;
                let note = format!("{}: helper unreachable: {e}", entry.username);
                log!("  → {note}");
                summary.notes.push(note);
            }
        }
    }

    // ---- cloud -----------------------------------------------------------
    let cloud_toml = match toml::to_string_pretty(&bundle.cloud) {
        Ok(s) => s,
        Err(e) => {
            summary.ok = false;
            log!("cloud: serialize failed: {e}");
            summary.notes.push(format!("cloud: serialize failed: {e}"));
            String::new()
        }
    };
    if !cloud_toml.is_empty() {
        let accounts_count = bundle
            .cloud
            .get("accounts")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let syncs_count = bundle
            .cloud
            .get("syncs")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        log!("Restoring cloud config ({accounts_count} account(s), {syncs_count} sync(s))…");
        match write_service_toml(&helper_grpc_socket, "cloud", cloud_toml).await {
            Ok(()) => {
                summary.cloud_accounts = accounts_count;
                summary.cloud_syncs = syncs_count;
                log!("  → cloud config written");
                // Install-to-activate hint: if the operator imported a
                // bundle with a cloud section but bananas-cloud isn't
                // installed yet, the file is preserved on disk for a
                // future `opkg install bananas-cloud` to pick up — but
                // /api/cloud/* will 404 until that install lands.
                // Surface the hint in the import summary so the
                // operator isn't surprised.
                if !cloud_extension_installed() {
                    let hint = "cloud config preserved on disk; run \
                         `opkg install bananas-cloud` to activate"
                        .to_string();
                    log!("  → {hint}");
                    summary.notes.push(hint);
                }
            }
            Err(note) => {
                summary.ok = false;
                log!("  → cloud FAILED: {note}");
                summary.notes.push(note);
            }
        }
    }

    // ---- service config tables (system / dashboard / stats) -------------
    for (name, table) in [
        ("system", &bundle.system),
        ("dashboard", &bundle.dashboard),
        ("stats", &bundle.stats),
    ] {
        if table.is_empty() {
            continue;
        }
        let serialized = match toml::to_string_pretty(table) {
            Ok(s) => s,
            Err(e) => {
                summary.ok = false;
                let note = format!("{name}: serialize failed: {e}");
                log!("{note}");
                summary.notes.push(note);
                continue;
            }
        };
        log!("Restoring {name}.toml…");
        match write_service_toml(&helper_grpc_socket, name, serialized).await {
            Ok(()) => {
                summary.notes.push(format!("{name}.toml restored"));
                log!("  → {name}.toml restored");
            }
            Err(note) => {
                summary.ok = false;
                log!("  → {name} FAILED: {note}");
                summary.notes.push(note);
            }
        }
    }

    // Final terminal status. The summary JSON is appended verbatim so
    // older clients (or someone fetching /api/operations/{id}) still
    // see the structured shape they used to get from the synchronous
    // POST response.
    let summary_json = serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".to_string());
    let final_status = if summary.ok {
        OperationStatus::Success
    } else {
        OperationStatus::Failure
    };
    manager
        .finish(
            op_id,
            final_status,
            Some(format!("--- summary ---\n{summary_json}")),
        )
        .await;
}

async fn write_service_toml(
    helper_grpc_socket: &PathBuf,
    name: &str,
    content: String,
) -> Result<(), String> {
    let channel = crate::engine_grpc::channel(helper_grpc_socket)
        .await
        .map_err(|e| format!("{name}: helper unreachable: {e}"))?;
    let mut client = EngineServiceClient::new(channel);
    match client
        .write_service_config(WriteServiceConfigRequest {
            name: name.into(),
            content,
        })
        .await
    {
        Ok(_) => Ok(()),
        Err(status) => Err(format!("{name}: {status}")),
    }
}

async fn grpc_write_exports(helper_grpc_socket: &PathBuf, content: String) -> Result<(), String> {
    let channel = crate::engine_grpc::channel(helper_grpc_socket)
        .await
        .map_err(|e| format!("exports: helper unreachable: {e}"))?;
    let mut client = EngineServiceClient::new(channel);
    client
        .write_exports(WriteExportsRequest { content })
        .await
        .map(|_| ())
        .map_err(|status| format!("exports: {status}"))
}

async fn grpc_write_fstab(helper_grpc_socket: &PathBuf, content: String) -> Result<(), String> {
    let channel = crate::engine_grpc::channel(helper_grpc_socket)
        .await
        .map_err(|e| format!("storage: helper unreachable: {e}"))?;
    let mut client = EngineServiceClient::new(channel);
    client
        .write_fstab(WriteFstabRequest { content })
        .await
        .map(|_| ())
        .map_err(|status| format!("storage: {status}"))
}

/// Quick filesystem probe — does /etc/bananas/extensions.d/cloud.toml
/// exist? bananas-cloud's IPK drops it on install and removes it on
/// uninstall, so this is the canonical "is the cloud plugin
/// installed?" check. Cheap (one `stat`); no helper round-trip.
fn cloud_extension_installed() -> bool {
    let dir: std::path::PathBuf = std::env::var_os("BANANAS_EXTENSIONS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/etc/bananas/extensions.d".into());
    dir.join("cloud.toml").exists()
}
