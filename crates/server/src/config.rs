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
use bananas_helper::{Command, Response as HelperResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{AppState, exports, fstab};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ConfigBundle {
    /// Schema version. Bump when the layout changes incompatibly so
    /// older importers can refuse instead of silently dropping fields.
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub exports: Vec<ExportEntry>,
    #[serde(default)]
    pub fstab: Vec<FstabEntry>,
    #[serde(default)]
    pub users: Vec<UserEntry>,
    /// Cloud-sync accounts + sync entries from /etc/bananas/cloud.toml.
    /// Persisted in the bundle so a fresh image's "Load config" call
    /// fully restores the operator's setup, including OAuth tokens. Treat
    /// the bundle as sensitive — anyone with the token can act as the
    /// account on the configured provider.
    #[serde(default)]
    pub cloud: crate::cloud::CloudConfig,
}

fn default_version() -> u32 { 1 }

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportEntry {
    pub path: String,
    pub host: String,
    pub options: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FstabEntry {
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
    let filename = format!(
        "bananas-config-{}.toml",
        chrono_compact_now()
    );
    (
        StatusCode::OK,
        [
            ("content-type", "application/toml; charset=utf-8".to_string()),
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
struct ImportSummary {
    ok: bool,
    exports_written: usize,
    fstab_written: usize,
    users_created: usize,
    users_skipped: usize,
    cloud_accounts: usize,
    cloud_syncs: usize,
    notes: Vec<String>,
}

pub async fn import_config(State(state): State<AppState>, body: String) -> Response {
    let bundle: ConfigBundle = match toml::from_str(&body) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "ok": false,
                    "error": format!("invalid TOML: {e}"),
                })),
            )
                .into_response();
        }
    };
    if bundle.version > default_version() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": format!(
                    "unsupported config version {} (this server understands up to {})",
                    bundle.version, default_version()
                ),
            })),
        )
            .into_response();
    }

    let mut summary = ImportSummary {
        ok: true,
        exports_written: 0,
        fstab_written: 0,
        users_created: 0,
        users_skipped: 0,
        cloud_accounts: 0,
        cloud_syncs: 0,
        notes: Vec::new(),
    };

    // ---- exports ---------------------------------------------------------
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
    match bananas_helper::call(
        &state.helper_socket,
        &Command::WriteExports { content: exports_content },
    )
    .await
    {
        Ok(HelperResponse { ok: true, .. }) => {
            summary.exports_written = export_rows.len();
        }
        Ok(HelperResponse { error, output, .. }) => {
            summary.ok = false;
            summary.notes.push(format!(
                "exports: {}\n{}",
                error.unwrap_or_else(|| "helper rejected exports".into()),
                output
            ));
        }
        Err(e) => {
            summary.ok = false;
            summary.notes.push(format!("exports: helper unreachable: {e}"));
        }
    }

    // ---- fstab -----------------------------------------------------------
    // Bundle entries are user-managed mounts only — defensively drop
    // anything that would shadow a protected system mount, even if a
    // hand-edited TOML tried to sneak one in.
    let bundle_rows: Vec<fstab::Row> = bundle
        .fstab
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

    // Preserve the comment header AND every on-disk protected row (so
    // restoring a backup never blows away `/`, `/proc`, etc., even though
    // those rows are intentionally absent from the bundle).
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
    match bananas_helper::call(
        &state.helper_socket,
        &Command::WriteFstab { content: fstab_content },
    )
    .await
    {
        Ok(HelperResponse { ok: true, .. }) => {
            summary.fstab_written = bundle_count;
        }
        Ok(HelperResponse { error, output, .. }) => {
            summary.ok = false;
            summary.notes.push(format!(
                "fstab: {}\n{}",
                error.unwrap_or_else(|| "helper rejected fstab".into()),
                output
            ));
        }
        Err(e) => {
            summary.ok = false;
            summary.notes.push(format!("fstab: helper unreachable: {e}"));
        }
    }

    // ---- users -----------------------------------------------------------
    // Each entry with a password_hash gets recreated via CreateUser
    // (password_is_hash=true). Entries without a hash are skipped because
    // the helper requires *some* credential to set on /etc/shadow.
    for entry in &bundle.users {
        let Some(hash) = &entry.password_hash else {
            summary.users_skipped += 1;
            summary.notes.push(format!(
                "{}: skipped (no password_hash in backup)",
                entry.username
            ));
            continue;
        };
        let cmd = Command::CreateUser {
            username: entry.username.clone(),
            password: hash.clone(),
            full_name: entry.full_name.clone(),
            admin: entry.admin,
            password_is_hash: true,
        };
        match bananas_helper::call(&state.helper_socket, &cmd).await {
            Ok(HelperResponse { ok: true, .. }) => summary.users_created += 1,
            Ok(HelperResponse { error, .. }) => {
                summary.users_skipped += 1;
                summary.notes.push(format!(
                    "{}: {}",
                    entry.username,
                    error.unwrap_or_else(|| "helper rejected create".into())
                ));
            }
            Err(e) => {
                summary.ok = false;
                summary.users_skipped += 1;
                summary.notes.push(format!("{}: helper unreachable: {e}", entry.username));
            }
        }
    }

    // ---- cloud -----------------------------------------------------------
    // Replace the on-disk cloud.toml with the bundle's section. The
    // server has no live state to invalidate (it reads cloud.toml on
    // every /api/cloud/* call), so write-and-done is sufficient.
    let cloud_toml = match toml::to_string_pretty(&bundle.cloud) {
        Ok(s) => s,
        Err(e) => {
            summary.ok = false;
            summary.notes.push(format!("cloud: serialize failed: {e}"));
            String::new()
        }
    };
    if !cloud_toml.is_empty() {
        match bananas_helper::call(
            &state.helper_socket,
            &Command::WriteServiceConfig { name: "cloud".into(), content: cloud_toml },
        )
        .await
        {
            Ok(HelperResponse { ok: true, .. }) => {
                summary.cloud_accounts = bundle.cloud.accounts.len();
                summary.cloud_syncs = bundle.cloud.syncs.len();
            }
            Ok(HelperResponse { error, output, .. }) => {
                summary.ok = false;
                summary.notes.push(format!(
                    "cloud: {}\n{}",
                    error.unwrap_or_else(|| "helper rejected cloud".into()),
                    output
                ));
            }
            Err(e) => {
                summary.ok = false;
                summary.notes.push(format!("cloud: helper unreachable: {e}"));
            }
        }
    }

    let status = if summary.ok { StatusCode::OK } else { StatusCode::INTERNAL_SERVER_ERROR };
    (status, Json(summary)).into_response()
}

async fn build_bundle(state: &AppState) -> Result<ConfigBundle, String> {
    let exports_raw = std::fs::read_to_string(&*state.exports_path).unwrap_or_default();
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
    let fstab_rows = fstab::rows(&fstab_raw)
        .into_iter()
        .filter(|r| !fstab::is_protected(r))
        .map(|r| FstabEntry {
            source: r.source,
            mountpoint: r.mountpoint,
            fstype: r.fstype,
            options: r.options,
            dump: r.dump,
            pass: r.pass,
        })
        .collect();

    // Users via the helper — ExportUsers includes shadow hashes so the
    // backup actually round-trips a working account. /api/users continues
    // to use ListUsers, which omits hashes.
    let users = match bananas_helper::call(&state.helper_socket, &Command::ExportUsers).await {
        Ok(HelperResponse { ok: true, output, .. }) => parse_users_payload(&output),
        Ok(HelperResponse { error, .. }) => {
            return Err(error.unwrap_or_else(|| "helper rejected export-users".into()));
        }
        Err(e) => return Err(format!("helper unreachable: {e}")),
    };

    // Cloud config — same source-of-truth as /api/cloud/*. Read via
    // the helper so the file's perms are respected (root-owned, the
    // server is unprivileged). Errors here are non-fatal because cloud
    // is optional and the absence of the file is a valid state.
    let cloud = match bananas_helper::call(
        &state.helper_socket,
        &Command::ReadServiceConfig { name: "cloud".into() },
    )
    .await
    {
        Ok(HelperResponse { ok: true, output, .. }) => {
            if output.trim().is_empty() {
                crate::cloud::CloudConfig::default()
            } else {
                toml::from_str(&output).unwrap_or_default()
            }
        }
        _ => crate::cloud::CloudConfig::default(),
    };

    Ok(ConfigBundle {
        version: 1,
        exports: exports_rows,
        fstab: fstab_rows,
        users,
        cloud,
    })
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
    let era = if z >= 0 { z / 146_097 } else { (z - 146_096) / 146_097 };
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
