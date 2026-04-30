//! `bananas-stats` daemon — runs the sampler on a tokio runtime and
//! persists snapshots to SQLite. No UI; readers (web admin and the LCD
//! dashboard) consume via the on-disk WAL store.

use anyhow::Result;
use bananas_stats::{config, live_socket, metrics, storage};
use std::path::PathBuf;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cfg_path = std::env::args()
        .skip_while(|a| a != "--config")
        .nth(1)
        .map(PathBuf::from)
        .or_else(default_config_path);

    let cfg = match cfg_path {
        Some(p) if p.exists() => config::Config::load(&p)?,
        _ => config::Config::default(),
    };

    tracing::info!(?cfg, "starting bananas-stats");

    let db = storage::Database::open(&cfg.storage)?;
    let (snapshot_tx, snapshot_rx) = watch::channel(metrics::Snapshot::default());

    let sampler = metrics::Sampler::new(
        cfg.sampling.clone(),
        cfg.devices.clone(),
        cfg.network.clone(),
        snapshot_tx,
    );
    let writer = storage::Writer::new(db.clone(), cfg.storage.clone(), snapshot_rx.clone());

    // Live socket pub/sub. Subscribers (bananas-server's WS bus,
    // bananas-dashboard's render loop) connect here and receive one
    // newline-delimited JSON snapshot per sampler tick. The watch
    // channel guarantees slow consumers only see the *latest* — never
    // a backlog — so memory stays bounded regardless of how many
    // subscribers misbehave.
    let socket_path = cfg.live_socket.path.clone();
    let socket_rx = snapshot_rx.clone();
    tokio::spawn(async move {
        if let Err(e) = live_socket::serve(socket_path, socket_rx).await {
            tracing::error!(error = ?e, "live socket server died");
        }
    });

    tokio::spawn(sampler.run());
    tokio::spawn(writer.run());

    tokio::signal::ctrl_c().await.ok();
    tracing::info!("shutdown signal — flushing");
    drop(snapshot_rx);
    drop(db);
    Ok(())
}

fn default_config_path() -> Option<PathBuf> {
    Some(PathBuf::from("/etc/bananas/stats.toml"))
}
