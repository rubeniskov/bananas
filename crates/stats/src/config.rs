//! TOML configuration with sane defaults for a 7" Banana Pi LCD.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub sampling: Sampling,
    pub storage: Storage,
    pub devices: Devices,
    pub network: Network,
    pub ui: Ui,
    pub live_socket: LiveSocket,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Sampling {
    pub interval_ms: u64,
}

impl Default for Sampling {
    fn default() -> Self {
        Self { interval_ms: 1000 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Storage {
    pub path: PathBuf,
    pub raw_retention_hours: u32,
    pub agg_retention_days: u32,
    pub flush_interval_ms: u64,
    /// Hard ceiling on the SQLite file size, in MB. `None` means "auto":
    /// computed on first run as min(5 % of free space, 100 MB), floored at
    /// 10 MB, then persisted in the `meta` table so it stays stable across
    /// restarts even if the host's free space changes later.
    pub max_db_mb: Option<u64>,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            path: default_db_path(),
            raw_retention_hours: 24,
            agg_retention_days: 30,
            flush_interval_ms: 5000,
            max_db_mb: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Devices {
    pub include_buses: Vec<String>,
    pub exclude_names: Vec<String>,
}

impl Default for Devices {
    fn default() -> Self {
        Self {
            include_buses: vec!["sata".into(), "usb".into()],
            exclude_names: vec![],
        }
    }
}

/// Network interface filtering. Defaults to ethernet-only — wireless,
/// docker bridges, veth pairs, etc. are noisy on a NAS dashboard and the
/// SATA-fed throughput numbers we care about flow over wired Ethernet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Network {
    /// Names or `name*` prefix patterns to include. Use `["*"]` to allow all.
    pub include_patterns: Vec<String>,
    /// Exact-name blocklist applied after `include_patterns`.
    pub exclude_names: Vec<String>,
}

impl Default for Network {
    fn default() -> Self {
        // `eth*` covers the kernel-classic naming used on Banana Pi/Armbian.
        // `en*` covers the systemd predictable-name scheme (enp0s3, enxabcd…).
        // Wireless (`wlan*`, `wlp*`) and bridges (`docker0`, `br-*`, `veth*`)
        // are intentionally not in the default allowlist.
        Self {
            include_patterns: vec!["eth*".into(), "en*".into()],
            exclude_names: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub theme: String,
    pub spark_window: usize,
    /// Minimum interval between UI repaints in milliseconds. The
    /// sampler still ticks at `sampling.interval_ms` (so historical
    /// data keeps its 1 Hz resolution), but the dashboard only pushes
    /// the latest snapshot to Slint at this cadence. Default 2000 ms
    /// — the BPI's Mali-400 + lima + femtovg stack is CPU-heavy
    /// enough that a 1 Hz repaint kept one core saturated. The eye
    /// can't read changes faster than ~2 s on a small LCD anyway.
    pub refresh_ms: u64,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            // Native resolution of the Banana Pi 7" LCD active area.
            width: 800,
            height: 480,
            title: "bananas-dashboard".into(),
            // "auto" picks dark in PM hours (12:00–23:59 local) and light in
            // AM hours, then flips at noon/midnight; "dark" or "light" pin
            // the theme. Press `t` at runtime to override either way.
            theme: "auto".into(),
            spark_window: 60,
            refresh_ms: 2000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveSocket {
    /// Path of the Unix socket bananas-stats binds for live snapshots.
    /// Subscribers (bananas-webadmin, bananas-dashboard) connect here and
    /// receive newline-delimited JSON. Default lives under
    /// `/run/bananas-stats/` (created by systemd's RuntimeDirectory=);
    /// dev hosts that don't have that dir fall back to /tmp.
    pub path: PathBuf,
}

impl Default for LiveSocket {
    fn default() -> Self {
        Self {
            path: crate::live_socket::default_socket_path(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config at {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&raw)
            .with_context(|| format!("parsing config at {}", path.display()))?;
        cfg.storage.path = expand_tilde(&cfg.storage.path);
        Ok(cfg)
    }
}

fn default_db_path() -> PathBuf {
    // BPI default: under the bananas service user's StateDirectory.
    // Falls back to a HOME-relative path on dev hosts (no /var/lib/bananas).
    let bpi = PathBuf::from("/var/lib/bananas/stats.db");
    if std::fs::metadata("/var/lib/bananas")
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return bpi;
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".local/share/bananas/stats.db")
}

fn expand_tilde(p: &Path) -> PathBuf {
    if let Ok(stripped) = p.strip_prefix("~") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(stripped);
        }
    }
    p.to_path_buf()
}
