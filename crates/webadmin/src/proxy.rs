//! Sub-proxy from the public TCP listener to a Unix-socket-backed
//! upstream. Used in two places:
//!
//! 1. `/api/*`        — forwarded to `bananas-router`, which routes
//!                      by manifest `api_prefix` to the right plugin
//!                      daemon. Webadmin doesn't duplicate the
//!                      manifest table; the router is the single
//!                      source of truth for API → socket mapping.
//! 2. `/assets/<id>/*` — forwarded directly to the plugin daemon
//!                      whose manifest declares `id`. Webadmin reads
//!                      the manifest dir at startup and keeps an
//!                      id → socket map; unknown ids 404.
//!
//! The body streams in both directions — request and response are
//! forwarded with `axum::body::Body` end-to-end, never buffered.
//! Cookies + every other header pass through untouched, so each
//! upstream daemon enforces its own auth boundary.

use std::path::Path;

use axum::{
    body::Body,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

/// Forward `req` to the daemon listening on `socket`. Returns the
/// upstream response (status, headers, streamed body) verbatim, or
/// a 502 if the socket is unreachable / the HTTP handshake fails.
pub async fn proxy_to_unix(socket: &Path, req: Request) -> Response {
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(socket = %socket.display(), error = %e, "upstream socket unreachable");
            return (
                StatusCode::BAD_GATEWAY,
                format!("upstream socket {} unreachable: {e}", socket.display()),
            )
                .into_response();
        }
    };

    let io = TokioIo::new(stream);
    let (mut sender, conn) = match http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(socket = %socket.display(), error = %e, "HTTP handshake failed");
            return (
                StatusCode::BAD_GATEWAY,
                format!("HTTP handshake to {} failed: {e}", socket.display()),
            )
                .into_response();
        }
    };
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!(error = %e, "proxy connection ended");
        }
    });

    // axum's Body implements hyper::body::Body, so the request as-is
    // can be forwarded.
    let req = req.map(|b| b.boxed_unsync());
    match sender.send_request(req).await {
        Ok(resp) => {
            let (parts, body) = resp.into_parts();
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("upstream send to {} failed: {e}", socket.display()),
        )
            .into_response(),
    }
}
