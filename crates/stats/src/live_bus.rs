//! Multi-consumer subscriber for the live-snapshot Unix socket.
//!
//! `bananas-stats` is the sole writer of `/run/bananas/stats.sock`,
//! emitting newline-delimited JSON `Snapshot`s on every sampler
//! tick. Multiple downstream consumers want those snapshots:
//!
//! - `bananas-stats-web` legacy `/api/stats/live` WebSocket route.
//! - `bananas-webadmin` gRPC streaming RPC
//!   (`bananas.stats.v1.StatsService::Live`).
//! - Future: `bananas-dashboard` could share the same multiplex.
//!
//! Each consumer holds a `LiveBus`. The bus opens ONE Unix-socket
//! subscription, parses every line, and re-broadcasts via a
//! `tokio::sync::broadcast` channel. N subscribers = one socket
//! connection regardless of N. The broadcast payload is
//! `Arc<String>` (already-serialized JSON) so we don't re-encode
//! on every fan-out.
//!
//! Reconnects on socket failure with a 200 ms → 5 s exponential
//! backoff so a stats-service restart recovers automatically.

use std::{path::PathBuf, sync::Arc, time::Duration};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::broadcast;

use crate::metrics::Snapshot;

/// Buffer this many snapshots in the broadcast channel. A slow
/// client that falls behind sees a `RecvError::Lagged` and is
/// expected to drop / reconnect — better than queuing memory
/// indefinitely.
const BROADCAST_CAPACITY: usize = 16;

#[derive(Clone)]
pub struct LiveBus {
    tx: broadcast::Sender<Arc<String>>,
}

impl LiveBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);
        Self { tx }
    }

    /// Spawn the subscriber task. Returns immediately; the loop
    /// runs forever, reconnecting on socket failure.
    pub fn start_socket(&self, socket_path: PathBuf) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut backoff_ms: u64 = 200;
            loop {
                match try_subscribe(&socket_path, &tx).await {
                    Ok(()) => {
                        tracing::debug!("live socket EOF — reconnecting");
                        backoff_ms = 200;
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

impl Default for LiveBus {
    fn default() -> Self {
        Self::new()
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
        // Validate the line parses as a Snapshot before
        // forwarding — we don't want to emit garbage to clients
        // if the producer ever sends a corrupt line.
        match serde_json::from_str::<Snapshot>(line.trim()) {
            Ok(_snap) => {
                let payload = Arc::new(line.trim().to_string());
                let _ = tx.send(payload);
            }
            Err(e) => {
                tracing::warn!(error = ?e, "bad snapshot line on live socket");
            }
        }
    }
}
