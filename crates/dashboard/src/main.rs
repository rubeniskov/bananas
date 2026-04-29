//! `bananas-dashboard` — Slint LCD app for the BPI 5″ panel.
//!
//! Runs as a separate process from the `bananas-stats` daemon. Opens
//! `/var/lib/bananas/stats.db` in WAL-shared mode and polls the latest
//! snapshot (most recent row per metric) on each `interval_ms`,
//! reshaping it into the same `Snapshot` struct that `app.rs` already
//! consumes. This way the original UI driver stays untouched: we just
//! feed it through a `tokio::sync::watch` channel like the in-process
//! version did.
//!
//! Slint owns the main thread (required on macOS, idiomatic on Linux).
//! Our tokio runtime runs the polling loop on its own thread pool and
//! `slint::invoke_from_event_loop` (used inside `app.rs`) marshals UI
//! updates back to the event loop.

mod app;

use anyhow::{Context, Result};
use bananas_stats::{config, metrics::Snapshot, storage};
use std::path::PathBuf;
use std::time::Duration;
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

    // Open the stats DB read-only — the daemon writes; we only consume.
    let db = storage::Database::open(&cfg.storage)
        .context("opening stats DB (is bananas-stats running?)")?;

    let (snapshot_tx, snapshot_rx) = watch::channel(Snapshot::default());

    // Polling loop: every interval_ms ask the SQLite for the latest
    // snapshot across all metrics, broadcast over the watch channel.
    let poll_db = db.clone();
    let poll_interval = Duration::from_millis(cfg.sampling.interval_ms.max(100));
    rt.spawn(async move {
        let mut tick = tokio::time::interval(poll_interval);
        loop {
            tick.tick().await;
            match storage::queries::latest_snapshot(&poll_db) {
                Ok(snap) => {
                    let _ = snapshot_tx.send(snap);
                }
                Err(e) => tracing::warn!(error=?e, "latest_snapshot failed"),
            }
        }
    });

    // Hand off to Slint. `app::launch` blocks until the user closes the
    // window or the process is signalled.
    app::launch(cfg.ui, snapshot_rx, db, local_offset)?;
    Ok(())
}

fn default_config_path() -> Option<PathBuf> {
    Some(PathBuf::from("/etc/bananas/dashboard.toml"))
}
