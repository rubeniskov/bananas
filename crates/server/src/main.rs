//! bananas-server — JSON API + static SPA host.
//!
//! Routes:
//!   GET    /api/exports        → list rows
//!   POST   /api/exports        → add a row (JSON body)
//!   DELETE /api/exports/{idx}  → remove a row by index
//!   GET    /api/browse?path=…  → directory listing for the path picker
//!   GET    /api/healthz        → liveness
//!
//! Everything else is served by the Dioxus Web bundle in `BANANAS_UI_DIR`
//! (defaults to /usr/share/bananas/ui), with SPA-style fallback to
//! index.html so client-side routes resolve.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    middleware::from_fn_with_state,
    response::{Html, IntoResponse},
    routing::{delete, get, post, put},
};
use bananas_helper::{Command, Response as HelperResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::{services::ServeDir, trace::TraceLayer};

mod auth;
mod cloud;
mod config;
mod dirs;
mod exports;
mod fstab;
mod permissions;
mod session;
mod stats;
mod stats_ws;
mod storage;
mod users;
use exports::{Opts, Row, Squash};
use session::SessionKey;

#[derive(Clone)]
pub struct AppState {
    pub exports_path: Arc<PathBuf>,
    pub helper_socket: Arc<PathBuf>,
    pub session_key: Arc<SessionKey>,
    pub stats: stats::StatsState,
    pub live_bus: stats_ws::LiveBus,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bananas_server=info,tower_http=info".into()),
        )
        .init();

    let session_key_path: PathBuf = std::env::var_os("BANANAS_SESSION_KEY")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/bananas/session.key".into());
    let session_key = SessionKey::load_or_create(&session_key_path)?;

    let stats_db_path: PathBuf = std::env::var_os("BANANAS_STATS_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/bananas/stats.db".into());

    let stats_state = stats::StatsState::open(&stats_db_path);
    let live_bus = stats_ws::LiveBus::new();
    // Subscribe to bananas-stats's live Unix socket and re-broadcast
    // to web WS clients. SQLite is no longer touched for live data —
    // bananas-stats is the in-memory source of truth.
    let live_socket_path: PathBuf = std::env::var_os("BANANAS_STATS_LIVE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas-stats/live.sock".into());
    live_bus.start_socket(live_socket_path);

    let state = AppState {
        exports_path: Arc::new(
            std::env::var_os("BANANAS_EXPORTS_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/etc/exports".into()),
        ),
        helper_socket: Arc::new(
            std::env::var_os("BANANAS_HELPER_SOCKET")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/run/bananas/helper.sock".into()),
        ),
        session_key: Arc::new(session_key),
        stats: stats_state,
        live_bus,
    };

    let ui_dir: PathBuf = std::env::var_os("BANANAS_UI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/usr/share/bananas/ui".into());
    let index_html_path = ui_dir.join("index.html");

    // Preload index.html. The SPA fallback handler returns this verbatim so
    // any client-side route resolves on full-page reload. ServeDir's
    // `not_found_service` does not work cleanly for this: tower_http 0.6's
    // ServeFile resolves against the request URI, not its configured path,
    // so it 404s on deep routes.
    let index_html = std::fs::read_to_string(&index_html_path).unwrap_or_else(|e| {
        tracing::warn!(
            path = %index_html_path.display(),
            error = %e,
            "could not read UI index.html — SPA fallback will return an empty body"
        );
        String::new()
    });

    // Public endpoints (login + healthz) and auth-required endpoints share
    // the same /api router. The middleware below permits the public ones
    // and 401s everything else without a valid session cookie.
    let api = Router::new()
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/me", get(auth::me))
        .route("/healthz", get(|| async { "ok" }))
        .route("/exports", get(get_exports).post(post_export))
        .route("/exports/{idx}", delete(delete_export).put(put_export))
        .route("/browse", get(get_browse))
        .route(
            "/permissions",
            get(permissions::get_perms).put(permissions::put_perms),
        )
        .route("/storage", get(get_storage))
        .route("/stats/snapshot", get(stats::snapshot))
        .route("/stats/range", get(stats::range))
        .route("/stats/series", get(stats::series))
        .route("/stats/live", get(stats_ws::live))
        .route(
            "/stats/config",
            get(stats::get_config).put(stats::put_config),
        )
        .route("/fstab", get(get_fstab).post(post_fstab))
        .route("/fstab/{idx}", delete(delete_fstab).put(put_fstab))
        .route("/users", get(users::list).post(users::create))
        .route("/users/{username}", delete(users::delete))
        .route("/users/{username}/password", put(users::set_password))
        .route("/users/{username}/admin", put(users::set_admin))
        .route("/cloud/providers", get(cloud::providers))
        .route(
            "/cloud/accounts",
            get(cloud::list_accounts).post(cloud::add_account),
        )
        .route("/cloud/accounts/{name}", delete(cloud::delete_account))
        .route("/cloud/syncs", get(cloud::list_syncs).post(cloud::add_sync))
        .route(
            "/cloud/syncs/{idx}",
            put(cloud::update_sync).delete(cloud::delete_sync),
        )
        .route("/cloud/syncs/{idx}/run", post(cloud::run_sync))
        .route(
            "/config",
            get(config::export_config).post(config::import_config),
        )
        .route_layer(from_fn_with_state(state.clone(), auth::require_session))
        .with_state(state);

    // Mount /assets/* as a ServeDir; everything else (including /) hits the
    // SPA fallback below so deep routes survive a full-page reload.
    let assets = ServeDir::new(ui_dir.join("assets"));

    let app = Router::new()
        .nest("/api", api)
        .nest_service("/assets", assets)
        .fallback(get(move || {
            let html = index_html.clone();
            async move {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    Html(html),
                )
            }
        }))
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = std::env::var("BANANAS_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    tracing::info!(%addr, ui=%ui_dir.display(), "listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// --- /api/exports -----------------------------------------------------------

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

async fn get_exports(State(state): State<AppState>) -> impl IntoResponse {
    let raw = std::fs::read_to_string(&*state.exports_path).unwrap_or_default();
    let parsed_rows = exports::rows(&raw);
    // Canonical pretty-printed view — same string the helper writes to
    // /etc/exports on save, so the UI's preview can't drift from disk.
    let preview = exports::serialize(&parsed_rows);
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
    Json(json!({ "rows": rows, "preview": preview })).into_response()
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
    // Preserve unknown options from the existing row so editing through
    // the structured form doesn't drop tokens like fsid=0 or nohide that
    // we don't expose as checkboxes.
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
    let cmd = Command::WriteExports { content };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
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

fn api_err(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({ "ok": false, "error": msg.into() }))).into_response()
}

// --- /api/browse ------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BrowseParams {
    #[serde(default = "default_browse_path")]
    path: String,
}

fn default_browse_path() -> String {
    "/srv".into()
}

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

// --- /api/fstab ------------------------------------------------------------

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
    /// True when this row represents a system mount (root, /proc, /sys, …)
    /// the UI is not allowed to edit or delete. Surfaced in the GET
    /// response so the UI can render a lock icon and disable buttons.
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
    // Preserve any existing comment-only / blank lines from the on-disk
    // file by prepending them to our serialized output. Keeps headers
    // like "# /etc/fstab — generated by bananas-image" alive across edits.
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
    match bananas_helper::call(&state.helper_socket, &cmd).await {
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
