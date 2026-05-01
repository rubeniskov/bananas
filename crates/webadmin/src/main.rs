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
    extract::{Path, State},
    http::StatusCode,
    middleware::from_fn_with_state,
    response::IntoResponse,
    routing::{any, get, post},
};
use bananas_engine::{Command, Response as HelperResponse};
use serde_json::json;
use tower_http::trace::TraceLayer;

mod auth;
mod config;
mod embedded;
mod errors;
mod extensions;
mod grpc;
// `exports` and `fstab` keep the lightweight parsers used by
// `config::build_bundle` + `operations::config_import` to dump
// and restore /etc/exports + /etc/fstab as a TOML bundle. The
// route handlers that wrapped them (POST /api/exports, etc.)
// moved to bananas-exports / bananas-storage in commits 3-4.
mod exports;
mod fstab;
mod operations;
mod proxy;
mod service_config;
mod system;
mod updates;
use bananas_server_common::{Manifest, SessionKey, load_all};

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
    pub helper_socket: Arc<PathBuf>,
    pub session_key: Arc<SessionKey>,
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
        helper_socket: Arc::new(helper_socket),
        session_key: Arc::new(session_key),
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
    // Webadmin's API surface post-extraction: auth + system +
    // updates + operations + config (import/export) +
    // /api/extensions discovery. Per-feature routes
    // (/api/exports, /api/storage, /api/users, /api/stats,
    // /api/dashboard, /api/fstab, /api/permissions, /api/browse,
    // /api/mkdir) live in their own plugin daemons now —
    // bananas-router routes them by manifest prefix.
    let api = Router::new()
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/password", post(auth::change_password))
        .route("/me", get(auth::me))
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/system/config",
            get(service_config::get_system).put(service_config::put_system),
        )
        .route("/system/timezone", post(system::post_timezone))
        .route("/system/timezones", get(system::get_timezones))
        .route("/system/reboot", post(post_reboot))
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
    // Live-snapshot bus — taps bananas-stats's Unix pub/sub once
    // and fans out to every gRPC streaming subscriber. Same
    // multiplex bananas-stats-web uses for its (legacy)
    // WebSocket route. Path is configurable via
    // BANANAS_STATS_LIVE_SOCKET so the e2e harness can point
    // it at a tmpdir socket.
    let live_socket: PathBuf = std::env::var_os("BANANAS_STATS_LIVE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/stats.sock".into());
    let live_bus = bananas_stats::live_bus::LiveBus::new();
    live_bus.start_socket(live_socket);

    // Sync-progress dir — bananas-engine writes per-line rclone
    // output to `<dir>/<idx>.log` while a sync runs. The
    // CloudService::TailRunLog RPC streams from there.
    let sync_progress_dir: PathBuf = std::env::var_os("BANANAS_SYNC_PROGRESS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/sync-progress".into());

    // gRPC stack: every `/api/grpc/*` path is handled by tonic
    // (with tonic-web translating browser-side gRPC-Web frames
    // to native gRPC). Paths like
    // `/api/grpc/bananas.health.v1.HealthService/Check` are
    // stripped to `/bananas.health.v1.HealthService/Check`
    // before tonic dispatches. Everything else (legacy /api/*
    // JSON, /assets/*, /) keeps its existing paths during the
    // gRPC migration. PR-5 retires the JSON sub-proxy.
    let grpc_routes = grpc::build_grpc_router(live_bus, sync_progress_dir);
    let grpc_axum: axum::Router = grpc_routes.into_axum_router();
    let grpc_service = tower::ServiceBuilder::new()
        .layer(tonic_web::GrpcWebLayer::new())
        .service(grpc_axum);

    let public_app = Router::new()
        // gRPC mount — strip the `/api/grpc` prefix before
        // tonic's path matcher sees the request.
        .nest_service("/api/grpc", grpc_service)
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

// /api/exports/*, /api/storage, /api/fstab/*, /api/permissions,
// /api/users/*, /api/stats/*, /api/dashboard/*, /api/browse,
// /api/mkdir all moved to per-feature plugin daemons in commits
// 3-7. Webadmin's /api/* surface post-extraction is auth +
// system + updates + operations + config import/export +
// extension discovery — see the route block in `main()` above.

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

/// Fallback handler — anything not matched by `/api/*` falls through
/// here, which means it's a static-asset or SPA-route request.
/// `embedded::serve` resolves the path against the in-binary tree and
/// picks the right content-encoding variant; webadmin mounts at `/`,
/// so we hand the URI through unchanged.
async fn serve_ui(req: axum::extract::Request) -> axum::response::Response {
    let path = req.uri().path().to_string();
    embedded::serve(&path, &req)
}
