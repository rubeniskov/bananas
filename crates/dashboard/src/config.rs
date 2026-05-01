//! Dashboard-side TOML config — flat schema dedicated to the LCD app.
//!
//! Earlier this re-used `bananas_stats::config::Config`, which forced
//! `[ui]` and `[live_socket]` subsections in `/etc/bananas/dashboard.toml`
//! even though the dashboard owns *only* render-side fields. The flat
//! shape (`width`, `height`, …, `socket`) makes the file (and the
//! bundle's `[dashboard]` section in a Save/Load config round-trip)
//! trivial to read and skip the wrapper noise.
//!
//! Sampler-side fields stay in `/etc/bananas/stats.toml` and continue
//! to use `bananas_stats::config::Config`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DashboardConfig {
    /// Render width / height in pixels. The BPI 5″ panel is 800×480 native.
    pub width: u32,
    pub height: u32,
    /// Window title (only visible under a desktop window manager — the
    /// production path renders fullscreen on KMS+EGL).
    pub title: String,
    /// "auto" picks dark in PM hours (12:00–23:59 local) and light in
    /// AM hours, then flips at noon/midnight; "dark" or "light" pin
    /// the theme. Press `t` at runtime to override either way.
    pub theme: String,
    /// Number of historical points kept on the live sparklines.
    pub spark_window: usize,
    /// Minimum interval between UI repaints in milliseconds. The
    /// sampler still ticks at `sampling.interval_ms` (so historical
    /// data keeps its 1 Hz resolution), but the dashboard only pushes
    /// the latest snapshot to Slint at this cadence. 2000 ms keeps
    /// Mali-400 + lima + femtovg at ~3 % CPU; the eye can't read
    /// sub-2-second changes on a 5″ LCD anyway.
    pub refresh_ms: u64,
    /// Path of the Unix socket bananas-stats binds for live snapshots.
    /// We connect here and receive newline-delimited JSON. Must match
    /// the path in /etc/bananas/stats.toml's [live_socket].
    pub socket: PathBuf,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            width: 800,
            height: 480,
            title: "bananas-dashboard".into(),
            theme: "auto".into(),
            spark_window: 60,
            refresh_ms: 2000,
            socket: PathBuf::from("/run/bananas/stats.sock"),
        }
    }
}

impl DashboardConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config at {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing config at {}", path.display()))
    }
}
