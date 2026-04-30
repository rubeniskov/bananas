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
    convert::Infallible,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
};
use bananas_helper::{Command as HelperCommand, Component};
use futures_util::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, sync::Mutex};

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
    pub(crate) client: reqwest::Client,
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

pub async fn get_version(State(state): State<AppState>) -> Json<crate::version::InstalledVersions> {
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

// ─── Install + SSE ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Downloading the tarball from GitHub to staging.
    Download,
    /// SHA-256 (server-side double-check) before handing off to helper.
    /// The helper re-verifies anyway — this catches gross corruption
    /// faster and surfaces it in the SSE stream.
    Verify,
    /// Helper is doing the swap + restart.
    Install,
    /// Final ok=true line — install succeeded.
    Done,
    /// Final ok=false line — install failed.
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub seq: u64,
    pub phase: Phase,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveInstall {
    pub id: String,
    pub component: String,
    pub version: String,
    pub started_at: u64,
    pub finished: bool,
    pub ok: Option<bool>,
    pub log: Vec<LogLine>,
}

#[derive(Clone)]
pub struct InstallState {
    inner: Arc<Mutex<Option<ActiveInstall>>>,
    pub(crate) tx: tokio::sync::broadcast::Sender<LogLine>,
}

impl InstallState {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(256);
        Self {
            inner: Arc::new(Mutex::new(None)),
            tx,
        }
    }

    async fn push(&self, phase: Phase, text: impl Into<String>) {
        let text = text.into();
        let mut guard = self.inner.lock().await;
        if let Some(a) = guard.as_mut() {
            let seq = a.log.len() as u64;
            let line = LogLine {
                seq,
                phase,
                text: text.clone(),
            };
            a.log.push(line.clone());
            // broadcast errors silently if no subscribers — that's fine.
            let _ = self.tx.send(line);
            if matches!(phase, Phase::Done | Phase::Error) {
                a.finished = true;
                a.ok = Some(matches!(phase, Phase::Done));
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    pub component: Component,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct InstallAccepted {
    pub id: String,
    pub started_at: u64,
}

pub async fn post_updates_install(
    State(state): State<AppState>,
    Json(req): Json<InstallRequest>,
) -> Result<(StatusCode, Json<InstallAccepted>), (StatusCode, String)> {
    // Fail fast if another install is already in flight. Sequential
    // installs only — there's no benefit to two at once and the helper
    // would queue them anyway.
    {
        let guard = state.install.inner.lock().await;
        if let Some(a) = guard.as_ref() {
            if !a.finished {
                return Err((
                    StatusCode::CONFLICT,
                    format!("install already in flight: {} {}", a.component, a.version),
                ));
            }
        }
    }

    // Look up asset url + sha256 from the updates cache (or refresh
    // if stale). Reject if either is missing for this component.
    let snap = state.updates.snapshot().await;
    if let Some(err) = snap.error.as_ref() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("github fetch failed: {err}"),
        ));
    }
    if snap.version != req.version {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "requested {} but latest release is {}",
                req.version, snap.version
            ),
        ));
    }
    let asset = asset_name(req.component);
    let asset_url = match snap.assets.get(asset).cloned() {
        Some(u) => u,
        None => {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("asset {asset} missing from release v{}", snap.version),
            ));
        }
    };
    let sha256 = match snap.shas.get(asset).cloned() {
        Some(s) => s,
        None => {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!(
                    "SHA256SUMS missing entry for {asset} in release v{}",
                    snap.version
                ),
            ));
        }
    };

    // Bookkeeping for the SSE stream.
    let started_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let id = format!("{}-{}", req.component.as_str(), started_at);
    let active = ActiveInstall {
        id: id.clone(),
        component: req.component.as_str().to_string(),
        version: req.version.clone(),
        started_at,
        finished: false,
        ok: None,
        log: Vec::new(),
    };
    {
        let mut guard = state.install.inner.lock().await;
        *guard = Some(active);
    }

    // Off we go.
    let task_state = state.clone();
    tokio::spawn(async move {
        run_install(task_state, req.component, &req.version, &asset_url, &sha256).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(InstallAccepted { id, started_at }),
    ))
}

async fn run_install(
    state: AppState,
    component: Component,
    version: &str,
    asset_url: &str,
    expected_sha256: &str,
) {
    let staging_dir: PathBuf = std::env::var_os("BANANAS_UPDATES_STAGING_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/bananas/updates/staging"));
    if let Err(e) = tokio::fs::create_dir_all(&staging_dir).await {
        state
            .install
            .push(
                Phase::Error,
                format!("creating staging dir {}: {e}", staging_dir.display()),
            )
            .await;
        return;
    }
    let asset_filename = asset_url.rsplit('/').next().unwrap_or("download.tar.gz");
    let tarball_path = staging_dir.join(asset_filename);

    state
        .install
        .push(
            Phase::Download,
            format!("downloading {asset_url} → {}", tarball_path.display()),
        )
        .await;

    if let Err(e) = download(&state.updates.client, asset_url, &tarball_path).await {
        state
            .install
            .push(Phase::Error, format!("download failed: {e:#}"))
            .await;
        return;
    }

    state
        .install
        .push(
            Phase::Verify,
            format!("verifying sha256 ({expected_sha256})"),
        )
        .await;

    if let Err(e) = verify_local_sha(&tarball_path, expected_sha256).await {
        state
            .install
            .push(Phase::Error, format!("sha256 mismatch: {e:#}"))
            .await;
        return;
    }

    state
        .install
        .push(
            Phase::Install,
            format!(
                "handing off to helper: install {} {} from {}",
                component.as_str(),
                version,
                tarball_path.display()
            ),
        )
        .await;

    let cmd = HelperCommand::InstallUpdate {
        component,
        tarball_path: tarball_path.to_string_lossy().to_string(),
        expected_version: version.to_string(),
        expected_sha256: expected_sha256.to_string(),
    };
    let resp = match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(r) => r,
        Err(e) => {
            state
                .install
                .push(Phase::Error, format!("helper unreachable: {e:#}"))
                .await;
            return;
        }
    };
    // Whatever the helper said, surface it verbatim.
    if !resp.output.is_empty() {
        state.install.push(Phase::Install, resp.output).await;
    }
    if resp.ok {
        state.versions.invalidate().await;
        state
            .install
            .push(
                Phase::Done,
                format!("installed {} {} successfully", component.as_str(), version),
            )
            .await;
    } else {
        state
            .install
            .push(
                Phase::Error,
                resp.error
                    .unwrap_or_else(|| "helper returned ok=false with no error".into()),
            )
            .await;
    }
}

async fn download(client: &reqwest::Client, url: &str, dest: &std::path::Path) -> Result<()> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("non-2xx for {url}"))?;
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading download chunk")?;
        file.write_all(&chunk).await.context("writing chunk")?;
    }
    file.flush().await.context("flushing tarball")?;
    Ok(())
}

async fn verify_local_sha(path: &std::path::Path, expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .await
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(anyhow::anyhow!(
            "expected {expected}, got {actual} for {}",
            path.display()
        ));
    }
    Ok(())
}

pub async fn get_updates_status(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe BEFORE reading the snapshot so we don't drop lines that
    // arrive in the gap between the two reads.
    let mut rx = state.install.tx.subscribe();
    let initial = {
        let guard = state.install.inner.lock().await;
        guard.clone()
    };

    let stream = async_stream::stream! {
        // Replay any history from the current install (if any) so a
        // freshly-connected client sees the full log.
        if let Some(active) = &initial {
            for line in &active.log {
                if let Ok(event) = Event::default().json_data(line) {
                    yield Ok::<_, Infallible>(event);
                }
            }
            // If we're already finished by the time the client connects,
            // emit the close event from the initial snapshot and bail.
            if active.finished {
                let close = Event::default().event("close").data(
                    if active.ok == Some(true) { "ok" } else { "error" }
                );
                yield Ok(close);
                return;
            }
        }
        // Forward new lines as they arrive. Stop when we see Done/Error.
        while let Ok(line) = rx.recv().await {
            let final_event = matches!(line.phase, Phase::Done | Phase::Error);
            if let Ok(event) = Event::default().json_data(&line) {
                yield Ok(event);
            }
            if final_event {
                let close = Event::default().event("close").data(
                    if matches!(line.phase, Phase::Done) { "ok" } else { "error" }
                );
                yield Ok(close);
                break;
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
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
