//! /api/stats — read-only window into the bananas-stats SQLite store.
//!
//! The daemon (bananas-stats) writes; we open the same file in
//! WAL-shared mode and run cheap reads on each request. WAL allows
//! multiple readers + one writer with no locking on the read path.
//!
//! Auth: routes are nested under /api so the existing session middleware
//! gates them automatically.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bananas_stats::storage::{self, queries};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;

#[derive(Clone)]
pub struct StatsState {
    pub db: Option<Arc<storage::Database>>,
}

impl StatsState {
    /// Open the stats DB in read-only mode. Returns a "stub" state with
    /// `db = None` if the file isn't there yet — the daemon may not
    /// have started for the first time. Endpoints will report this
    /// transparently rather than panicking at boot.
    pub fn open(path: &std::path::Path) -> Self {
        if !path.exists() {
            tracing::warn!(?path, "stats DB not found — /api/stats/* will return empty until bananas-stats writes its first row");
            return Self { db: None };
        }
        match storage::Database::open_readonly(path) {
            Ok(db) => Self { db: Some(Arc::new(db)) },
            Err(e) => {
                tracing::warn!(error = ?e, ?path, "could not open stats DB read-only");
                Self { db: None }
            }
        }
    }
}

pub async fn snapshot(State(state): State<AppState>) -> Response {
    let Some(db) = state.stats.db.as_ref() else {
        return empty_snapshot();
    };
    let db = db.clone();
    match tokio::task::spawn_blocking(move || queries::latest_snapshot(&db)).await {
        Ok(Ok(snap)) => Json(snap).into_response(),
        Ok(Err(e)) => err_500(e.to_string()),
        Err(e) => err_500(format!("join: {e}")),
    }
}

#[derive(Debug, Deserialize)]
pub struct RangeParams {
    pub metric: String,        // "net" | "disk"
    pub key: String,           // iface name or device name
    #[serde(default = "default_window")]
    pub window: String,        // "5m" | "1h" | "24h" — parsed in seconds
    #[serde(default = "default_resolution")]
    pub resolution: String,    // "raw" | "1m"
}

fn default_window() -> String { "5m".into() }
fn default_resolution() -> String { "raw".into() }

pub async fn range(
    State(state): State<AppState>,
    Query(p): Query<RangeParams>,
) -> Response {
    let Some(db) = state.stats.db.as_ref() else {
        return Json(json!({ "points": [] })).into_response();
    };
    let secs = parse_window(&p.window);
    let table = match (p.metric.as_str(), p.resolution.as_str()) {
        ("net", "raw") => "net_samples",
        ("net", "1m") => "net_samples_1m",
        ("disk", "raw") => "disk_samples",
        ("disk", "1m") => "disk_samples_1m",
        _ => return err_400(format!(
            "unknown metric/resolution combination: metric={:?} resolution={:?}",
            p.metric, p.resolution
        )),
    };
    let now = chrono_now();
    let from = now - secs as i64;
    let db = db.clone();
    let metric = p.metric.clone();
    let key = p.key.clone();
    let table_owned = table.to_string();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        match metric.as_str() {
            "net" => {
                let rows = queries::net_range(&db, &key, from, now, &table_owned)?;
                Ok(json!({ "points": rows }))
            }
            "disk" => {
                let rows = queries::disk_range(&db, &key, from, now, &table_owned)?;
                Ok(json!({ "points": rows }))
            }
            _ => unreachable!(),
        }
    })
    .await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => err_500(e.to_string()),
        Err(e) => err_500(format!("join: {e}")),
    }
}

pub async fn series(State(state): State<AppState>) -> Response {
    let Some(db) = state.stats.db.as_ref() else {
        return Json(json!({ "interfaces": [], "disks": [] })).into_response();
    };
    let db = db.clone();
    match tokio::task::spawn_blocking(move || queries::series_keys(&db)).await {
        Ok(Ok(keys)) => Json(keys).into_response(),
        Ok(Err(e)) => err_500(e.to_string()),
        Err(e) => err_500(format!("join: {e}")),
    }
}

fn empty_snapshot() -> Response {
    Json(json!({
        "ts_unix": 0,
        "cpu": { "busy_pct": 0.0 },
        "mem": { "total": 0, "used": 0, "available": 0, "free": 0 },
        "network": [],
        "disks": [],
        "parts": [],
        "stats_db_present": false,
    }))
    .into_response()
}

/// GET /api/stats/config — current TOML for the bananas-stats service.
/// Returns `{config: "..."}` so the UI can drop it straight into a
/// textarea without parsing.
pub async fn get_config(State(state): State<AppState>) -> Response {
    use bananas_helper::{Command, Response as HelperResponse};
    let cmd = Command::ReadServiceConfig { name: "stats".into() };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse { ok: true, output, .. }) => {
            Json(json!({ "config": output })).into_response()
        }
        Ok(HelperResponse { error, .. }) => err_500(
            error.unwrap_or_else(|| "helper rejected ReadServiceConfig".into()),
        ),
        Err(e) => err_500(format!("helper unreachable: {e}")),
    }
}

#[derive(Debug, Deserialize)]
pub struct PutConfig {
    pub config: String,
}

/// PUT /api/stats/config — replace stats.toml and bounce the service.
/// Returns `{ok, output}` mirroring the helper's response so the UI can
/// surface systemctl's stdout in the success banner.
pub async fn put_config(
    State(state): State<AppState>,
    Json(req): Json<PutConfig>,
) -> Response {
    use bananas_helper::{Command, Response as HelperResponse};
    let cmd = Command::WriteServiceConfig {
        name: "stats".into(),
        content: req.config,
    };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse { ok: true, output, .. }) => {
            Json(json!({ "ok": true, "output": output })).into_response()
        }
        Ok(HelperResponse { error, output, .. }) => err_400(format!(
            "{}\n\n{}",
            error.unwrap_or_else(|| "helper rejected WriteServiceConfig".into()),
            output
        )),
        Err(e) => err_500(format!("helper unreachable: {e}")),
    }
}

fn err_500(msg: String) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg }))).into_response()
}

fn err_400(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

fn parse_window(s: &str) -> u64 {
    let s = s.trim();
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u64 = num.parse().unwrap_or(300);
    match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => 300, // fall back to 5 minutes
    }
}

fn chrono_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
