//! Backup/restore the BanaNAS admin config as a single TOML document.
//!
//! Export captures the *current* state of /etc/exports, /etc/fstab, and
//! the regular (UID >= 1000, non-`bananas`) users. Import applies
//! exports + fstab via the helper. User entries round-trip in the
//! payload but are NOT recreated on import — `/etc/shadow` hashes are
//! intentionally excluded from the backup, so we can't restore working
//! accounts. The import response surfaces a "users skipped" note so
//! the operator knows to recreate them via the Users page.

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bananas_proto::engine::v1::{ExportUsersRequest, engine_service_client::EngineServiceClient};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{AppState, exports, fstab};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ConfigBundle {
    /// Schema version. Bump when the layout changes incompatibly so
    /// older importers can refuse instead of silently dropping fields.
    #[serde(default = "default_version")]
    pub version: u32,
    /// NFS exports (mirrors /etc/exports rows).
    #[serde(default)]
    pub exports: Vec<ExportEntry>,
    /// Mount entries (mirrors /etc/fstab rows; named after the UI tab
    /// that owns them rather than the file format).
    #[serde(default)]
    pub storage: Vec<StorageEntry>,
    /// User accounts (UID >= 1000, non-system).
    #[serde(default)]
    pub users: Vec<UserEntry>,
    /// Per-section TOML config tables. Each key matches a UI tab and
    /// round-trips the file at /etc/bananas/<key>.toml. Stored as
    /// opaque tables — the consuming daemon owns the schema; webadmin
    /// only ferries the bytes so a "Save config / Load config" cycle
    /// preserves them across reflashes (even for plugins that aren't
    /// installed yet, like bananas-cloud).
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub system: toml::Table,
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub dashboard: toml::Table,
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub stats: toml::Table,
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub cloud: toml::Table,
}

/// Current bundle schema version.
///
/// v2 (May 2026): all per-tab config files (system / dashboard / stats
/// / cloud) round-trip as parsed `toml::Table` blocks under their
/// matching key, replacing the v1-era `*_toml = "raw string"` fields.
/// Bumped when fstab → storage is introduced as a structural rename.
/// Mount/export rows still round-trip as their own typed arrays
/// because their schemas are stable and benefit from validation.
pub fn default_version() -> u32 {
    2
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportEntry {
    pub path: String,
    pub host: String,
    pub options: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StorageEntry {
    pub source: String,
    pub mountpoint: String,
    pub fstype: String,
    pub options: String,
    #[serde(default)]
    pub dump: u32,
    #[serde(default)]
    pub pass: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserEntry {
    pub username: String,
    #[serde(default)]
    pub full_name: Option<String>,
    pub uid: u32,
    pub gid: u32,
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub locked: bool,
    /// Shadow-style hash (`$6$salt$digest`). Present in exports, optional
    /// in imports. Treat the file containing this as sensitive as
    /// /etc/shadow itself — anyone with the hash can attempt offline
    /// brute-force.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
}

pub async fn export_config(State(state): State<AppState>) -> Response {
    let bundle = match build_bundle(&state).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e })),
            )
                .into_response();
        }
    };
    let body = match toml::to_string_pretty(&bundle) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("toml serialize: {e}") })),
            )
                .into_response();
        }
    };
    let filename = format!("bananas-config-{}.toml", chrono_compact_now());
    (
        StatusCode::OK,
        [
            (
                "content-type",
                "application/toml; charset=utf-8".to_string(),
            ),
            (
                "content-disposition",
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response()
}

#[derive(Debug, Serialize)]
pub struct ImportSummary {
    pub ok: bool,
    pub exports_written: usize,
    pub storage_written: usize,
    pub users_created: usize,
    pub users_skipped: usize,
    pub cloud_accounts: usize,
    pub cloud_syncs: usize,
    pub notes: Vec<String>,
}

impl Default for ImportSummary {
    fn default() -> Self {
        // ok=true is the optimistic baseline — the runner flips it to
        // false on the first step that errors. Avoids an "ok=false at
        // start" gotcha for any new caller.
        Self {
            ok: true,
            exports_written: 0,
            storage_written: 0,
            users_created: 0,
            users_skipped: 0,
            cloud_accounts: 0,
            cloud_syncs: 0,
            notes: Vec::new(),
        }
    }
}

pub async fn import_config(State(state): State<AppState>, body: String) -> Response {
    // POST is now async: parse + validate happen synchronously (so a
    // bad bundle 400s immediately), the actual helper-driven apply
    // runs as an OperationManager-tracked op so the SPA can resume
    // progress UI across browser refreshes / server restarts.
    match crate::operations::config_import::start(
        state.operations.clone(),
        (*state.helper_grpc_socket).clone(),
        body,
    )
    .await
    {
        Ok(op_id) => (
            StatusCode::ACCEPTED,
            Json(json!({ "ok": true, "op_id": op_id })),
        )
            .into_response(),
        Err((status, msg)) => (status, Json(json!({ "ok": false, "error": msg }))).into_response(),
    }
}

async fn build_bundle(state: &AppState) -> Result<ConfigBundle, String> {
    // Read the canonical /etc/exports + /etc/fstab paths directly;
    // the per-feature plugins own their own writes via
    // bananas-engine, but the host's config-export bundle still
    // needs to slurp the rows for the TOML dump.
    let exports_raw = std::fs::read_to_string("/etc/exports").unwrap_or_default();
    let exports_rows = exports::rows(&exports_raw)
        .into_iter()
        .map(|r| ExportEntry {
            path: r.path,
            host: r.host,
            options: r.options,
        })
        .collect();

    // Skip protected system mounts in the backup — `/`, `/proc`, `/sys`,
    // … aren't user-managed config and a restored backup should never
    // try to overwrite them. Importer enforces the same filter, so the
    // bundle is the user-config slice on both ends.
    let fstab_raw = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    let storage_rows = fstab::rows(&fstab_raw)
        .into_iter()
        .filter(|r| !fstab::is_protected(r))
        .map(|r| StorageEntry {
            source: r.source,
            mountpoint: r.mountpoint,
            fstype: r.fstype,
            options: r.options,
            dump: r.dump,
            pass: r.pass,
        })
        .collect();

    // Users via the engine — ExportUsers includes shadow hashes so the
    // backup actually round-trips a working account. /api/users continues
    // to use ListUsers, which omits hashes.
    let users = {
        let channel = crate::engine_grpc::channel(&state.helper_grpc_socket)
            .await
            .map_err(|e| format!("helper unreachable: {e}"))?;
        let mut client = EngineServiceClient::new(channel);
        let resp = client
            .export_users(ExportUsersRequest {})
            .await
            .map_err(|status| format!("export_users: {status}"))?;
        parse_users_payload(&resp.into_inner().users_json)
    };

    // Service config files — read via the helper so root-owned files
    // are accessible to the unprivileged webadmin. Each is parsed into
    // a toml::Table; an absent / empty / unparseable file becomes an
    // empty table that serializes out via skip_serializing_if so the
    // resulting bundle stays minimal.
    let system = read_service_table(state, "system").await;
    let dashboard = read_service_table(state, "dashboard").await;
    let stats = read_service_table(state, "stats").await;
    let cloud = read_service_table(state, "cloud").await;

    Ok(ConfigBundle {
        version: default_version(),
        exports: exports_rows,
        storage: storage_rows,
        users,
        system,
        dashboard,
        stats,
        cloud,
    })
}

async fn read_service_table(state: &AppState, name: &str) -> toml::Table {
    let channel = match crate::engine_grpc::channel(&state.helper_grpc_socket).await {
        Ok(c) => c,
        Err(_) => return toml::Table::new(),
    };
    let mut client =
        bananas_proto::engine::v1::engine_service_client::EngineServiceClient::new(channel);
    match client
        .read_service_config(bananas_proto::engine::v1::ReadServiceConfigRequest {
            name: name.to_string(),
        })
        .await
    {
        Ok(resp) => {
            let content = resp.into_inner().content;
            if content.trim().is_empty() {
                toml::Table::new()
            } else {
                toml::from_str::<toml::Table>(&content).unwrap_or_default()
            }
        }
        Err(_) => toml::Table::new(),
    }
}

/// Parse the helper's ListUsers JSON, filter to "human" accounts (UID
/// >= 1000, excluding the bananas service user and OE's `nobody`), and
/// return as TOML-friendly UserEntry records.
fn parse_users_payload(output: &str) -> Vec<UserEntry> {
    let value: serde_json::Value = match serde_json::from_str(output) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let list = match value.get("users").and_then(|u| u.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };
    list.iter()
        .filter_map(|u| {
            let name = u.get("name").and_then(|v| v.as_str())?.to_string();
            let uid = u.get("uid").and_then(|v| v.as_u64())? as u32;
            let gid = u.get("gid").and_then(|v| v.as_u64())? as u32;
            if uid < 1000 || matches!(name.as_str(), "bananas" | "nobody") {
                return None;
            }
            let groups = u
                .get("groups")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|g| g.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Some(UserEntry {
                username: name,
                full_name: u
                    .get("full_name")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                uid,
                gid,
                admin: groups.iter().any(|g| g == "bananas-admin"),
                locked: u.get("locked").and_then(|v| v.as_bool()).unwrap_or(false),
                password_hash: u
                    .get("password_hash")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            })
        })
        .collect()
}

/// Best-effort timestamp for the export filename — yyyymmdd-hhmmss in
/// the system's local time. Not exact about timezone or DST; the goal
/// is just "files don't overwrite each other on quick re-exports".
fn chrono_compact_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Cheap UTC components — avoids pulling chrono just for a filename.
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 {
        z / 146_097
    } else {
        (z - 146_096) / 146_097
    };
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y } as i64;
    let h = ((secs % 86_400) / 3600) as u32;
    let mi = ((secs % 3600) / 60) as u32;
    let s = (secs % 60) as u32;
    let _ = z; // silence in some compile contexts
    format!("{year:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}")
}
