//! Live-snapshot push over WebSocket (`/api/stats/live`).
//!
//! Architecture: ONE background tokio task subscribes to bananas-stats's
//! Unix-socket pub/sub at `/run/bananas-stats/live.sock`, parses each
//! incoming line as a `Snapshot`, and re-broadcasts to a
//! `tokio::sync::broadcast` channel. Every connected websocket gets
//! its own broadcast receiver, so 0..N web clients = one socket
//! subscription regardless of N. SQLite is no longer touched for live
//! data — bananas-stats is the sole owner of in-memory state, and the
//! DB is only read by `/api/stats/range` for historical queries.
//!
//! The auth middleware applies to this route the same way it does to
//! `/api/stats/snapshot` — the WS upgrade is just a regular GET HTTP
//! request that needs the `bananas_session` cookie.

use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use bananas_stats::metrics::Snapshot;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::broadcast;

use crate::AppState;

/// Buffer this many snapshots in the broadcast channel. If a slow
/// client falls behind, it sees a `RecvError::Lagged` — we just drop
/// the connection and let it reconnect.
const BROADCAST_CAPACITY: usize = 16;

/// Once-per-app live ticker. Holds the broadcast Sender; subscribers
/// clone their own Receiver. We broadcast `Arc<String>` (already-
/// serialized JSON) instead of the raw `Snapshot` so N WebSocket
/// clients don't each re-encode the same payload per tick — at
/// 1 Hz × N clients that's measurable on the BPI's CPU. The Arc keeps
/// the broadcast itself O(1) regardless of N.
#[derive(Clone)]
pub struct LiveBus {
    tx: broadcast::Sender<Arc<String>>,
}

impl LiveBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);
        Self { tx }
    }

    /// Connect to bananas-stats's live Unix socket and re-broadcast
    /// every snapshot it sends to all WS subscribers. Reconnects on
    /// failure with exponential backoff (200 ms → 5 s) so the bus
    /// recovers from a stats-service restart without operator
    /// intervention.
    pub fn start_socket(&self, socket_path: PathBuf) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut backoff_ms: u64 = 200;
            loop {
                match try_subscribe(&socket_path, &tx).await {
                    Ok(()) => {
                        tracing::debug!("live socket EOF — reconnecting");
                        backoff_ms = 200; // healthy session before EOF, reset
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e, path = %socket_path.display(),
                            "live socket subscribe failed; backing off"
                        );
                    }
                }
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(5_000);
            }
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<String>> {
        self.tx.subscribe()
    }
}

async fn try_subscribe(
    path: &std::path::Path,
    tx: &broadcast::Sender<Arc<String>>,
) -> std::io::Result<()> {
    let stream = UnixStream::connect(path).await?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(()); // EOF — peer closed
        }
        // The line we read from bananas-stats is already a serialized
        // Snapshot. We only need to validate it's a structurally-correct
        // Snapshot (so a corrupt line doesn't reach browsers as opaque
        // garbage), then forward the original string. Skipping
        // re-serialization saves an alloc + memcpy per tick × N clients.
        match serde_json::from_str::<Snapshot>(line.trim()) {
            Ok(_snap) => {
                let payload = Arc::new(line.trim().to_string());
                // broadcast::send returns Err only when there are no
                // subscribers; ignore that — clients reconnecting later
                // still get fresh data.
                let _ = tx.send(payload);
            }
            Err(e) => {
                tracing::warn!(error = ?e, "bad snapshot line on live socket");
            }
        }
    }
}

pub async fn live(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let bus = state.live_bus.clone();
    ws.on_upgrade(move |socket| handle(socket, bus))
}

async fn handle(mut socket: WebSocket, bus: LiveBus) {
    let mut rx = bus.subscribe();
    loop {
        tokio::select! {
            // Bus → client. The payload is already a JSON-encoded
            // Snapshot string in an Arc — no re-encode needed.
            recv = rx.recv() => match recv {
                Ok(payload) => {
                    if socket.send(Message::Text(payload.as_str().to_string().into())).await.is_err() {
                        break; // client gone
                    }
                }
                // Slow consumer — easier to drop them and let the
                // browser reconnect than to negotiate catch-up.
                Err(broadcast::error::RecvError::Lagged(_)) => break,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            // Client → us. We don't expect commands; just consume so
            // the underlying stream can detect close cleanly.
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}
