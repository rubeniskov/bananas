//! Plugin manifest schema, shared by `bananas-router` and
//! `bananas-webadmin`. Each plugin (and `bananas-webadmin` itself,
//! for the API-routing self-loop) drops one TOML file under
//! `/etc/bananas/extensions.d/`. Both daemons read the directory at
//! startup; `systemctl reload` re-reads it after an opkg install.
//!
//! The manifest declares two things — the API prefix the plugin
//! claims, and the Unix socket its daemon listens on. The asset
//! prefix is implied by `id`: webadmin sub-proxies
//! `/assets/<id>/*` to the same socket. There's no separate
//! "spa_path" any more; the host SPA computes the asset URL from
//! `id` directly and discovers the entry script via the daemon's
//! own `/api/<id>/__mfe_entry` JSON endpoint.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Stable identifier. Drives the asset prefix (`/assets/<id>/`)
    /// and the `data-bananas-mfe="<id>"` script tag the host injects.
    pub id: String,
    /// Human-readable label for the SPA's nav tab. Optional — a
    /// plugin without UI can omit it.
    #[serde(default)]
    pub label: Option<String>,
    /// Absolute path to the daemon's Unix socket.
    pub socket: PathBuf,
    /// API path prefix this plugin claims, e.g. `/api/cloud`. The
    /// router routes by longest-prefix match against this field.
    /// `bananas-webadmin`'s own manifest declares `/api` as a
    /// catch-all so anything not claimed by another plugin lands
    /// on its Unix socket.
    pub api_prefix: String,
    /// Sort key for the host SPA's nav bar — plugins render
    /// left-to-right by ascending `order`. Defaults to 0 if unset
    /// (legacy manifests still parse and sort first). Conventional
    /// values: 10/20/30/… so new plugins can slot between without
    /// renumbering siblings.
    #[serde(default)]
    pub order: u32,
    /// Lucide icon name the host SPA renders next to `label` in the
    /// nav. Optional; the host falls back to a generic "box" glyph
    /// if absent. Same icon set webadmin's NavTab already uses (see
    /// `crates/webadmin/src/ui/icons.rs`).
    #[serde(default)]
    pub icon: Option<String>,
}

/// Parse every `*.toml` file under `dir`. Bad files are logged and
/// skipped — a single broken manifest shouldn't take the gateway
/// offline. Duplicate ids are de-duplicated with a warning; the
/// last-loaded definition wins (filesystem iteration order is
/// arbitrary, so order-dependent semantics would be a bug).
pub fn load_all(dir: &Path) -> Vec<Manifest> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(dir = %dir.display(), "manifest dir does not exist; no extensions loaded");
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "manifest dir read failed");
            return Vec::new();
        }
    };
    let mut by_id: HashMap<String, Manifest> = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "manifest read failed");
                continue;
            }
        };
        match toml::from_str::<Manifest>(&body) {
            Ok(m) => {
                if let Some(existing) = by_id.insert(m.id.clone(), m.clone()) {
                    tracing::warn!(
                        id = %m.id,
                        prev = %existing.socket.display(),
                        new = %m.socket.display(),
                        "duplicate manifest id; later definition wins"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "manifest parse failed; skipping");
            }
        }
    }
    by_id.into_values().collect()
}

/// Longest-prefix match against `path`. Used by `bananas-router`
/// to pick the right plugin daemon for an incoming `/api/*`
/// request. Exact-match or `path.starts_with("<prefix>/")` only —
/// `/api/cloudfoo` does NOT match `api_prefix = "/api/cloud"`.
pub fn match_prefix<'a>(manifests: &'a [Manifest], path: &str) -> Option<&'a Manifest> {
    let mut best: Option<&Manifest> = None;
    let mut best_len: usize = 0;
    for m in manifests {
        let trimmed = m.api_prefix.trim_end_matches('/');
        let matches = if trimmed.is_empty() {
            true
        } else {
            path == trimmed || path.starts_with(&format!("{trimmed}/"))
        };
        if matches && m.api_prefix.len() >= best_len {
            best = Some(m);
            best_len = m.api_prefix.len();
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, socket: &str, api_prefix: &str) -> Manifest {
        Manifest {
            id: id.into(),
            label: None,
            socket: PathBuf::from(socket),
            api_prefix: api_prefix.into(),
            order: 0,
            icon: None,
        }
    }

    #[test]
    fn longest_prefix_wins() {
        let exts = vec![
            manifest("webadmin", "/run/webadmin.sock", "/api"),
            manifest("cloud", "/run/cloud.sock", "/api/cloud"),
        ];
        assert_eq!(match_prefix(&exts, "/api/cloud/runs").unwrap().id, "cloud");
        assert_eq!(match_prefix(&exts, "/api/exports").unwrap().id, "webadmin");
        assert_eq!(match_prefix(&exts, "/api/me").unwrap().id, "webadmin");
        assert_eq!(match_prefix(&exts, "/api").unwrap().id, "webadmin");
    }

    #[test]
    fn exact_prefix_match_does_not_overshoot() {
        let exts = vec![
            manifest("webadmin", "/run/webadmin.sock", "/api"),
            manifest("cloud", "/run/cloud.sock", "/api/cloud"),
        ];
        assert_eq!(match_prefix(&exts, "/api/cloudfoo").unwrap().id, "webadmin");
    }

    #[test]
    fn no_match_when_nothing_claims_root_api() {
        let exts = vec![manifest("cloud", "/run/cloud.sock", "/api/cloud")];
        assert!(match_prefix(&exts, "/api/exports").is_none());
        assert!(match_prefix(&exts, "/api/me").is_none());
    }

    #[test]
    fn load_all_skips_unparseable_and_dedups() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(
            td.path().join("ok.toml"),
            r#"id = "ok"
socket = "/run/ok.sock"
api_prefix = "/api/ok""#,
        )
        .unwrap();
        std::fs::write(td.path().join("bad.toml"), "not = valid = toml = at all").unwrap();
        std::fs::write(td.path().join("README"), "ignored, not toml").unwrap();
        let v = load_all(td.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "ok");
    }

    #[test]
    fn load_all_handles_missing_dir() {
        let v = load_all(Path::new("/nonexistent/path/here"));
        assert!(v.is_empty());
    }
}
