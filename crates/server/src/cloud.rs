//! /api/cloud/* — cloud-sync configuration storage + run dispatch.
//!
//! Storage shape lives at `/etc/bananas/cloud.toml` and is round-trip
//! safe through the existing config bundle (TOML save/load on the
//! Save Config / Load Config buttons). The `cloud_jobs` module owns
//! the in-process job table that backs `/api/cloud/runs/*`.
//!
//! Endpoints:
//!   GET    /api/cloud/providers      — static list of supported
//!                                       provider kinds.
//!   GET    /api/cloud/accounts       — current accounts (tokens
//!                                       redacted).
//!   POST   /api/cloud/accounts       — add an account.
//!   DELETE /api/cloud/accounts/<n>   — remove an account by name.
//!   GET    /api/cloud/syncs          — list sync entries.
//!   POST   /api/cloud/syncs          — add a sync entry.
//!   PUT    /api/cloud/syncs/<idx>    — update entry by index.
//!   DELETE /api/cloud/syncs/<idx>    — remove entry by index.
//!   POST   /api/cloud/syncs/<idx>/run — enqueue a run; returns the
//!                                       job_id immediately.
//!   GET    /api/cloud/runs           — list recent runs (newest first).
//!   GET    /api/cloud/runs/<job_id>  — fetch one run with its output
//!                                       tail; UI polls this while a
//!                                       sync is in flight.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bananas_helper::{Command, Response as HelperResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;

/// Static provider catalog. Surfaced via /api/cloud/providers so the
/// UI's "Add account" form can populate a dropdown without baking the
/// list into the wasm bundle. Add new entries here as we gain
/// confidence each provider works end-to-end with the (future)
/// rclone wiring.
// Provider keys MUST match rclone's actual backend names so the
// "rclone authorize <key>" hint in the UI is copy-pasteable. rclone
// calls Google Drive `drive`, not `google_drive` — getting that wrong
// blew up the operator's first authorize attempt.
const PROVIDERS: &[(&str, &str)] = &[
    ("drive", "Google Drive"),
    ("dropbox", "Dropbox"),
    ("onedrive", "OneDrive"),
    ("s3", "Amazon S3 / S3-compatible"),
    ("webdav", "WebDAV"),
    ("ftp", "FTP / FTPS"),
];

/// Mirrors the on-disk cloud.toml shape. Round-trips through serde
/// + the helper's `WriteServiceConfig`. The same struct is consumed
/// by the config-bundle save/load flow so an admin's TOML backup
/// captures their cloud setup verbatim (tokens included — operators
/// who don't want tokens in their backups can scrub them by hand
/// before sharing the bundle, just like /etc/shadow hashes).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudConfig {
    pub accounts: Vec<Account>,
    pub syncs: Vec<SyncEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Human-friendly name — also the rclone "remote" name. Validated:
    /// POSIX-portable charset only so it round-trips through rclone's
    /// section-header parser.
    pub name: String,
    /// One of the keys in `PROVIDERS`.
    pub provider: String,
    /// rclone-authorize JSON token blob, base64 or raw. The UI shows
    /// it as a redacted "•••" with a "reveal" toggle.
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEntry {
    /// References Account.name. The server doesn't enforce existence
    /// at save time — orphan entries surface as "missing account" in
    /// the UI rather than blocking edits.
    pub account: String,
    pub local_path: String,
    pub remote_path: String,
    /// `push` (local → remote), `pull` (remote → local), or
    /// `bidirectional`. Default push.
    #[serde(default = "default_direction")]
    pub direction: String,
    /// `manual` (run-on-demand) or a 5-field cron string. Default manual.
    #[serde(default = "default_schedule")]
    pub schedule: String,
}

fn default_direction() -> String {
    "push".into()
}
fn default_schedule() -> String {
    "manual".into()
}

// ---------------- Provider catalog ----------------

pub async fn providers() -> Response {
    let list: Vec<serde_json::Value> = PROVIDERS
        .iter()
        .map(|(k, label)| json!({ "key": k, "label": label }))
        .collect();
    Json(json!({ "providers": list })).into_response()
}

// ---------------- Read / write helpers ----------------

async fn load(state: &AppState) -> Result<CloudConfig, String> {
    let cmd = Command::ReadServiceConfig {
        name: "cloud".into(),
    };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => {
            if output.trim().is_empty() {
                return Ok(CloudConfig::default());
            }
            toml::from_str(&output).map_err(|e| format!("parsing cloud.toml: {e}"))
        }
        Ok(HelperResponse { error, .. }) => {
            Err(error.unwrap_or_else(|| "helper rejected ReadServiceConfig".into()))
        }
        Err(e) => Err(format!("helper unreachable: {e}")),
    }
}

async fn save(state: &AppState, cfg: &CloudConfig) -> Result<(), String> {
    let body = toml::to_string_pretty(cfg).map_err(|e| format!("serializing cloud.toml: {e}"))?;
    let cmd = Command::WriteServiceConfig {
        name: "cloud".into(),
        content: body,
    };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse { ok: true, .. }) => Ok(()),
        Ok(HelperResponse { error, output, .. }) => Err(format!(
            "{}\n\n{}",
            error.unwrap_or_else(|| "helper rejected WriteServiceConfig".into()),
            output
        )),
        Err(e) => Err(format!("helper unreachable: {e}")),
    }
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "ok": false, "error": msg.into() }))).into_response()
}

// ---------------- Accounts ----------------

/// List configured accounts. Tokens are redacted to a single `•` so
/// they never leak through normal API browsing — the operator can
/// confirm an account exists without exposing the credential. Backup
/// (config save) goes through a different code path that includes
/// the token verbatim.
pub async fn list_accounts(State(state): State<AppState>) -> Response {
    let cfg = match load(&state).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let accounts: Vec<serde_json::Value> = cfg
        .accounts
        .iter()
        .map(|a| {
            json!({
                "name": a.name,
                "provider": a.provider,
                "token_present": !a.token.is_empty(),
            })
        })
        .collect();
    Json(json!({ "accounts": accounts })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct AddAccountReq {
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub token: String,
}

pub async fn add_account(
    State(state): State<AppState>,
    Json(req): Json<AddAccountReq>,
) -> Response {
    if !is_safe_account_name(&req.name) {
        return err(
            StatusCode::BAD_REQUEST,
            "account name must be 1–32 chars from [A-Za-z0-9_-]",
        );
    }
    if !PROVIDERS.iter().any(|(k, _)| *k == req.provider) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("unknown provider {:?}", req.provider),
        );
    }
    let mut cfg = match load(&state).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if cfg.accounts.iter().any(|a| a.name == req.name) {
        return err(
            StatusCode::CONFLICT,
            format!("account {:?} already exists", req.name),
        );
    }
    cfg.accounts.push(Account {
        name: req.name,
        provider: req.provider,
        token: req.token,
    });
    if let Err(e) = save(&state, &cfg).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    Json(json!({ "ok": true })).into_response()
}

/// Update the provider and/or token of an existing account. Renaming an
/// account is intentionally NOT supported here — `name` is the rclone
/// remote identifier, and renaming would orphan every sync entry that
/// references it. Operators who want a different name delete + add.
///
/// Empty `token` keeps the existing token (the same redaction the GET
/// path uses), so an operator who's only swapping provider does not have
/// to re-paste their auth blob.
#[derive(Debug, Deserialize)]
pub struct UpdateAccountReq {
    pub provider: String,
    #[serde(default)]
    pub token: String,
}

pub async fn update_account(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(req): Json<UpdateAccountReq>,
) -> Response {
    if !PROVIDERS.iter().any(|(k, _)| *k == req.provider) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("unknown provider {:?}", req.provider),
        );
    }
    let mut cfg = match load(&state).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let Some(account) = cfg.accounts.iter_mut().find(|a| a.name == name) else {
        return err(StatusCode::NOT_FOUND, format!("account {name:?} not found"));
    };
    account.provider = req.provider;
    if !req.token.is_empty() {
        account.token = req.token;
    }
    if let Err(e) = save(&state, &cfg).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete_account(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let mut cfg = match load(&state).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let before = cfg.accounts.len();
    cfg.accounts.retain(|a| a.name != name);
    if cfg.accounts.len() == before {
        return err(StatusCode::NOT_FOUND, format!("account {name:?} not found"));
    }
    // Also drop any sync entries pointing at the removed account so
    // the UI doesn't end up listing orphans.
    cfg.syncs.retain(|s| s.account != name);
    if let Err(e) = save(&state, &cfg).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    Json(json!({ "ok": true })).into_response()
}

fn is_safe_account_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ---------------- Sync entries ----------------

pub async fn list_syncs(State(state): State<AppState>) -> Response {
    let cfg = match load(&state).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let with_idx: Vec<serde_json::Value> = cfg
        .syncs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "idx": i,
                "account": s.account,
                "local_path": s.local_path,
                "remote_path": s.remote_path,
                "direction": s.direction,
                "schedule": s.schedule,
            })
        })
        .collect();
    Json(json!({ "syncs": with_idx })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct AddSyncReq {
    pub account: String,
    pub local_path: String,
    pub remote_path: String,
    #[serde(default = "default_direction")]
    pub direction: String,
    #[serde(default = "default_schedule")]
    pub schedule: String,
}

fn validate_sync(req: &AddSyncReq) -> Result<(), String> {
    if req.account.is_empty() {
        return Err("account is required".into());
    }
    if !req.local_path.starts_with('/') {
        return Err("local_path must be absolute".into());
    }
    if req.remote_path.is_empty() {
        return Err("remote_path is required".into());
    }
    if !["push", "pull", "bidirectional"].contains(&req.direction.as_str()) {
        return Err(format!("unknown direction {:?}", req.direction));
    }
    Ok(())
}

pub async fn add_sync(State(state): State<AppState>, Json(req): Json<AddSyncReq>) -> Response {
    if let Err(e) = validate_sync(&req) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let mut cfg = match load(&state).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    cfg.syncs.push(SyncEntry {
        account: req.account,
        local_path: req.local_path,
        remote_path: req.remote_path,
        direction: req.direction,
        schedule: req.schedule,
    });
    if let Err(e) = save(&state, &cfg).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    Json(json!({ "ok": true })).into_response()
}

pub async fn update_sync(
    State(state): State<AppState>,
    AxumPath(idx): AxumPath<usize>,
    Json(req): Json<AddSyncReq>,
) -> Response {
    if let Err(e) = validate_sync(&req) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let mut cfg = match load(&state).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if idx >= cfg.syncs.len() {
        return err(StatusCode::NOT_FOUND, format!("sync row {idx} not found"));
    }
    cfg.syncs[idx] = SyncEntry {
        account: req.account,
        local_path: req.local_path,
        remote_path: req.remote_path,
        direction: req.direction,
        schedule: req.schedule,
    };
    if let Err(e) = save(&state, &cfg).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete_sync(
    State(state): State<AppState>,
    AxumPath(idx): AxumPath<usize>,
) -> Response {
    let mut cfg = match load(&state).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if idx >= cfg.syncs.len() {
        return err(StatusCode::NOT_FOUND, format!("sync row {idx} not found"));
    }
    cfg.syncs.remove(idx);
    if let Err(e) = save(&state, &cfg).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    Json(json!({ "ok": true })).into_response()
}

/// Trigger a sync. Returns immediately with the new (or existing, if a
/// run was already in flight) job_id; the actual rclone call runs on a
/// background task. Caller polls `/api/cloud/runs/{job_id}` to follow it.
pub async fn run_sync(State(state): State<AppState>, AxumPath(idx): AxumPath<usize>) -> Response {
    let cfg = match load(&state).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let Some(entry) = cfg.syncs.get(idx) else {
        return err(StatusCode::NOT_FOUND, format!("sync row {idx} not found"));
    };
    let label = format!(
        "{}:{} → {}:{}",
        entry.account, entry.local_path, entry.account, entry.remote_path
    );
    let job_id = state
        .jobs
        .enqueue(idx, label, (*state.helper_socket).clone())
        .await;
    Json(json!({ "ok": true, "job_id": job_id })).into_response()
}

/// List recent jobs (newest first). Bounded to ~100 by the job manager.
pub async fn list_runs(State(state): State<AppState>) -> Response {
    let jobs = state.jobs.list().await;
    Json(json!({ "runs": jobs })).into_response()
}

pub async fn get_run(State(state): State<AppState>, AxumPath(job_id): AxumPath<u64>) -> Response {
    match state.jobs.get(job_id).await {
        Some(j) => Json(j).into_response(),
        None => err(StatusCode::NOT_FOUND, format!("job {job_id} not found")),
    }
}
