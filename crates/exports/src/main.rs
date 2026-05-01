//! bananas-exports — NFS export-table plugin daemon.
//!
//! Listens only on `/run/bananas/exports.sock`. Two URL spaces:
//!  - `/api/exports/*` (forwarded by bananas-router from
//!    webadmin's public TCP) — the row CRUD + browse + mkdir
//!    helpers. Auth-gated via the shared session.key.
//!  - `/assets/exports/*` (forwarded by webadmin's plugin-asset
//!    sub-proxy) — strict asset lookup against bytes embedded
//!    via `include_dir!`. Never serves index.html.
//!
//! All file writes go through `bananas-engine` over its own
//! Unix socket — this daemon runs as the unprivileged bananas
//! user and never touches `/etc/exports` directly.

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::StatusCode,
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use bananas_proto::engine::v1::{
    MakeDirectoryRequest, WriteExportsRequest, engine_service_client::EngineServiceClient,
};
use bananas_proto::engine_client;
use bananas_server_common::{Session, SessionKey, extract_cookie};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod dirs;
mod embedded;
mod exports;

use exports::{Opts, Row, Squash};

#[derive(Clone)]
pub struct AppState {
    pub exports_path: Arc<PathBuf>,
    pub helper_grpc_socket: Arc<PathBuf>,
    pub session_key: Arc<SessionKey>,
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bananas_exports=info,tower_http=info".into()),
        )
        .init();

    let session_key_path: PathBuf = std::env::var_os("BANANAS_SESSION_KEY")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/bananas/session.key".into());
    let session_key = SessionKey::load_or_create(&session_key_path)?;

    let helper_grpc_socket: PathBuf = std::env::var_os("BANANAS_ENGINE_SOCKET")
        .or_else(|| std::env::var_os("BANANAS_ENGINE_GRPC_SOCKET"))
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/engine.sock".into());

    let exports_path: PathBuf = std::env::var_os("BANANAS_EXPORTS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/exports".into());

    let state = AppState {
        exports_path: Arc::new(exports_path),
        helper_grpc_socket: Arc::new(helper_grpc_socket),
        session_key: Arc::new(session_key),
    };

    let api = Router::new()
        .route("/exports", get(get_exports).post(post_export))
        .route("/exports/{idx}", put(put_export).delete(delete_export))
        .route("/exports/browse", get(get_browse))
        .route("/exports/mkdir", post(post_mkdir))
        .route("/exports/__mfe_entry", get(mfe_entry))
        .route_layer(from_fn_with_state(state.clone(), require_session))
        .with_state(state);

    let app = Router::new()
        .nest("/api", api)
        .route("/assets/exports/{*rest}", get(serve_asset))
        .layer(TraceLayer::new_for_http());

    let socket_path: PathBuf = std::env::var_os("BANANAS_EXPORTS_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/exports.sock".into());
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).ok();
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o660))?;
    tracing::info!(socket = %socket_path.display(), "bananas-exports listening");
    axum::serve(listener, app).await?;
    Ok(())
}

// --- handlers ------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ExportRow {
    idx: usize,
    path: String,
    host: String,
    options: String,
    parsed: ExportOpts,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct ExportOpts {
    rw: bool,
    sync: bool,
    no_subtree_check: bool,
    squash: String,
    anonuid: Option<u32>,
    anongid: Option<u32>,
    insecure: bool,
    extra: Vec<String>,
}

impl From<&Opts> for ExportOpts {
    fn from(o: &Opts) -> Self {
        Self {
            rw: o.rw,
            sync: o.sync,
            no_subtree_check: o.no_subtree_check,
            squash: o.squash.as_str().into(),
            anonuid: o.anonuid,
            anongid: o.anongid,
            insecure: o.insecure,
            extra: o.extra.clone(),
        }
    }
}

/// `systemctl is-active` is a read-only check the unprivileged
/// `bananas` user can run without going through the helper. We
/// surface the result on `/api/exports` so the UI can warn when
/// /etc/exports has rows but nfs-server.service isn't running
/// (or vice versa).
async fn nfs_server_status() -> &'static str {
    use tokio::process::Command;
    match Command::new("systemctl")
        .args(["is-active", "nfs-server.service"])
        .output()
        .await
    {
        Ok(out) => match String::from_utf8_lossy(&out.stdout).trim() {
            "active" => "active",
            "inactive" => "inactive",
            "failed" => "failed",
            "activating" => "activating",
            "deactivating" => "deactivating",
            _ => "unknown",
        },
        Err(_) => "unknown",
    }
}

async fn get_exports(State(state): State<AppState>) -> impl IntoResponse {
    let raw = std::fs::read_to_string(&*state.exports_path).unwrap_or_default();
    let parsed_rows = exports::rows(&raw);
    let preview = exports::serialize(&parsed_rows);
    let nfs_status = nfs_server_status().await;
    let rows: Vec<ExportRow> = parsed_rows
        .into_iter()
        .enumerate()
        .map(|(idx, r)| {
            let opts = Opts::parse(&r.options);
            ExportRow {
                idx,
                path: r.path,
                host: r.host,
                options: r.options,
                parsed: ExportOpts::from(&opts),
            }
        })
        .collect();
    Json(json!({
        "rows": rows,
        "preview": preview,
        "nfs_server_status": nfs_status,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct AddExport {
    path: String,
    host: String,
    #[serde(default = "default_true")]
    rw: bool,
    #[serde(default = "default_true")]
    sync: bool,
    #[serde(default = "default_true")]
    no_subtree_check: bool,
    #[serde(default = "default_squash")]
    squash: String,
    anonuid: Option<u32>,
    anongid: Option<u32>,
    #[serde(default)]
    insecure: bool,
}

fn default_true() -> bool {
    true
}
fn default_squash() -> String {
    "all_squash".into()
}

async fn post_export(
    State(state): State<AppState>,
    Json(req): Json<AddExport>,
) -> impl IntoResponse {
    if req.path.trim().is_empty() || req.host.trim().is_empty() {
        return api_err(StatusCode::BAD_REQUEST, "path and host are required");
    }
    if !req.path.starts_with('/') {
        return api_err(StatusCode::BAD_REQUEST, "path must be absolute");
    }
    let opts = Opts {
        rw: req.rw,
        sync: req.sync,
        no_subtree_check: req.no_subtree_check,
        squash: Squash::from_form(&req.squash),
        anonuid: req.anonuid,
        anongid: req.anongid,
        insecure: req.insecure,
        extra: vec![],
    };
    let new_row = Row {
        path: req.path.trim().into(),
        host: req.host.trim().into(),
        options: opts.to_options_string(),
    };
    let raw = std::fs::read_to_string(&*state.exports_path).unwrap_or_default();
    let mut rows = exports::rows(&raw);
    rows.push(new_row);
    apply(&state, &rows).await
}

async fn delete_export(State(state): State<AppState>, Path(idx): Path<usize>) -> impl IntoResponse {
    let raw = std::fs::read_to_string(&*state.exports_path).unwrap_or_default();
    let mut rows = exports::rows(&raw);
    if idx >= rows.len() {
        return api_err(StatusCode::NOT_FOUND, format!("row {idx} not found"));
    }
    rows.remove(idx);
    apply(&state, &rows).await
}

async fn put_export(
    State(state): State<AppState>,
    Path(idx): Path<usize>,
    Json(req): Json<AddExport>,
) -> impl IntoResponse {
    if req.path.trim().is_empty() || req.host.trim().is_empty() {
        return api_err(StatusCode::BAD_REQUEST, "path and host are required");
    }
    if !req.path.starts_with('/') {
        return api_err(StatusCode::BAD_REQUEST, "path must be absolute");
    }
    let raw = std::fs::read_to_string(&*state.exports_path).unwrap_or_default();
    let mut rows = exports::rows(&raw);
    if idx >= rows.len() {
        return api_err(StatusCode::NOT_FOUND, format!("row {idx} not found"));
    }
    // Preserve unknown options from the existing row so editing
    // through the structured form doesn't drop tokens like fsid=0
    // or nohide that we don't expose as checkboxes.
    let existing_extra = Opts::parse(&rows[idx].options).extra;
    let opts = Opts {
        rw: req.rw,
        sync: req.sync,
        no_subtree_check: req.no_subtree_check,
        squash: Squash::from_form(&req.squash),
        anonuid: req.anonuid,
        anongid: req.anongid,
        insecure: req.insecure,
        extra: existing_extra,
    };
    rows[idx] = Row {
        path: req.path.trim().into(),
        host: req.host.trim().into(),
        options: opts.to_options_string(),
    };
    apply(&state, &rows).await
}

async fn apply(state: &AppState, rows: &[Row]) -> axum::response::Response {
    let content = exports::serialize(rows);
    let channel = match engine_client::channel(&state.helper_grpc_socket).await {
        Ok(c) => c,
        Err(e) => {
            return api_err(
                StatusCode::BAD_GATEWAY,
                format!("could not reach engine: {e}"),
            );
        }
    };
    let mut client = EngineServiceClient::new(channel);
    match client.write_exports(WriteExportsRequest { content }).await {
        Ok(resp) => Json(json!({ "ok": true, "output": resp.into_inner().output })).into_response(),
        Err(status) if status.code() == tonic::Code::InvalidArgument => {
            api_err(StatusCode::BAD_REQUEST, status.message().to_string())
        }
        Err(status) => api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("WriteExports failed: {status}"),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct MkdirReq {
    path: String,
}

async fn post_mkdir(State(state): State<AppState>, Json(req): Json<MkdirReq>) -> impl IntoResponse {
    let path = req.path.trim().to_string();
    if path.is_empty() {
        return api_err(StatusCode::BAD_REQUEST, "path required");
    }
    let channel = match engine_client::channel(&state.helper_grpc_socket).await {
        Ok(c) => c,
        Err(e) => {
            return api_err(
                StatusCode::BAD_GATEWAY,
                format!("could not reach engine: {e}"),
            );
        }
    };
    let mut client = EngineServiceClient::new(channel);
    match client.make_directory(MakeDirectoryRequest { path }).await {
        Ok(resp) => Json(json!({ "ok": true, "output": resp.into_inner().output })).into_response(),
        Err(status) if status.code() == tonic::Code::InvalidArgument => {
            api_err(StatusCode::BAD_REQUEST, status.message().to_string())
        }
        Err(status) => api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("mkdir failed: {status}"),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct BrowseParams {
    #[serde(default = "default_browse_path")]
    path: String,
}

fn default_browse_path() -> String {
    "/srv".into()
}

async fn get_browse(Query(p): Query<BrowseParams>) -> impl IntoResponse {
    match dirs::list(std::path::Path::new(&p.path)) {
        Ok(listing) => Json(listing).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string(), "path": p.path })),
        )
            .into_response(),
    }
}

fn api_err(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({ "ok": false, "error": msg.into() }))).into_response()
}

// --- MFE + asset handlers ----------------------------------------

async fn mfe_entry() -> Json<serde_json::Value> {
    Json(json!({ "entry": &*embedded::MFE_ENTRY }))
}

async fn serve_asset(Path(rest): Path<String>, req: Request) -> Response {
    embedded::serve(&rest, &req)
}

// --- auth middleware --------------------------------------------

async fn require_session(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let cookie_value = req
        .headers()
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_cookie);

    let valid = cookie_value
        .and_then(|c| Session::verify(&state.session_key, c))
        .is_some();

    if valid {
        next.run(req).await
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                r#"{"ok":false,"error":"not signed in"}"#,
            ))
            .unwrap()
    }
}
