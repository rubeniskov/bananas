//! What versions are installed for each component.
//!
//! Two sources of truth:
//!   1. `/etc/bananas/versions.toml` — written by the helper after each
//!      successful InstallUpdate. Authoritative for `webadmin` (no bin
//!      to interrogate) and a fast path for the other components.
//!   2. `<binary> --version` — every shipped bin embeds CARGO_PKG_VERSION
//!      at build time. Used as fallback when versions.toml has no row
//!      for that component (fresh image, never installed in-place).
//!
//! Refreshed at most once per `CACHE_TTL` so the Updates page can poll
//! `/api/version` without spawning four processes per request.
//!
//! On a steady state (versions.toml populated), no shell-outs happen.
//! That's the path most operators will hit after the first install.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use bananas_helper::{Command as HelperCommand, Component};
use serde::Serialize;
use tokio::{process::Command as TokioCommand, sync::Mutex};

const CACHE_TTL: Duration = Duration::from_secs(60);

/// Where to find each binary on the deployed BPI. Override per-test
/// via `BANANAS_INSTALL_BIN_DIR`. None for webadmin (read from toml only).
fn binary_path(component: Component) -> Option<PathBuf> {
    let bin_dir = std::env::var_os("BANANAS_INSTALL_BIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/bin"));
    Some(match component {
        Component::Server => bin_dir.join("bananas-server"),
        Component::Helper => bin_dir.join("bananas-helper"),
        Component::Stats => bin_dir.join("bananas-stats"),
        Component::Dashboard => bin_dir.join("bananas-dashboard"),
        Component::Webadmin => return None,
    })
}

/// Stable list of components we report on. Order is the order they
/// appear in /api/version + the Updates page.
pub const ALL: &[Component] = &[
    Component::Server,
    Component::Helper,
    Component::Stats,
    Component::Dashboard,
    Component::Webadmin,
];

#[derive(Debug, Clone, Serialize)]
pub struct InstalledVersions {
    /// Map of component (snake_case) → installed semver string. Entries
    /// missing here mean we couldn't determine the version (fresh image
    /// + bin missing, or webadmin never installed via the helper). The
    /// Updates page renders missing rows as "unknown".
    #[serde(flatten)]
    pub by_component: HashMap<String, String>,
}

#[derive(Clone)]
pub struct VersionCache {
    inner: Arc<Mutex<Option<(Instant, InstalledVersions)>>>,
}

impl VersionCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn current(&self, helper_socket: &Path) -> InstalledVersions {
        let mut guard = self.inner.lock().await;
        if let Some((t, v)) = guard.as_ref() {
            if t.elapsed() < CACHE_TTL {
                return v.clone();
            }
        }
        let fresh = compute(helper_socket).await;
        *guard = Some((Instant::now(), fresh.clone()));
        fresh
    }

    /// Force the next `current()` to refresh. Called after a successful
    /// install so the UI doesn't show stale numbers for ~60 s.
    pub async fn invalidate(&self) {
        *self.inner.lock().await = None;
    }
}

async fn compute(helper_socket: &Path) -> InstalledVersions {
    // Step 1: read versions.toml via the helper. The helper runs as root;
    // versions.toml is in /etc/bananas/ which the bananas user could
    // technically read, but routing through the helper means a single
    // canonical I/O path with the install logic.
    let from_toml = match bananas_helper::call(helper_socket, &HelperCommand::ReadVersions).await {
        Ok(resp) if resp.ok => parse_versions_toml(&resp.output),
        _ => HashMap::new(),
    };

    // Step 2: for each component missing from the toml, fall back to
    // running the binary's --version. webadmin has no bin so it stays
    // missing if not in the toml.
    let mut out = HashMap::new();
    for &c in ALL {
        let key = c.as_str().to_string();
        if let Some(v) = from_toml.get(&key) {
            out.insert(key, v.clone());
            continue;
        }
        if let Some(path) = binary_path(c) {
            if let Some(v) = run_version_flag(&path).await {
                out.insert(key, v);
            }
        }
    }
    InstalledVersions { by_component: out }
}

fn parse_versions_toml(s: &str) -> HashMap<String, String> {
    if s.trim().is_empty() {
        return HashMap::new();
    }
    let parsed: toml::Table = match s.parse() {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };
    let mut out = HashMap::new();
    for (component, entry) in &parsed {
        if let Some(version) = entry
            .as_table()
            .and_then(|t| t.get("version"))
            .and_then(|v| v.as_str())
        {
            out.insert(component.clone(), version.to_string());
        }
    }
    out
}

async fn run_version_flag(bin: &Path) -> Option<String> {
    let out = TokioCommand::new(bin)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    // "<name> <version>" — take the last whitespace-separated token.
    s.split_whitespace().last().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_toml() {
        let s = r#"
[stats]
version = "1.2.3"
sha256 = "abc"

[dashboard]
version = "1.2.0"
"#;
        let map = parse_versions_toml(s);
        assert_eq!(map.get("stats"), Some(&"1.2.3".to_string()));
        assert_eq!(map.get("dashboard"), Some(&"1.2.0".to_string()));
        assert_eq!(map.get("webadmin"), None);
    }

    #[test]
    fn empty_toml_returns_empty_map() {
        assert!(parse_versions_toml("").is_empty());
        assert!(parse_versions_toml("\n  \n").is_empty());
        assert!(parse_versions_toml("not [valid] toml = =").is_empty());
    }
}
