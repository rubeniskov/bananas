//! bananas-router — path-based reverse proxy gateway.
//!
//! Listens on `:8080` (the public port). Reads extension manifests from
//! `/etc/bananas/extensions.d/*.toml` at startup. Each manifest declares
//! the path prefixes an extension owns and the Unix socket its daemon
//! listens on. Every incoming HTTP request is matched against the
//! manifest table by longest-prefix, then forwarded over a fresh
//! `UnixStream` to the matching daemon — request and response stream
//! through unchanged (cookies forward verbatim so each daemon enforces
//! its own auth boundary).
//!
//! The router has no business logic, no SPA serving, no auth handling
//! — it's the gateway. Plugins (bananas-webadmin core, bananas-cloud,
//! whatever lands next) are the daemons that actually answer requests.
//!
//! Manifests are read at startup. `systemctl reload bananas-router`
//! re-reads them so freshly opkg-installed extensions become reachable
//! without a hard restart.
//!
//! Env vars:
//!   BANANAS_LISTEN_ADDR        — defaults to "0.0.0.0:8080"
//!   BANANAS_EXTENSIONS_DIR     — defaults to "/etc/bananas/extensions.d"

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::net::UnixStream;
use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;

/// One extension's manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    /// Stable id, used in tracing + the `/api/extensions` listing.
    pub id: String,
    /// Human-readable label for the SPA's nav.
    #[serde(default)]
    pub label: Option<String>,
    /// Unix socket the extension's daemon listens on.
    pub socket: PathBuf,
    /// Path prefixes this extension owns. Longest match wins.
    pub prefixes: Vec<String>,
    /// Optional SPA path the core webadmin should navigate to when
    /// the user clicks the extension's nav tab.
    #[serde(default)]
    pub spa_path: Option<String>,
}

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
    let extensions = load_manifests(&manifests_dir);
    tracing::info!(
        count = extensions.len(),
        dir = %manifests_dir.display(),
        "loaded extension manifests"
    );
    for m in &extensions {
        tracing::info!(
            id = %m.id,
            socket = %m.socket.display(),
            prefixes = ?m.prefixes,
            "extension"
        );
    }

    let state = AppState {
        extensions: Arc::new(RwLock::new(extensions)),
    };

    let app = Router::new().fallback(proxy).with_state(state);

    let addr: SocketAddr = std::env::var("BANANAS_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    tracing::info!(%addr, "bananas-router listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Parse every `*.toml` under `dir`. Bad files are logged and skipped
/// rather than fatal — a single broken manifest shouldn't take the
/// gateway offline.
fn load_manifests(dir: &Path) -> Vec<Manifest> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(dir = %dir.display(), "manifest dir does not exist; no extensions loaded");
            return out;
        }
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "manifest dir read failed");
            return out;
        }
    };
    let mut by_id: HashMap<String, Manifest> = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "manifest read failed");
                continue;
            }
        };
        match toml::from_str::<Manifest>(&body) {
            Ok(m) => {
                if let Some(existing) = by_id.insert(m.id.clone(), m.clone()) {
                    tracing::warn!(
                        id = %m.id,
                        prev = %existing.socket.display(),
                        new = %m.socket.display(),
                        "duplicate manifest id; later definition wins"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "manifest parse failed; skipping");
            }
        }
    }
    out.extend(by_id.into_values());
    out
}

/// Match `path` against the manifest table; longest matching prefix wins.
fn lookup<'a>(extensions: &'a [Manifest], path: &str) -> Option<&'a Manifest> {
    let mut best: Option<&Manifest> = None;
    let mut best_len: usize = 0;
    for m in extensions {
        for prefix in &m.prefixes {
            let trimmed = prefix.trim_end_matches('/');
            let matches = if trimmed.is_empty() {
                true // root matches everything
            } else {
                path == trimmed || path.starts_with(&format!("{trimmed}/"))
            };
            if matches && prefix.len() >= best_len {
                best = Some(m);
                best_len = prefix.len();
            }
        }
    }
    best
}

/// Forward a request to the extension whose prefix matches `req.uri().path()`.
/// Returns 404 (no extension matches) or 502 (extension socket unreachable
/// / handshake fails).
async fn proxy(State(state): State<AppState>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let extensions = state.extensions.read().await;
    let Some(m) = lookup(&extensions, &path) else {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, socket: &str, prefixes: &[&str]) -> Manifest {
        Manifest {
            id: id.into(),
            label: None,
            socket: PathBuf::from(socket),
            prefixes: prefixes.iter().map(|s| s.to_string()).collect(),
            spa_path: None,
        }
    }

    #[test]
    fn longest_prefix_wins() {
        let exts = vec![
            manifest("core", "/run/core.sock", &["/"]),
            manifest("cloud", "/run/cloud.sock", &["/api/cloud", "/cloud"]),
        ];
        assert_eq!(lookup(&exts, "/api/cloud/runs").unwrap().id, "cloud");
        assert_eq!(lookup(&exts, "/api/exports").unwrap().id, "core");
        assert_eq!(lookup(&exts, "/cloud/page").unwrap().id, "cloud");
        assert_eq!(lookup(&exts, "/").unwrap().id, "core");
    }

    #[test]
    fn no_match_when_no_root_extension() {
        let exts = vec![manifest("cloud", "/run/cloud.sock", &["/api/cloud"])];
        assert!(lookup(&exts, "/api/exports").is_none());
        assert!(lookup(&exts, "/").is_none());
    }

    #[test]
    fn root_prefix_matches_everything() {
        let exts = vec![manifest("core", "/run/core.sock", &["/"])];
        assert_eq!(lookup(&exts, "/").unwrap().id, "core");
        assert_eq!(lookup(&exts, "/anything").unwrap().id, "core");
        assert_eq!(lookup(&exts, "/deep/nested/path").unwrap().id, "core");
    }

    #[test]
    fn exact_prefix_match_does_not_overshoot() {
        let exts = vec![
            manifest("core", "/run/core.sock", &["/"]),
            manifest("cloud", "/run/cloud.sock", &["/api/cloud"]),
        ];
        // /api/cloudfoo should NOT match the cloud prefix
        assert_eq!(lookup(&exts, "/api/cloudfoo").unwrap().id, "core");
    }

    #[test]
    fn load_manifests_skips_unparseable() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(
            td.path().join("ok.toml"),
            r#"id = "ok"
socket = "/run/ok.sock"
prefixes = ["/api/ok"]"#,
        )
        .unwrap();
        std::fs::write(td.path().join("bad.toml"), "not = valid = toml = at all").unwrap();
        let v = load_manifests(td.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "ok");
    }

    #[test]
    fn load_manifests_handles_missing_dir() {
        let v = load_manifests(Path::new("/nonexistent/path/here"));
        assert!(v.is_empty());
    }
}
