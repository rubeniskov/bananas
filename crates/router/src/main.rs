//! bananas-router — internal API gateway over Unix socket.
//!
//! No public listener. `bananas-webadmin` is the public TCP face;
//! it sub-proxies every `/api/*` request to this daemon over
//! `/run/bananas/router.sock`. We read manifests from
//! `/etc/bananas/extensions.d/`, longest-prefix-match the request
//! path against each manifest's `api_prefix`, and forward to the
//! matching plugin daemon's Unix socket. Cookies (and every other
//! header) pass through untouched — each plugin daemon enforces
//! its own auth.
//!
//! Webadmin's own /api endpoints (login, me, users, …) are reached
//! via this same router by configuring webadmin's manifest with
//! the catch-all `api_prefix = "/api"`. The TCP listener and the
//! Unix listener are both bound by the webadmin process; the TCP
//! one only does proxy/static, the Unix one does the API work.
//!
//! `systemctl reload bananas-router` re-reads the manifest dir so
//! freshly opkg-installed plugins become reachable without a
//! daemon restart.
//!
//! Env vars:
//!   BANANAS_ROUTER_SOCKET   — defaults to "/run/bananas/router.sock"
//!   BANANAS_EXTENSIONS_DIR  — defaults to "/etc/bananas/extensions.d"

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bananas_server_common::{Manifest, load_all, match_prefix};
use http_body_util::BodyExt;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    extensions: Arc<RwLock<Vec<Manifest>>>,
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
                .unwrap_or_else(|_| "bananas_router=info,tower_http=info".into()),
        )
        .init();

    let manifests_dir: PathBuf = std::env::var_os("BANANAS_EXTENSIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/bananas/extensions.d".into());
    let extensions = load_all(&manifests_dir);
    tracing::info!(
        count = extensions.len(),
        dir = %manifests_dir.display(),
        "loaded extension manifests"
    );
    for m in &extensions {
        tracing::info!(
            id = %m.id,
            socket = %m.socket.display(),
            api_prefix = %m.api_prefix,
            "extension"
        );
    }

    let state = AppState {
        extensions: Arc::new(RwLock::new(extensions)),
    };

    let app = Router::new().fallback(proxy).with_state(state);

    let socket_path: PathBuf = std::env::var_os("BANANAS_ROUTER_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas/router.sock".into());
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).ok();
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    use std::os::unix::fs::PermissionsExt;
    // 0660 — webadmin (also bananas user) connects from one side,
    // root can connect for diagnostics. No outsider has access.
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o660))?;
    tracing::info!(socket = %socket_path.display(), "bananas-router listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Forward a request to the extension whose `api_prefix` matches
/// `req.uri().path()`. Returns 404 (no extension claims this
/// path) or 502 (extension socket unreachable / handshake fails).
async fn proxy(State(state): State<AppState>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let extensions = state.extensions.read().await;
    let Some(m) = match_prefix(&extensions, &path) else {
        return (
            StatusCode::NOT_FOUND,
            "no extension matches this path".to_string(),
        )
            .into_response();
    };
    let socket = m.socket.clone();
    let extension_id = m.id.clone();
    drop(extensions);

    let stream = match UnixStream::connect(&socket).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(socket = %socket.display(), error = %e, "extension socket unreachable");
            return (
                StatusCode::BAD_GATEWAY,
                format!("extension '{extension_id}' socket unreachable: {e}"),
            )
                .into_response();
        }
    };

    let io = TokioIo::new(stream);
    let (mut sender, conn) = match http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("HTTP handshake to '{extension_id}' failed: {e}"),
            )
                .into_response();
        }
    };
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!(error = %e, "proxy connection ended");
        }
    });

    // axum's Body implements hyper::body::Body, so the request as-is can
    // be forwarded. Cookies + every other header pass through verbatim;
    // the extension daemon validates auth itself.
    let req = req.map(|b| b.boxed_unsync());
    match sender.send_request(req).await {
        Ok(resp) => {
            let (parts, body) = resp.into_parts();
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("upstream send to '{extension_id}' failed: {e}"),
        )
            .into_response(),
    }
}
