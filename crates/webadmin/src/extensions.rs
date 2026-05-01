//! `/api/extensions` — discovery surface for the SPA.
//!
//! Mirrors what `bananas-router` reads at startup, so the SPA can
//! decide which optional plugin tabs to render. We re-read the dir
//! per request because installs/uninstalls happen at runtime via opkg
//! and the SPA needs to see the current truth (caching means the
//! Cloud tab would lag a hot install).
//!
//! The router's per-startup-load is fine because it doesn't refresh
//! often; the SPA's poll is rare (once per page load, sometimes a
//! periodic check). Either way the directory is small.

use std::path::PathBuf;

use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extension {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    /// Path the SPA's nav tab should jump to when clicked. Optional —
    /// not every extension has UI.
    #[serde(default)]
    pub spa_path: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct ListResponse {
    pub extensions: Vec<Extension>,
}

#[derive(Debug, Deserialize)]
struct ManifestOnDisk {
    id: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    spa_path: Option<String>,
}

pub async fn list_extensions() -> Json<ListResponse> {
    let dir: PathBuf = std::env::var_os("BANANAS_EXTENSIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/bananas/extensions.d".into());
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Json(ListResponse { extensions: out }),
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let body = match std::fs::read_to_string(&p) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if let Ok(m) = toml::from_str::<ManifestOnDisk>(&body) {
            out.push(Extension {
                id: m.id,
                label: m.label,
                spa_path: m.spa_path,
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Json(ListResponse { extensions: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lists_only_toml_files() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(
            td.path().join("cloud.toml"),
            r#"id = "cloud"
label = "Cloud sync"
spa_path = "/cloud"
socket = "/run/bananas/cloud.sock"
prefixes = ["/api/cloud"]"#,
        )
        .unwrap();
        std::fs::write(td.path().join("README"), "ignore me").unwrap();
        // SAFETY: tests are serialised on env access via cargo test's
        // single-threaded mode for env-dependent suites; this one is
        // small enough that the race is acceptable in practice.
        unsafe {
            std::env::set_var("BANANAS_EXTENSIONS_DIR", td.path());
        }
        let resp = list_extensions().await;
        assert_eq!(resp.0.extensions.len(), 1);
        assert_eq!(resp.0.extensions[0].id, "cloud");
        assert_eq!(resp.0.extensions[0].spa_path.as_deref(), Some("/cloud"));
    }

    #[tokio::test]
    async fn returns_empty_when_dir_missing() {
        unsafe {
            std::env::set_var("BANANAS_EXTENSIONS_DIR", "/nonexistent/path");
        }
        let resp = list_extensions().await;
        assert!(resp.0.extensions.is_empty());
    }
}
