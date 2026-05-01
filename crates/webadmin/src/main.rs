//! bananas-webadmin — public TCP face + internal API daemon.
//!
//! Two listeners on the same process:
//!
//!  - **Public TCP** (`:8080`) handles every browser-facing path:
//!    serves the embedded host SPA at `/` and `/assets/*`, sub-proxies
//!    `/assets/<plugin_id>/*` to plugin daemons over Unix socket, and
//!    sub-proxies `/api/*` to `bananas-router`. No business logic here.
//!  - **Internal Unix** (`/run/bananas/webadmin.sock`) handles
//!    webadmin's own /api endpoints (login, me, users, exports, …).
//!    Reached only via the router's catch-all `api_prefix = "/api"`,
//!    which the public TCP layer hits via the proxy.
//!
//! The single public origin means: ONE `index.html` ever leaves the
//! router publicly, plugin SPAs are composed inline at runtime via
//! the MFE loader, and `bananas-router` is an internal-only API
//! gateway that no browser ever connects to directly.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    middleware::from_fn_with_state,
    response::IntoResponse,
    routing::{any, delete, get, post, put},
};
use bananas_engine::{Command, Response as HelperResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::trace::TraceLayer;

mod auth;
mod config;
mod dirs;
mod embedded;
mod exports;
mod extensions;
mod fstab;
mod operations;
mod permissions;
mod proxy;
mod service_config;
mod stats;
mod stats_ws;
mod storage;
mod system;
mod updates;
mod users;
use bananas_server_common::{Manifest, SessionKey, load_all};
use exports::{Opts, Row, Squash};

/// Map of plugin id → daemon Unix socket. Built once at startup
/// from `/etc/bananas/extensions.d/`. `/assets/<id>/*` requests
/// look up the socket here and 404 if `<id>` isn't a known
/// plugin. `bananas-webadmin`'s own manifest is excluded from this
/// map (it's not an asset-providing plugin; its assets are the
/// embedded host SPA, served from `/assets/<file>` directly).
#[derive(Clone, Default)]
pub struct PluginAssetMap(Arc<std::collections::HashMap<String, PathBuf>>);

impl PluginAssetMap {
    fn from_manifests(manifests: &[Manifest], own_id: &str) -> Self {
        let mut by_id = std::collections::HashMap::new();
        for m in manifests {
            if m.id == own_id {
                continue;
            }
            by_id.insert(m.id.clone(), m.socket.clone());
        }
        PluginAssetMap(Arc::new(by_id))
    }
    fn get(&self, id: &str) -> Option<&PathBuf> {
        self.0.get(id)
    }
}

#[derive(Clone)]
pub struct AppState {
    pub exports_path: Arc<PathBuf>,
    pub helper_socket: Arc<PathBuf>,
    pub session_key: Arc<SessionKey>,
    pub stats: stats::StatsState,
    pub live_bus: stats_ws::LiveBus,
    pub storage_cache: storage::StorageCache,
    pub operations: operations::OperationManager,
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bananas_webadmin=info,tower_http=info".into()),
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
        .unwrap_or_else(|| "/run/bananas/stats.sock".into());
    live_bus.start_socket(live_socket_path);

    let helper_socket: PathBuf = std::env::var_os("BANANAS_ENGINE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/engine.sock".into());

    let operations_journal: PathBuf = std::env::var_os("BANANAS_OPERATIONS_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/bananas/operations.json".into());
    let operations = operations::OperationManager::load(operations_journal).await;
    // Per-kind reinstaters: ask the helper if any opkg upgrade is
    // still in flight from a previous server lifetime. If yes, the
    // matching journal entry is reattached with a fresh log watcher.
    operations::opkg::reinstate(&operations, &helper_socket).await;
    // Flush any Running entry no reinstater claimed. After this point,
    // the only Running ops are ones genuinely backed by live work.
    operations.flush_orphan_running().await;

    let state = AppState {
        exports_path: Arc::new(
            std::env::var_os("BANANAS_EXPORTS_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/etc/exports".into()),
        ),
        helper_socket: Arc::new(helper_socket),
        session_key: Arc::new(session_key),
        stats: stats_state,
        live_bus,
        storage_cache: storage::StorageCache::new(),
        operations,
    };

    // First-boot geoip → timezone (best-effort, non-blocking, non-fatal).
    // Skips silently if /etc/bananas/system.toml already has a tz set.
    system::spawn_first_boot_geoip(state.clone());

    // Read the manifest dir to build the plugin asset map and the
    // router socket path. `bananas-router` reads the same dir; we
    // keep our own snapshot so /assets/<id>/* requests don't have
    // to round-trip to the router for socket lookup.
    let manifests_dir: PathBuf = std::env::var_os("BANANAS_EXTENSIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/bananas/extensions.d".into());
    let manifests = load_all(&manifests_dir);
    let asset_map = PluginAssetMap::from_manifests(&manifests, "webadmin");
    let router_socket: PathBuf = std::env::var_os("BANANAS_ROUTER_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/router.sock".into());
    tracing::info!(
        plugins = asset_map.0.len(),
        router = %router_socket.display(),
        "asset proxy map ready"
    );

    // Public endpoints (login + healthz) and auth-required endpoints share
    // the same /api router. The middleware below permits the public ones
    // and 401s everything else without a valid session cookie.
    let api = Router::new()
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/password", post(auth::change_password))
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
        .route(
            "/dashboard/config",
            get(service_config::get_dashboard).put(service_config::put_dashboard),
        )
        .route(
            "/system/config",
            get(service_config::get_system).put(service_config::put_system),
        )
        .route("/system/timezone", post(system::post_timezone))
        .route("/system/timezones", get(system::get_timezones))
        .route("/fstab", get(get_fstab).post(post_fstab))
        .route("/fstab/{idx}", delete(delete_fstab).put(put_fstab))
        .route("/users", get(users::list).post(users::create))
        .route("/users/{username}", delete(users::delete))
        .route("/users/{username}/password", put(users::set_password))
        .route("/users/{username}/admin", put(users::set_admin))
        .route("/system/reboot", post(post_reboot))
        .route("/mkdir", post(post_mkdir))
        .route("/version", get(updates::get_version))
        .route("/updates/check", get(updates::get_updates_check))
        .route("/updates/install", post(updates::post_updates_install))
        .route("/updates/status", get(updates::get_updates_status))
        .route("/extensions", get(extensions::list_extensions))
        .route("/operations", get(operations::list))
        .route("/operations/active", get(operations::list_active))
        .route("/operations/{id}", get(operations::get_one))
        .route("/operations/{id}/log", get(operations::log_stream))
        .route("/operations/{id}/cancel", post(operations::cancel))
        .route(
            "/config",
            get(config::export_config).post(config::import_config),
        )
        .route_layer(from_fn_with_state(state.clone(), auth::require_session))
        .with_state(state);

    // Internal app — bound to the Unix socket. Holds webadmin's own
    // /api/* handlers (login, me, exports, …); the public TCP listener
    // never invokes these directly, only via sub-proxy through
    // bananas-router's catch-all api_prefix="/api".
    let internal_app = Router::new()
        .nest("/api", api)
        .layer(TraceLayer::new_for_http());

    // Public app — bound to TCP. No business logic; only static asset
    // serving + sub-proxies. Order matters: more-specific routes
    // (`/assets/<id>/*`, `/api/*`) are registered before the broad
    // `/assets/*` and the SPA-fallback so axum's matcher hits them
    // first.
    let proxy_state = ProxyState {
        router_socket: Arc::new(router_socket),
        asset_map,
    };
    let public_app = Router::new()
        // Plugin static assets — sub-proxied to the right plugin
        // daemon's Unix socket. Path is forwarded verbatim, including
        // the `/assets/<id>/` prefix, because each plugin's daemon
        // routes its own `/assets/<id>/{*rest}` handler.
        .route(
            "/assets/{plugin_id}/{*rest}",
            any(plugin_asset_proxy).with_state(proxy_state.clone()),
        )
        // Every /api/* path goes through bananas-router. Even
        // webadmin's own /api/login lands here on the public side
        // and self-loops back via the router's catch-all to
        // webadmin's Unix socket.
        .route(
            "/api/{*rest}",
            any(api_proxy).with_state(proxy_state.clone()),
        )
        .route("/api", any(api_proxy).with_state(proxy_state))
        // Everything else is the embedded host SPA: bare `/`,
        // `/assets/<host-file>`, deep-link SPA paths.
        .fallback(get(serve_ui))
        .layer(TraceLayer::new_for_http());

    // Both listeners run concurrently; tokio::try_join surfaces
    // either one's error. Public TCP is `0.0.0.0:8080`; the public
    // app is the only daemon a browser ever connects to.
    let socket_path: PathBuf = std::env::var_os("BANANAS_WEBADMIN_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/webadmin.sock".into());
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).ok();
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let unix_listener = tokio::net::UnixListener::bind(&socket_path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o660))?;
    tracing::info!(socket = %socket_path.display(), "internal API listener (unix)");

    let addr: SocketAddr = std::env::var("BANANAS_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    let tcp_listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "public listener (tcp)");

    let unix_fut = axum::serve(unix_listener, internal_app);
    let tcp_fut = axum::serve(tcp_listener, public_app);
    tokio::try_join!(unix_fut.into_future(), tcp_fut.into_future())?;
    Ok(())
}

#[derive(Clone)]
struct ProxyState {
    router_socket: Arc<PathBuf>,
    asset_map: PluginAssetMap,
}

async fn api_proxy(
    State(state): State<ProxyState>,
    req: axum::extract::Request,
) -> axum::response::Response {
    proxy::proxy_to_unix(&state.router_socket, req).await
}

async fn plugin_asset_proxy(
    State(state): State<ProxyState>,
    Path((plugin_id, _rest)): Path<(String, String)>,
    req: axum::extract::Request,
) -> axum::response::Response {
    let Some(socket) = state.asset_map.get(&plugin_id) else {
        return (
            StatusCode::NOT_FOUND,
            format!("unknown plugin '{plugin_id}'"),
        )
            .into_response();
    };
    proxy::proxy_to_unix(socket, req).await
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

/// `systemctl is-active` is a read-only check the unprivileged
/// `bananas` user can run without going through the helper. We surface
/// the result on `/api/exports` so the UI can warn when /etc/exports
/// has rows but nfs-server.service isn't running (or vice versa).
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
    // Canonical pretty-printed view — same string the helper writes to
    // /etc/exports on save, so the UI's preview can't drift from disk.
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

/// Forward the `RebootSystem` helper command. The helper returns
/// before systemd actually fires the reboot, so we get a normal 200
/// back; the browser then sees the connection drop a beat later.
async fn post_reboot(State(state): State<AppState>) -> impl IntoResponse {
    let cmd = Command::RebootSystem;
    match bananas_engine::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => Json(json!({ "ok": true, "output": output })).into_response(),
        Ok(HelperResponse { error, output, .. }) => api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "{}\n\n{}",
                error.as_deref().unwrap_or("reboot failed"),
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

/// Fallback handler — anything not matched by `/api/*` falls through
/// here, which means it's a static-asset or SPA-route request.
/// `embedded::serve` resolves the path against the in-binary tree and
/// picks the right content-encoding variant; webadmin mounts at `/`,
/// so we hand the URI through unchanged.
async fn serve_ui(req: axum::extract::Request) -> axum::response::Response {
    let path = req.uri().path().to_string();
    embedded::serve(&path, &req)
}
