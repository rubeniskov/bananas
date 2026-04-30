//! GitHub-driven update check.
//!
//! Polls `https://api.github.com/repos/<owner>/<repo>/releases/latest`,
//! parses the asset list + the sibling `SHA256SUMS` asset, and combines
//! it with the locally-installed versions from `version::VersionCache`
//! to produce a per-component "is there an update available" answer.
//!
//! Auth: anonymous. Public repo, well within the 60 req/hr unauthenticated
//! rate limit given the 5-minute cache below. If we ever hit the wall,
//! `BANANAS_GH_TOKEN` env var lifts the limit to 5000 req/hr.
//!
//! Caching:
//!   - 5 min positive TTL on the release JSON + SHA256SUMS pair.
//!   - 60 s negative TTL on fetch failures so transient blips (DNS,
//!     captive-portal redirect) don't hammer the API.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use axum::{Json, extract::State, http::StatusCode};
use bananas_helper::Component;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::AppState;

const RELEASES_API: &str = "https://api.github.com/repos/rubeniskov/bananas/releases/latest";
const RELEASE_BASE: &str = "https://github.com/rubeniskov/bananas/releases";
const POSITIVE_TTL: Duration = Duration::from_secs(5 * 60);
const NEGATIVE_TTL: Duration = Duration::from_secs(60);
const USER_AGENT: &str = concat!("bananas-server/", env!("CARGO_PKG_VERSION"));

/// Asset name shipped per component. Server + Helper share the same
/// tarball (both binaries are bundled in `bananas-server-armv7.tar.gz`).
fn asset_name(c: Component) -> &'static str {
    match c {
        Component::Server | Component::Helper => "bananas-server-armv7.tar.gz",
        Component::Stats => "bananas-stats-armv7.tar.gz",
        Component::Dashboard => "bananas-dashboard-armv7.tar.gz",
        Component::Webadmin => "bananas-webadmin.tar.gz",
    }
}

/// What we cache per fetch. Empty `assets`/`shas` + a non-None `error`
/// is the negative-cache state.
#[derive(Debug, Clone)]
pub struct ReleaseSnapshot {
    /// `tag_name` minus a leading "v" (e.g. "1.0.1"). Empty on error.
    pub version: String,
    pub release_url: String,
    pub assets: HashMap<String, String>,
    pub shas: HashMap<String, String>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct UpdatesCache {
    inner: Arc<Mutex<Option<(Instant, ReleaseSnapshot)>>>,
    client: reqwest::Client,
}

impl UpdatesCache {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client");
        Self {
            inner: Arc::new(Mutex::new(None)),
            client,
        }
    }

    pub async fn snapshot(&self) -> ReleaseSnapshot {
        let mut guard = self.inner.lock().await;
        if let Some((t, snap)) = guard.as_ref() {
            let ttl = if snap.error.is_some() {
                NEGATIVE_TTL
            } else {
                POSITIVE_TTL
            };
            if t.elapsed() < ttl {
                return snap.clone();
            }
        }
        let fresh = match fetch(&self.client).await {
            Ok(s) => s,
            Err(e) => ReleaseSnapshot {
                version: String::new(),
                release_url: String::new(),
                assets: HashMap::new(),
                shas: HashMap::new(),
                error: Some(format!("{e:#}")),
            },
        };
        *guard = Some((Instant::now(), fresh.clone()));
        fresh
    }

    pub async fn invalidate(&self) {
        *self.inner.lock().await = None;
    }
}

async fn fetch(client: &reqwest::Client) -> Result<ReleaseSnapshot> {
    #[derive(serde::Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
    }
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
        html_url: String,
        assets: Vec<Asset>,
    }
    let mut req = client.get(RELEASES_API);
    if let Ok(token) = std::env::var("BANANAS_GH_TOKEN") {
        if !token.is_empty() {
            req = req.bearer_auth(token);
        }
    }
    let resp = req
        .send()
        .await
        .context("GET releases/latest")?
        .error_for_status()
        .context("releases/latest non-2xx")?;
    let release: Release = resp.json().await.context("parsing release JSON")?;

    let mut assets = HashMap::new();
    let mut sha_url = None;
    for a in release.assets {
        if a.name == "SHA256SUMS" {
            sha_url = Some(a.browser_download_url);
        } else {
            assets.insert(a.name, a.browser_download_url);
        }
    }
    let shas = match sha_url {
        Some(url) => fetch_shasums(client, &url).await.unwrap_or_default(),
        None => HashMap::new(),
    };

    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name)
        .to_string();

    Ok(ReleaseSnapshot {
        version,
        release_url: release.html_url,
        assets,
        shas,
        error: None,
    })
}

async fn fetch_shasums(client: &reqwest::Client, url: &str) -> Result<HashMap<String, String>> {
    let body = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(parse_shasums(&body))
}

fn parse_shasums(body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in body.lines() {
        // sha256sum format: "<64 hex>  <filename>" (two spaces).
        let mut parts = line.splitn(2, ' ');
        let sha = parts.next().unwrap_or("").trim();
        let rest = parts.next().unwrap_or("").trim();
        // Some shasum tools emit "<sha> *<file>" (binary mode); strip leading * if present.
        let name = rest.trim_start_matches('*').trim().to_string();
        if sha.len() == 64 && !name.is_empty() {
            out.insert(name, sha.to_string());
        }
    }
    out
}

/// Per-component view returned by GET /api/updates/check. `latest`
/// + `asset_url` + `sha256` are absent when the GitHub fetch failed
/// (look at `error` on the wrapper). `installed` is absent for fresh
/// images that haven't run any binary yet (very rare — happens before
/// systemd starts the units).
#[derive(Debug, Clone, Serialize)]
pub struct ComponentStatus {
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub outdated: bool,
    pub asset_url: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdatesCheckResponse {
    pub latest_version: Option<String>,
    pub release_url: Option<String>,
    pub components: HashMap<String, ComponentStatus>,
    /// Set to the GitHub fetch error when `latest_version` is None.
    pub error: Option<String>,
}

pub async fn get_updates_check(
    State(state): State<AppState>,
) -> Result<Json<UpdatesCheckResponse>, (StatusCode, String)> {
    build_check(&state).await.map(Json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("updates check: {e:#}"),
        )
    })
}

pub async fn get_version(
    State(state): State<AppState>,
) -> Json<crate::version::InstalledVersions> {
    Json(state.versions.current(&state.helper_socket).await)
}

async fn build_check(state: &AppState) -> Result<UpdatesCheckResponse> {
    let installed = state.versions.current(&state.helper_socket).await;
    let release = state.updates.snapshot().await;

    let mut components = HashMap::new();
    for &c in crate::version::ALL {
        let key = c.as_str().to_string();
        let installed_v = installed.by_component.get(&key).cloned();
        let latest_v = (!release.version.is_empty()).then(|| release.version.clone());
        let asset_url = release
            .assets
            .get(asset_name(c))
            .cloned()
            .or_else(|| latest_v.as_ref().map(|v| fallback_asset_url(c, v)));
        let sha256 = release.shas.get(asset_name(c)).cloned();
        let outdated = match (installed_v.as_deref(), latest_v.as_deref()) {
            (Some(i), Some(l)) => semver_lt(i, l),
            _ => false,
        };
        components.insert(
            key,
            ComponentStatus {
                installed: installed_v,
                latest: latest_v,
                outdated,
                asset_url,
                sha256,
            },
        );
    }

    Ok(UpdatesCheckResponse {
        latest_version: (!release.version.is_empty()).then(|| release.version.clone()),
        release_url: (!release.release_url.is_empty()).then(|| release.release_url.clone()),
        components,
        error: release.error.clone(),
    })
}

/// When the GitHub release lists assets but doesn't include a download
/// URL for one of ours (shouldn't happen in practice but pads the API
/// surface), reconstruct the canonical URL from `tag_name`.
fn fallback_asset_url(c: Component, version: &str) -> String {
    format!("{RELEASE_BASE}/download/v{version}/{}", asset_name(c))
}

/// Naive semver-less-than comparison sufficient for our X.Y.Z tags.
/// Returns true iff `a < b` componentwise. Non-numeric segments fall
/// back to lexical compare so a "1.0.0-rc1" never beats "1.0.0".
fn semver_lt(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> (Vec<u64>, String) {
        let (numeric, rest) = match s.find(|c: char| c != '.' && !c.is_ascii_digit()) {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        };
        let parts: Vec<u64> = numeric
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect();
        (parts, rest.to_string())
    };
    let (av, ar) = parse(a);
    let (bv, br) = parse(b);
    let len = av.len().max(bv.len());
    for i in 0..len {
        let ai = av.get(i).copied().unwrap_or(0);
        let bi = bv.get(i).copied().unwrap_or(0);
        if ai != bi {
            return ai < bi;
        }
    }
    // Pre-release suffix (a < b lexically). "1.0.0-rc1" < "1.0.0".
    match (ar.is_empty(), br.is_empty()) {
        (false, true) => true,
        (true, false) => false,
        _ => ar < br,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shasums_format() {
        let body = "abc123  bananas-stats-armv7.tar.gz\n\
                    deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  bananas-webadmin.tar.gz\n";
        let map = parse_shasums(body);
        // First line has only 6 hex chars — too short, skipped.
        assert!(!map.contains_key("bananas-stats-armv7.tar.gz"));
        assert_eq!(
            map.get("bananas-webadmin.tar.gz"),
            Some(&"deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string())
        );
    }

    #[test]
    fn semver_compares_correctly() {
        assert!(semver_lt("1.0.0", "1.0.1"));
        assert!(semver_lt("1.0.0", "2.0.0"));
        assert!(semver_lt("1.0.0", "1.1.0"));
        assert!(!semver_lt("1.0.1", "1.0.0"));
        assert!(!semver_lt("1.0.0", "1.0.0"));
        // Pre-release lower than release of same triple.
        assert!(semver_lt("1.0.0-rc1", "1.0.0"));
        assert!(!semver_lt("1.0.0", "1.0.0-rc1"));
    }
}
