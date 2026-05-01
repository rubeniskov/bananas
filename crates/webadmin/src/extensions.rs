//! `/api/extensions` — discovery surface for the host SPA.
//!
//! Returns the list of installed plugins so the SPA can render
//! their nav tabs and decide when to fire the MFE loader. We
//! re-read the manifest dir per request because installs/uninstalls
//! happen at runtime via opkg; caching would mean the Cloud tab
//! lags a hot install. The dir is small (one file per plugin),
//! and the request itself is rare (once per page load).
//!
//! The response is intentionally minimal: just `id` + `label`.
//! Asset URLs are computed by the host as `/assets/<id>/…` and
//! the entry script is discovered per-plugin via the daemon's
//! own `/api/<id>/__mfe_entry` JSON endpoint — there's nothing
//! plugin-specific to surface here beyond the rendering hint.

use std::path::PathBuf;

use axum::Json;
use bananas_server_common::load_all;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extension {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct ListResponse {
    pub extensions: Vec<Extension>,
}

pub async fn list_extensions() -> Json<ListResponse> {
    let dir: PathBuf = std::env::var_os("BANANAS_EXTENSIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/bananas/extensions.d".into());
    let mut out: Vec<Extension> = load_all(&dir)
        .into_iter()
        // Webadmin's own manifest is for router self-loop wiring;
        // it's not a plugin the SPA renders a nav tab for.
        .filter(|m| m.id != "webadmin")
        .map(|m| Extension {
            id: m.id,
            label: m.label,
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Json(ListResponse { extensions: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lists_only_plugin_manifests() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(
            td.path().join("cloud.toml"),
            r#"id = "cloud"
label = "Cloud sync"
socket = "/run/bananas/cloud.sock"
api_prefix = "/api/cloud""#,
        )
        .unwrap();
        std::fs::write(
            td.path().join("webadmin.toml"),
            r#"id = "webadmin"
socket = "/run/bananas/webadmin.sock"
api_prefix = "/api""#,
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
        // webadmin filtered out, only cloud surfaces
        assert_eq!(resp.0.extensions.len(), 1);
        assert_eq!(resp.0.extensions[0].id, "cloud");
        assert_eq!(resp.0.extensions[0].label.as_deref(), Some("Cloud sync"));
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
