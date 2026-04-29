//! `bananas-dashboard` — Slint LCD app for the BPI 5″ panel.
//!
//! Runs as a separate process from the `bananas-stats` daemon and
//! subscribes to its Unix-socket pub/sub at
//! `/run/bananas-stats/live.sock`. Each newline-delimited JSON
//! snapshot the daemon emits is forwarded to a `tokio::sync::watch`
//! channel that `app.rs` already consumes — so the existing UI driver
//! stays untouched, but we no longer open SQLite at all (the daemon
//! holds the in-memory source of truth and only persists to disk
//! every `flush_interval_ms` for historical queries).
//!
//! Slint owns the main thread (required on macOS, idiomatic on Linux).
//! Our tokio runtime runs the socket reader on its own thread pool and
//! `slint::invoke_from_event_loop` (used inside `app.rs`) marshals UI
//! updates back to the event loop.

mod app;

use anyhow::Result;
use bananas_stats::{config, metrics::Snapshot};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Local offset must be read before tokio spawns worker threads —
    // `time::UtcOffset::current_local_offset()` is only sound while the
    // process is single-threaded.
    let local_offset = time::UtcOffset::current_local_offset()
        .unwrap_or(time::UtcOffset::UTC);

    let cfg_path = std::env::args()
        .skip_while(|a| a != "--config")
        .nth(1)
        .map(PathBuf::from)
        .or_else(default_config_path);

    let cfg = match cfg_path {
        Some(p) if p.exists() => config::Config::load(&p)?,
        _ => config::Config::default(),
    };

    tracing::info!(?cfg, ?local_offset, "starting bananas-dashboard");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bananas-dash-rt")
        .build()?;
    let _guard = rt.enter();

    let (snapshot_tx, snapshot_rx) = watch::channel(Snapshot::default());

    // Subscribe to bananas-stats's live socket. Reconnects on failure
    // so a daemon restart doesn't kill the dashboard — the LCD freezes
    // briefly on the last frame, then resumes.
    let socket_path = cfg.live_socket.path.clone();
    rt.spawn(async move {
        if let Err(e) = subscribe_loop(&socket_path, snapshot_tx).await {
            tracing::error!(error = ?e, path = %socket_path.display(),
                "live socket subscriber permanently failed");
        }
    });

    // Hand off to Slint. `app::launch` blocks until the user closes the
    // window or the process is signalled.
    app::launch(cfg.ui, snapshot_rx, local_offset)?;
    Ok(())
}

/// Connect, read newline-delimited JSON, push each parsed Snapshot
/// into the watch channel. On EOF or error, sleep with exponential
/// backoff (200 ms → 5 s) and reconnect.
async fn subscribe_loop(path: &Path, tx: watch::Sender<Snapshot>) -> Result<()> {
    let mut backoff_ms: u64 = 200;
    loop {
        match try_subscribe(path, &tx).await {
            Ok(()) => {
                tracing::debug!("live socket EOF — reconnecting");
                backoff_ms = 200;
            }
            Err(e) => {
                tracing::warn!(error = %e, "live socket connect failed; backing off");
            }
        }
        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        backoff_ms = (backoff_ms * 2).min(5_000);
    }
}

async fn try_subscribe(path: &Path, tx: &watch::Sender<Snapshot>) -> std::io::Result<()> {
    let stream = UnixStream::connect(path).await?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(()); // EOF
        }
        match serde_json::from_str::<Snapshot>(line.trim()) {
            Ok(snap) => {
                let _ = tx.send(snap);
            }
            Err(e) => {
                tracing::warn!(error = ?e, "bad snapshot from live socket");
            }
        }
    }
}

fn default_config_path() -> Option<PathBuf> {
    Some(PathBuf::from("/etc/bananas/dashboard.toml"))
}
