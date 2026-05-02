//! Live-snapshot push over WebSocket (`/api/stats/live`).
//!
//! The fan-out logic + Unix-socket subscriber moved to
//! `bananas_stats::live_bus::LiveBus` so webadmin's gRPC streaming
//! RPC (`bananas.stats.v1.StatsService::Live`) can share the same
//! multiplex. This module is now just the axum WebSocket adapter
//! around a `LiveBus` instance.
//!
//! The legacy `/api/stats/live` route stays available during the
//! gRPC transition; PR-5 removes it once every consumer is on the
//! streaming RPC.

use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
pub use bananas_stats::live_bus::LiveBus;
use tokio::sync::broadcast;

use super::AppState;

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
