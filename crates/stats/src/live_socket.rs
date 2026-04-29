//! Unix-socket pub/sub for live snapshots.
//!
//! Replaces the previous "every consumer re-reads SQLite" pattern.
//! `bananas-stats` is the sole writer of in-memory state; readers
//! (`bananas-server` for the web UI's WebSocket, `bananas-dashboard`
//! for the LCD) connect to this socket and get newline-delimited
//! JSON snapshots, one per sampler tick.
//!
//! Wire format: each snapshot is a single line of `serde_json::to_string(&Snapshot)`
//! followed by `\n`. Empty / partial lines are not emitted.
//!
//! Backpressure: we use the same `tokio::sync::watch` channel the
//! sampler already publishes to, so a slow client only sees the
//! *latest* snapshot available — never a queue. That's the right
//! semantics for live monitoring (you don't want to scroll through
//! old data), and means we never grow memory for a stuck client.

use crate::metrics::Snapshot;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

/// Bind a Unix socket at `path` and serve newline-delimited JSON
/// snapshots to every connecting client. Runs forever — caller
/// `tokio::spawn`s it.
pub async fn serve(path: PathBuf, rx: watch::Receiver<Snapshot>) -> Result<()> {
    // Make sure the parent dir exists. systemd's RuntimeDirectory=
    // normally takes care of /run/bananas-stats, but creating it here
    // lets `cargo run` work on a dev host too.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Remove any stale socket from a previous (crashed) run; bind()
    // would otherwise return EADDRINUSE.
    let _ = std::fs::remove_file(&path);

    let listener = UnixListener::bind(&path)
        .with_context(|| format!("binding live socket at {}", path.display()))?;

    tracing::info!(path = %path.display(), "live socket listening");

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = ?e, "live socket accept failed");
                continue;
            }
        };
        let rx = rx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, rx).await {
                tracing::debug!(error = ?e, "live socket client ended");
            }
        });
    }
}

async fn handle_client(mut stream: UnixStream, mut rx: watch::Receiver<Snapshot>) -> Result<()> {
    // Send the current value immediately on connect so the consumer
    // doesn't have to wait up to `interval_ms` for the first frame.
    {
        let snap = rx.borrow().clone();
        if snap.ts_unix > 0 {
            send_snapshot(&mut stream, &snap).await?;
        }
    }
    while rx.changed().await.is_ok() {
        let snap = rx.borrow_and_update().clone();
        send_snapshot(&mut stream, &snap).await?;
    }
    Ok(())
}

async fn send_snapshot(stream: &mut UnixStream, snap: &Snapshot) -> std::io::Result<()> {
    let mut payload = serde_json::to_vec(snap)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    payload.push(b'\n');
    stream.write_all(&payload).await
}

/// Default location for the live socket. Mirrors bananas-stats's
/// systemd `RuntimeDirectory=bananas-stats` so the path lives under
/// `/run/bananas-stats/` on-device. Dev hosts without that dir fall
/// back to /tmp.
pub fn default_socket_path() -> PathBuf {
    let runtime = Path::new("/run/bananas-stats");
    if runtime.exists() {
        runtime.join("live.sock")
    } else if Path::new("/run").exists() {
        // We'll create /run/bananas-stats below; sane default for the
        // first-boot case where the dir doesn't exist yet.
        runtime.join("live.sock")
    } else {
        PathBuf::from("/tmp/bananas-stats-live.sock")
    }
}
