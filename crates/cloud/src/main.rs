//! bananas-cloud — optional plugin daemon for cloud-sync.
//!
//! Owns the `/api/cloud/*` and `/cloud/*` paths. Reached only via
//! `bananas-router`'s reverse proxy on the public TCP port; this
//! daemon listens on `/run/bananas/cloud.sock`. Validates session
//! cookies locally using the same `session.key` that
//! `bananas-webadmin` issues — both daemons are in the same trust
//! domain (run as the `bananas` user, share /var/lib/bananas/).
//!
//! Talks to `bananas-engine` over `/run/bananas/engine.sock` for the
//! one privileged op cloud needs (`RunCloudSync` invokes rclone as a
//! subprocess of the engine, not of this daemon).
//!
//! Step 4 of the plugin migration: this binary BUILDS but is not yet
//! shipped via Yocto. The webadmin still has its own copies of cloud
//! routes (`crates/webadmin/src/cloud.rs` + `cloud_jobs.rs`); this
//! daemon is a parallel implementation that will take over once
//! step 5 ships the IPK and step 6 drops the duplicates.

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::{
    Router,
    extract::Request,
    http::StatusCode,
    middleware::{Next, from_fn_with_state},
    response::Response,
    routing::{get, post, put},
};
use bananas_server_common::{Session, SessionKey, extract_cookie};
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
        .route_layer(from_fn_with_state(state.clone(), require_session))
        .with_state(state);

    // Serve the cloud SPA bundle at /cloud/* from bytes embedded in
    // this binary. `build.rs` ran `dx build --bin bananas-cloud-ui`
    // and staged the output under $OUT_DIR/ui/, which `embedded.rs`
    // pulls in via `include_dir!`. Asset URLs that hit the embedded
    // tree directly get an immutable Cache-Control; SPA deep-links
    // fall back to index.html (no-cache).
    let app = Router::new()
        .nest("/api", api)
        .fallback(get(serve_ui))
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

/// Fallback handler — anything not matched by `/api/*` falls through
/// here, which means it's a static-asset or SPA-route request.
/// bananas-router forwards the path verbatim, so we see the full
/// `/cloud/...` URL; strip that mount prefix before consulting the
/// embedded tree (dx-cli emits assets at `assets/<hash>.{js,wasm,…}`
/// without the `/cloud/` prefix — that prefix only lives in the URL
/// strings written into index.html).
async fn serve_ui(req: Request) -> Response {
    let path = req.uri().path().to_string();
    let rest = path.strip_prefix("/cloud").unwrap_or(path.as_str());
    embedded::serve(rest, &req)
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
