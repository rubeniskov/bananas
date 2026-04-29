//! Live-snapshot push over WebSocket (`/api/stats/live`).
//!
//! Architecture: ONE background tokio task polls the stats DB once per
//! second and broadcasts the resulting `Snapshot` to a
//! `tokio::sync::broadcast` channel. Every connected websocket gets its
//! own `Receiver` clone, so 0..N clients = exactly one DB read per
//! second regardless. Clients never poll.
//!
//! The auth middleware applies to this route the same way it does to
//! `/api/stats/snapshot` — the WS upgrade is just a regular GET HTTP
//! request that needs the `bananas_session` cookie.

use std::{sync::Arc, time::Duration};

use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use bananas_stats::{metrics::Snapshot, storage::queries};
use tokio::sync::broadcast;

use crate::AppState;

/// Buffer this many snapshots in the broadcast channel. If a slow
/// client falls behind, it sees a `RecvError::Lagged` — we just drop
/// the connection and let it reconnect.
const BROADCAST_CAPACITY: usize = 16;

/// Once-per-app live ticker. Holds the broadcast Sender; subscribers
/// clone their own Receiver. Wired into AppState at startup.
#[derive(Clone)]
pub struct LiveBus {
    tx: broadcast::Sender<Snapshot>,
}

impl LiveBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);
        Self { tx }
    }

    /// Spawn the polling task. Reads `latest_snapshot()` once per
    /// `interval` and broadcasts. If the stats DB isn't open yet, we
    /// skip the tick — readers see no events but the WS stays alive.
    pub fn start(&self, db: Option<Arc<bananas_stats::storage::Database>>, interval: Duration) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let Some(db) = db.as_ref().cloned() else { continue };
                match tokio::task::spawn_blocking(move || queries::latest_snapshot(&db)).await {
                    Ok(Ok(snap)) => {
                        // No subscribers → ignore (broadcast::send Errs).
                        let _ = tx.send(snap);
                    }
                    Ok(Err(e)) => tracing::warn!(error=?e, "latest_snapshot for ws bus failed"),
                    Err(e) => tracing::warn!(error=?e, "ws bus join failed"),
                }
            }
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Snapshot> {
        self.tx.subscribe()
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
            // Bus → client.
            recv = rx.recv() => match recv {
                Ok(snap) => {
                    let payload = match serde_json::to_string(&snap) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::warn!(error=?e, "snapshot serialize failed");
                            continue;
                        }
                    };
                    if socket.send(Message::Text(payload.into())).await.is_err() {
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
