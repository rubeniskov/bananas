//! bananas-storage — disk + fstab + permissions plugin daemon.
//!
//! Listens only on `/run/bananas/storage.sock`. Two URL spaces:
//!  - `/api/storage/*` (forwarded by bananas-router) — disk
//!    dashboard, fstab CRUD, permissions, browse + mkdir.
//!    Auth-gated via the shared session.key.
//!  - `/assets/storage/*` (forwarded by bananas-webadmin's
//!    sub-proxy) — strict include_dir lookup; no index.html.
//!
//! All file writes (`/etc/fstab`, `chmod`/`chown`) go through
//! `bananas-engine` over its own Unix socket; this daemon runs as
//! the unprivileged `bananas` user.

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
use bananas_engine::{Command, Response as HelperResponse};
use bananas_server_common::{Session, SessionKey, extract_cookie};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod dirs;
mod embedded;
mod fstab;
mod permissions;
mod storage;

#[derive(Clone)]
pub struct AppState {
    pub helper_socket: Arc<PathBuf>,
    pub session_key: Arc<SessionKey>,
    pub storage_cache: storage::StorageCache,
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
                .unwrap_or_else(|_| "bananas_storage=info,tower_http=info".into()),
        )
        .init();

    let session_key_path: PathBuf = std::env::var_os("BANANAS_SESSION_KEY")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/bananas/session.key".into());
    let session_key = SessionKey::load_or_create(&session_key_path)?;

    let helper_socket: PathBuf = std::env::var_os("BANANAS_ENGINE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/engine.sock".into());

    let state = AppState {
        helper_socket: Arc::new(helper_socket),
        session_key: Arc::new(session_key),
        storage_cache: storage::StorageCache::new(),
    };

    let api = Router::new()
        .route("/storage", get(get_storage))
        .route("/storage/fstab", get(get_fstab).post(post_fstab))
        .route("/storage/fstab/{idx}", put(put_fstab).delete(delete_fstab))
        .route(
            "/storage/permissions",
            get(permissions::get_perms).put(permissions::put_perms),
        )
        .route("/storage/browse", get(get_browse))
        .route("/storage/mkdir", post(post_mkdir))
        .route("/storage/__mfe_entry", get(mfe_entry))
        .route_layer(from_fn_with_state(state.clone(), require_session))
        .with_state(state);

    let app = Router::new()
        .nest("/api", api)
        .route("/assets/storage/{*rest}", get(serve_asset))
        .layer(TraceLayer::new_for_http());

    let socket_path: PathBuf = std::env::var_os("BANANAS_STORAGE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/storage.sock".into());
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).ok();
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o660))?;
    tracing::info!(socket = %socket_path.display(), "bananas-storage listening");
    axum::serve(listener, app).await?;
    Ok(())
}

// --- /api/storage --------------------------------------------------------

async fn get_storage(State(state): State<AppState>) -> impl IntoResponse {
    match storage::get_storage(&state).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

// --- /api/storage/fstab --------------------------------------------------

#[derive(Debug, Serialize)]
struct FstabRow {
    idx: usize,
    source: String,
    mountpoint: String,
    fstype: String,
    options: String,
    dump: u32,
    pass: u32,
    parsed: FstabOpts,
    protected: bool,
}

#[derive(Debug, Serialize, Default)]
struct FstabOpts {
    defaults: bool,
    noatime: bool,
    nofail: bool,
    ro: bool,
    discard: bool,
    noexec: bool,
    nosuid: bool,
    nodev: bool,
    device_timeout: Option<u32>,
    extra: Vec<String>,
}

impl From<&fstab::Opts> for FstabOpts {
    fn from(o: &fstab::Opts) -> Self {
        Self {
            defaults: o.defaults,
            noatime: o.noatime,
            nofail: o.nofail,
            ro: o.ro,
            discard: o.discard,
            noexec: o.noexec,
            nosuid: o.nosuid,
            nodev: o.nodev,
            device_timeout: o.device_timeout,
            extra: o.extra.clone(),
        }
    }
}

async fn get_fstab() -> impl IntoResponse {
    let raw = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    let parsed_rows = fstab::rows(&raw);
    let preview = fstab::serialize(&parsed_rows);
    let rows: Vec<FstabRow> = parsed_rows
        .into_iter()
        .enumerate()
        .map(|(idx, r)| {
            let opts = fstab::Opts::parse(&r.options);
            let protected = fstab::is_protected(&r);
            FstabRow {
                idx,
                source: r.source,
                mountpoint: r.mountpoint,
                fstype: r.fstype,
                options: r.options,
                dump: r.dump,
                pass: r.pass,
                parsed: FstabOpts::from(&opts),
                protected,
            }
        })
        .collect();
    Json(json!({ "rows": rows, "preview": preview })).into_response()
}

#[derive(Debug, Deserialize)]
struct AddFstab {
    source: String,
    mountpoint: String,
    fstype: String,
    #[serde(default = "default_true")]
    defaults: bool,
    #[serde(default = "default_true")]
    noatime: bool,
    #[serde(default = "default_true")]
    nofail: bool,
    #[serde(default)]
    ro: bool,
    #[serde(default)]
    discard: bool,
    #[serde(default)]
    noexec: bool,
    #[serde(default)]
    nosuid: bool,
    #[serde(default)]
    nodev: bool,
    device_timeout: Option<u32>,
    #[serde(default = "default_pass")]
    pass: u32,
    #[serde(default)]
    dump: u32,
}

fn default_true() -> bool {
    true
}
fn default_pass() -> u32 {
    2
}

async fn post_fstab(State(state): State<AppState>, Json(req): Json<AddFstab>) -> impl IntoResponse {
    if req.source.trim().is_empty() {
        return api_err(StatusCode::BAD_REQUEST, "device/source is required");
    }
    if req.mountpoint.trim().is_empty() || !req.mountpoint.starts_with('/') {
        return api_err(StatusCode::BAD_REQUEST, "mountpoint must be absolute");
    }
    if req.fstype.trim().is_empty() {
        return api_err(StatusCode::BAD_REQUEST, "filesystem type is required");
    }
    if fstab::is_protected_target(req.mountpoint.trim(), req.fstype.trim(), req.source.trim()) {
        return api_err(
            StatusCode::FORBIDDEN,
            format!(
                "{} is a protected system mount; refusing to shadow it from the UI",
                req.mountpoint.trim()
            ),
        );
    }
    let opts = fstab::Opts {
        defaults: req.defaults,
        noatime: req.noatime,
        nofail: req.nofail,
        ro: req.ro,
        discard: req.discard,
        noexec: req.noexec,
        nosuid: req.nosuid,
        nodev: req.nodev,
        device_timeout: req.device_timeout,
        extra: vec![],
    };
    let new_row = fstab::Row {
        source: req.source.trim().into(),
        mountpoint: req.mountpoint.trim().into(),
        fstype: req.fstype.trim().into(),
        options: opts.to_options_string(),
        dump: req.dump,
        pass: req.pass,
    };
    let raw = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    let mut rows = fstab::rows(&raw);
    rows.push(new_row);
    apply_fstab(&state, &rows).await
}

async fn delete_fstab(State(state): State<AppState>, Path(idx): Path<usize>) -> impl IntoResponse {
    let raw = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    let mut rows = fstab::rows(&raw);
    if idx >= rows.len() {
        return api_err(StatusCode::NOT_FOUND, format!("row {idx} not found"));
    }
    if fstab::is_protected(&rows[idx]) {
        return api_err(
            StatusCode::FORBIDDEN,
            format!(
                "row {idx} is a protected system mount ({}); refusing to delete",
                rows[idx].mountpoint
            ),
        );
    }
    rows.remove(idx);
    apply_fstab(&state, &rows).await
}

async fn put_fstab(
    State(state): State<AppState>,
    Path(idx): Path<usize>,
    Json(req): Json<AddFstab>,
) -> impl IntoResponse {
    if req.source.trim().is_empty() {
        return api_err(StatusCode::BAD_REQUEST, "device/source is required");
    }
    if req.mountpoint.trim().is_empty() || !req.mountpoint.starts_with('/') {
        return api_err(StatusCode::BAD_REQUEST, "mountpoint must be absolute");
    }
    if req.fstype.trim().is_empty() {
        return api_err(StatusCode::BAD_REQUEST, "filesystem type is required");
    }
    let raw = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    let mut rows = fstab::rows(&raw);
    if idx >= rows.len() {
        return api_err(StatusCode::NOT_FOUND, format!("row {idx} not found"));
    }
    if fstab::is_protected(&rows[idx]) {
        return api_err(
            StatusCode::FORBIDDEN,
            format!(
                "row {idx} is a protected system mount ({}); refusing to edit",
                rows[idx].mountpoint
            ),
        );
    }
    if fstab::is_protected_target(req.mountpoint.trim(), req.fstype.trim(), req.source.trim()) {
        return api_err(
            StatusCode::FORBIDDEN,
            format!(
                "{} is a protected system mount; refusing to retarget row {idx}",
                req.mountpoint.trim()
            ),
        );
    }
    let existing_extra = fstab::Opts::parse(&rows[idx].options).extra;
    let opts = fstab::Opts {
        defaults: req.defaults,
        noatime: req.noatime,
        nofail: req.nofail,
        ro: req.ro,
        discard: req.discard,
        noexec: req.noexec,
        nosuid: req.nosuid,
        nodev: req.nodev,
        device_timeout: req.device_timeout,
        extra: existing_extra,
    };
    rows[idx] = fstab::Row {
        source: req.source.trim().into(),
        mountpoint: req.mountpoint.trim().into(),
        fstype: req.fstype.trim().into(),
        options: opts.to_options_string(),
        dump: req.dump,
        pass: req.pass,
    };
    apply_fstab(&state, &rows).await
}

async fn apply_fstab(state: &AppState, rows: &[fstab::Row]) -> axum::response::Response {
    let raw = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    let mut header = String::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            header.push_str(line);
            header.push('\n');
        } else {
            break;
        }
    }
    let body = fstab::serialize(rows);
    let content = format!("{}{}", header, body);

    let cmd = Command::WriteFstab { content };
    match bananas_engine::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => Json(json!({ "ok": true, "output": output })).into_response(),
        Ok(HelperResponse { error, output, .. }) => api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "{}\n\n{}",
                error.as_deref().unwrap_or("helper rejected the change"),
                output
            ),
        ),
        Err(e) => api_err(
            StatusCode::BAD_GATEWAY,
            format!(
                "could not reach helper at {}: {e}",
                state.helper_socket.display()
            ),
        ),
    }
}

// --- /api/storage/browse + /api/storage/mkdir ----------------------------

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

#[derive(Debug, Deserialize)]
struct MkdirReq {
    path: String,
}

async fn post_mkdir(State(state): State<AppState>, Json(req): Json<MkdirReq>) -> impl IntoResponse {
    let path = req.path.trim().to_string();
    if path.is_empty() {
        return api_err(StatusCode::BAD_REQUEST, "path required");
    }
    let cmd = Command::MakeDirectory { path };
    match bananas_engine::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => Json(json!({ "ok": true, "output": output })).into_response(),
        Ok(HelperResponse { error, output, .. }) => api_err(
            StatusCode::BAD_REQUEST,
            format!(
                "{}\n\n{}",
                error.as_deref().unwrap_or("mkdir failed"),
                output
            ),
        ),
        Err(e) => api_err(
            StatusCode::BAD_GATEWAY,
            format!("could not reach helper: {e}"),
        ),
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
