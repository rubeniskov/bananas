//! bananas-stats-web — stats viewer plugin daemon.
//!
//! Lives inside crates/stats alongside the collector daemon.
//! Reads the same SQLite DB the collector writes (read-only) and
//! subscribes to its live socket for WS push to the SPA. Owns
//! `/api/stats/*` (auth-gated) and `/assets/stats/*` (static).
//!
//! All operations are read-only against the DB; the
//! `/api/stats/config` writer goes through bananas-engine like
//! every other config write.

#![allow(clippy::too_many_arguments)]

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Request},
    http::StatusCode,
    middleware::{Next, from_fn_with_state},
    response::Response,
    routing::{get, put},
};
use bananas_server_common::{Session, SessionKey, extract_cookie};
use serde_json::json;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod embedded;
mod handlers;
mod ws;

#[derive(Clone)]
pub struct AppState {
    pub helper_socket: Arc<PathBuf>,
    pub session_key: Arc<SessionKey>,
    pub stats: handlers::StatsState,
    pub live_bus: ws::LiveBus,
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
                .unwrap_or_else(|_| "bananas_stats_web=info,tower_http=info".into()),
        )
        .init();

    let session_key_path: PathBuf = std::env::var_os("BANANAS_SESSION_KEY")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/bananas/session.key".into());
    let session_key = SessionKey::load_or_create(&session_key_path)?;

    let helper_socket: PathBuf = std::env::var_os("BANANAS_ENGINE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/engine.sock".into());

    let stats_db_path: PathBuf = std::env::var_os("BANANAS_STATS_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/bananas/stats.db".into());
    let stats = handlers::StatsState::open(&stats_db_path);

    let live_bus = ws::LiveBus::new();
    let live_socket_path: PathBuf = std::env::var_os("BANANAS_STATS_LIVE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/stats.sock".into());
    live_bus.start_socket(live_socket_path);

    let state = AppState {
        helper_socket: Arc::new(helper_socket),
        session_key: Arc::new(session_key),
        stats,
        live_bus,
    };

    let api = Router::new()
        .route("/stats/snapshot", get(handlers::snapshot))
        .route("/stats/range", get(handlers::range))
        .route("/stats/series", get(handlers::series))
        .route("/stats/live", get(ws::live))
        .route(
            "/stats/config",
            get(handlers::get_config).put(handlers::put_config),
        )
        .route("/stats/__mfe_entry", get(mfe_entry))
        .route_layer(from_fn_with_state(state.clone(), require_session))
        .with_state(state);

    let app = Router::new()
        .nest("/api", api)
        .route("/assets/stats/{*rest}", get(serve_asset))
        .layer(TraceLayer::new_for_http());

    let socket_path: PathBuf = std::env::var_os("BANANAS_STATS_WEB_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/stats-web.sock".into());
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).ok();
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o660))?;
    tracing::info!(socket = %socket_path.display(), "bananas-stats-web listening");
    axum::serve(listener, app).await?;
    Ok(())
}

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
