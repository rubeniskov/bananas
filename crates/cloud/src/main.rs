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
    http::{StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{Html, Response},
    routing::{get, post, put},
};
use bananas_server_common::{Session, SessionKey, extract_cookie};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer, trace::TraceLayer};
use tracing_subscriber::EnvFilter;

mod cloud;
mod cloud_jobs;

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

    // Serve the cloud SPA bundle at /cloud/* from the BANANAS_CLOUD_UI_DIR
    // (default /usr/share/bananas/cloud-ui/, populated by the
    // bananas-cloud-ui IPK). Static assets are immutable-cached
    // because dx-cli emits content-hashed filenames; the SPA
    // fallback reads index.html per request so a future bananas-cloud-ui
    // upgrade lands without restarting this daemon.
    let ui_dir: PathBuf = std::env::var_os("BANANAS_CLOUD_UI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/usr/share/bananas/cloud-ui".into());
    let index_html_path = Arc::new(ui_dir.join("index.html"));
    if let Err(e) = std::fs::metadata(&*index_html_path) {
        tracing::warn!(
            path = %index_html_path.display(),
            error = %e,
            "cloud-ui index.html is not readable at startup"
        );
    }
    let assets = ServeDir::new(ui_dir.join("assets"))
        .precompressed_br()
        .precompressed_gzip();

    let app = Router::new()
        .nest("/api", api)
        .nest_service(
            "/cloud/assets",
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("public, max-age=31536000, immutable"),
                ))
                .service(assets),
        )
        .fallback(get({
            let index_html_path = index_html_path.clone();
            move || {
                let path = index_html_path.clone();
                async move {
                    match tokio::fs::read_to_string(&*path).await {
                        Ok(html) => (
                            StatusCode::OK,
                            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                            Html(html),
                        ),
                        Err(e) => {
                            tracing::warn!(path = %path.display(), error = %e, "cloud SPA fallback read failed");
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                                Html(String::from("cloud-ui bundle missing on disk")),
                            )
                        }
                    }
                }
            }
        }))
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
