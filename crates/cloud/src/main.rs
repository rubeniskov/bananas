//! bananas-cloud — optional plugin daemon for cloud-sync.
//!
//! Listens only on `/run/bananas/cloud.sock`. Two URL spaces it
//! answers:
//!  - `/api/cloud/*` — auth-gated JSON API (forwarded by
//!    bananas-router from webadmin's public TCP).
//!  - `/assets/cloud/*` — strict asset lookup against the bytes
//!    embedded via `include_dir!` (forwarded by webadmin's
//!    plugin-asset sub-proxy, also over Unix socket).
//!
//! There is no public-facing SPA fallback. The host webadmin owns
//! THE single `index.html`; the cloud SPA is composed inline at
//! runtime via the MFE loader, which discovers this daemon's
//! content-hashed entry script through `/api/cloud/__mfe_entry`.
//!
//! Validates session cookies locally using the same `session.key`
//! that bananas-webadmin issues — both daemons share the bananas
//! service user and `/var/lib/bananas/`.

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Request},
    http::StatusCode,
    middleware::{Next, from_fn_with_state},
    response::Response,
    routing::{get, post, put},
};
use bananas_server_common::{Session, SessionKey, extract_cookie};
use serde_json::json;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod cloud;
mod cloud_jobs;
mod embedded;

#[derive(Clone)]
pub struct AppState {
    pub helper_socket: Arc<PathBuf>,
    pub session_key: Arc<SessionKey>,
    pub jobs: cloud_jobs::JobManager,
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
                .unwrap_or_else(|_| "bananas_cloud=info,tower_http=info".into()),
        )
        .init();

    let session_key_path: PathBuf = std::env::var_os("BANANAS_SESSION_KEY")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/bananas/session.key".into());
    let session_key = SessionKey::load_or_create(&session_key_path)?;

    let helper_socket: PathBuf = std::env::var_os("BANANAS_ENGINE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/engine.sock".into());

    let jobs = cloud_jobs::JobManager::new();
    cloud_jobs::spawn_scheduler(jobs.clone(), helper_socket.clone());

    let state = AppState {
        helper_socket: Arc::new(helper_socket),
        session_key: Arc::new(session_key),
        jobs,
    };

    let api = Router::new()
        .route("/cloud/providers", get(cloud::providers))
        .route(
            "/cloud/accounts",
            get(cloud::list_accounts).post(cloud::add_account),
        )
        .route(
            "/cloud/accounts/{name}",
            put(cloud::update_account).delete(cloud::delete_account),
        )
        .route("/cloud/syncs", get(cloud::list_syncs).post(cloud::add_sync))
        .route(
            "/cloud/syncs/{idx}",
            put(cloud::update_sync).delete(cloud::delete_sync),
        )
        .route("/cloud/syncs/{idx}/run", post(cloud::run_sync))
        .route("/cloud/syncs/{idx}/cancel", post(cloud::cancel_sync))
        .route("/cloud/runs", get(cloud::list_runs))
        .route("/cloud/runs/{job_id}", get(cloud::get_run))
        // The MFE entry handshake: returns the content-hashed JS
        // shim URL that the host webadmin's loader injects as a
        // <script type="module"> tag. Auth-gated like every other
        // /api/cloud/* route, so unauthenticated visitors can't
        // even enumerate which plugins are installed.
        .route("/cloud/__mfe_entry", get(mfe_entry))
        .route_layer(from_fn_with_state(state.clone(), require_session))
        .with_state(state);

    // Asset handler — webadmin sub-proxies /assets/cloud/<rest>
    // verbatim to this daemon's socket. `embedded::serve` does a
    // strict include_dir lookup; index.html is never served, only
    // content-hashed asset files. No auth gate here: the bytes are
    // public (compiled wasm + JS shim), and the host SPA can't
    // discover the URL without a session anyway.
    let app = Router::new()
        .nest("/api", api)
        .route("/assets/cloud/{*rest}", get(serve_asset))
        .layer(TraceLayer::new_for_http());

    // Always Unix socket — the public TCP port is owned by
    // bananas-router. TCP listener is intentionally not provided here
    // (unlike bananas-webadmin's transitional fallback); cloud is a
    // plugin and there is no legacy "direct TCP" deployment to
    // preserve.
    let socket_path: PathBuf = std::env::var_os("BANANAS_CLOUD_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/cloud.sock".into());
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).ok();
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o660))?;
    tracing::info!(socket = %socket_path.display(), "bananas-cloud listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// `/api/cloud/__mfe_entry` handler — returns the content-hashed
/// JS shim URL the host webadmin injects to load this plugin's
/// SPA. The URL is in webadmin's public namespace
/// (`/assets/cloud/<hash>.js`) because Dioxus.toml's `base_path`
/// is configured to that prefix; the host SPA can use the value
/// verbatim as a `<script src>`.
async fn mfe_entry() -> Json<serde_json::Value> {
    Json(json!({ "entry": &*embedded::MFE_ENTRY }))
}

/// `/assets/cloud/{*rest}` handler — webadmin sub-proxies asset
/// requests over our Unix socket. `embedded::serve` does a strict
/// include_dir lookup against the dx-built tree.
async fn serve_asset(Path(rest): Path<String>, req: Request) -> Response {
    embedded::serve(&rest, &req)
}

/// Auth middleware — same shape as bananas-webadmin's
/// `auth::require_session`, just smaller (no exempt list, since cloud
/// has no public endpoints). Validates the session cookie issued by
/// bananas-webadmin's `/api/login`.
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
