//! opkg-backed update endpoints.
//!
//! Replaces the previous custom GitHub-tarball flow with thin wrappers
//! around the helper's opkg commands. The four endpoints are:
//!
//!   GET  /api/version          — current installed versions
//!   GET  /api/updates/check    — what's upgradable + installed
//!   POST /api/updates/install  — kick `opkg upgrade <packages>`
//!   GET  /api/updates/status   — SSE stream of the active install
//!
//! The JSON shape of /api/updates/check is preserved from the legacy
//! GitHub-fetch implementation so the webadmin SPA keeps working
//! through this step. Step 6 rewrites the SPA to use a more natural
//! opkg-shaped payload, and at that point we can simplify the
//! response too.
//!
//! No more on-server caching: `opkg list-upgradable` reads the local
//! /var/lib/opkg state which is already in-process-fast. Pre-step
//! `opkg update` is rate-limited inside the helper-side run, but
//! every check refreshes by default — the network round-trip to the
//! gh-pages feed is small (Packages.gz is single-digit KB).

use std::{collections::HashMap, convert::Infallible, sync::Arc, time::SystemTime};

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
};
use bananas_helper::{Command as HelperCommand, Component, Response as HelperResponse};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::AppState;

/// Map a Component (which is the wire shape clients send today) to
/// the opkg package name. Server + Helper share a single IPK
/// (bananas-server) since they're built from the same Cargo workspace
/// and their ELF binaries ride together; the legacy frontend still
/// asks for them separately, so we accept that and resolve to the
/// same package.
fn package_name(c: Component) -> &'static str {
    match c {
        Component::Server | Component::Helper => "bananas-server",
        Component::Stats => "bananas-stats",
        Component::Dashboard => "bananas-dashboard",
        Component::Webadmin => "bananas-webadmin",
    }
}

const COMPONENT_SLUGS: &[(&str, Component)] = &[
    ("server", Component::Server),
    ("helper", Component::Helper),
    ("stats", Component::Stats),
    ("dashboard", Component::Dashboard),
    ("webadmin", Component::Webadmin),
];

// ─── /api/version ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
pub struct InstalledVersions {
    /// Map slug → version string. Slugs match the legacy frontend
    /// terminology ("server", "helper", "stats", ...). Helper +
    /// Server map to the same `bananas-server` package and report
    /// the same version.
    #[serde(flatten)]
    pub by_component: HashMap<String, String>,
}

pub async fn get_version(State(state): State<AppState>) -> Json<InstalledVersions> {
    let installed = installed_packages(&state).await.unwrap_or_default();
    let mut by_component = HashMap::new();
    for &(slug, c) in COMPONENT_SLUGS {
        let pkg = package_name(c);
        if let Some(version) = installed.get(pkg) {
            by_component.insert(slug.to_string(), version.clone());
        }
    }
    Json(InstalledVersions { by_component })
}

async fn installed_packages(state: &AppState) -> Option<HashMap<String, String>> {
    let resp = bananas_helper::call(&state.helper_socket, &HelperCommand::OpkgListInstalled)
        .await
        .ok()?;
    if !resp.ok {
        tracing::warn!(error=?resp.error, "opkg list-installed failed");
        return None;
    }
    #[derive(Deserialize)]
    struct Row {
        name: String,
        version: String,
    }
    let rows: Vec<Row> = serde_json::from_str(&resp.output).ok()?;
    Some(rows.into_iter().map(|r| (r.name, r.version)).collect())
}

// ─── /api/updates/check ─────────────────────────────────────────────

/// Per-component view, kept compatible with the legacy frontend so
/// step 5 doesn't break the SPA. `asset_url` and `sha256` are always
/// None now — opkg has its own integrity story; step 6 drops them.
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
    /// Maximum candidate version across all upgradable packages (or
    /// any installed-but-not-upgradable version if nothing is
    /// upgradable). Preserved so the SPA can render a "Latest:
    /// vX.Y.Z" banner.
    pub latest_version: Option<String>,
    /// Always None in the opkg world — there's no single GitHub
    /// release page to deep-link to from a local opkg upgrade.
    pub release_url: Option<String>,
    pub components: HashMap<String, ComponentStatus>,
    pub error: Option<String>,
}

pub async fn get_updates_check(
    State(state): State<AppState>,
) -> Result<Json<UpdatesCheckResponse>, (StatusCode, String)> {
    // Refresh feeds first so a freshly-pushed release shows up.
    // Failure here doesn't kill the call — list-upgradable + list-
    // installed against the cached metadata still produces a useful
    // answer; we just stuff the error into the response so the SPA
    // can surface it.
    let mut error: Option<String> = None;
    if let Err(e) = run_helper_simple(&state, HelperCommand::OpkgUpdate).await {
        error = Some(format!("opkg update: {e}"));
    }

    let installed = installed_packages(&state).await.unwrap_or_default();

    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct UpgRow {
        name: String,
        // `installed` is in the helper's JSON for completeness but
        // we reconstruct from list-installed below — opkg sometimes
        // reports a different installed string here vs there when a
        // hold/lock is in play.
        installed: String,
        candidate: String,
    }
    let upgradable: Vec<UpgRow> = match bananas_helper::call(
        &state.helper_socket,
        &HelperCommand::OpkgListUpgradable,
    )
    .await
    {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => serde_json::from_str(&output).unwrap_or_default(),
        Ok(HelperResponse { error: e, .. }) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "opkg list-upgradable: {}",
                    e.unwrap_or_else(|| "unknown error".into())
                ),
            ));
        }
        Err(e) => {
            return Err((StatusCode::BAD_GATEWAY, format!("helper unreachable: {e}")));
        }
    };
    let upg_by_pkg: HashMap<String, &UpgRow> =
        upgradable.iter().map(|r| (r.name.clone(), r)).collect();

    let mut components = HashMap::new();
    let mut highest_candidate: Option<String> = None;
    for &(slug, c) in COMPONENT_SLUGS {
        let pkg = package_name(c);
        let installed_v = installed.get(pkg).cloned();
        let upg = upg_by_pkg.get(pkg);
        let candidate = upg.map(|r| r.candidate.clone());
        let latest = candidate.clone().or_else(|| installed_v.clone());
        let outdated = upg.is_some();
        if let Some(c) = &candidate {
            highest_candidate = Some(match highest_candidate.take() {
                Some(prev) if version_gt(&prev, c) => prev,
                _ => c.clone(),
            });
        }
        components.insert(
            slug.to_string(),
            ComponentStatus {
                installed: installed_v,
                latest,
                outdated,
                asset_url: None,
                sha256: None,
            },
        );
    }

    let latest_version = highest_candidate.or_else(|| {
        // Nothing upgradable — surface the highest installed version
        // so the SPA still has a "you're at vX.Y.Z" banner.
        installed.values().max_by(|a, b| compare(a, b)).cloned()
    });

    Ok(Json(UpdatesCheckResponse {
        latest_version,
        release_url: None,
        components,
        error,
    }))
}

/// Naive "is `a` > `b`" for X.Y.Z[-suffix] strings. Sufficient for
/// our release tags; falls back to lexical for non-numeric segments.
fn version_gt(a: &str, b: &str) -> bool {
    matches!(compare(a, b), std::cmp::Ordering::Greater)
}

fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| {
                p.split('-')
                    .next()
                    .unwrap_or("")
                    .parse::<u64>()
                    .unwrap_or(0)
            })
            .collect()
    };
    let av = parts(a);
    let bv = parts(b);
    for i in 0..av.len().max(bv.len()) {
        let ai = av.get(i).copied().unwrap_or(0);
        let bi = bv.get(i).copied().unwrap_or(0);
        match ai.cmp(&bi) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    a.cmp(b)
}

// ─── POST /api/updates/install + SSE stream ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Helper is running `opkg upgrade`.
    Install,
    /// Final ok=true line.
    Done,
    /// Final ok=false line.
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
            let line = LogLine { seq, phase, text };
            a.log.push(line.clone());
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
    /// Kept on the wire for legacy compatibility but ignored — opkg
    /// always installs the candidate version from the feed.
    #[serde(default)]
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

    let pkg = package_name(req.component).to_string();
    let started_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let id = format!("{}-{started_at}", req.component.as_str());
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

    let task_state = state.clone();
    let component_name = req.component.as_str().to_string();
    tokio::spawn(async move {
        run_install(task_state, &component_name, &pkg).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(InstallAccepted { id, started_at }),
    ))
}

async fn run_install(state: AppState, component_name: &str, pkg: &str) {
    state
        .install
        .push(Phase::Install, format!("running opkg upgrade {pkg}"))
        .await;

    let cmd = HelperCommand::OpkgUpgrade {
        packages: vec![pkg.to_string()],
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
    if !resp.output.is_empty() {
        state.install.push(Phase::Install, resp.output).await;
    }
    if resp.ok {
        state
            .install
            .push(
                Phase::Done,
                format!("upgraded {component_name} ({pkg}) successfully"),
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

pub async fn get_updates_status(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.install.tx.subscribe();
    let initial = {
        let guard = state.install.inner.lock().await;
        guard.clone()
    };

    let stream = async_stream::stream! {
        if let Some(active) = &initial {
            for line in &active.log {
                if let Ok(event) = Event::default().json_data(line) {
                    yield Ok::<_, Infallible>(event);
                }
            }
            if active.finished {
                let close = Event::default().event("close").data(
                    if active.ok == Some(true) { "ok" } else { "error" }
                );
                yield Ok(close);
                return;
            }
        }
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

// ─── helpers ────────────────────────────────────────────────────────

async fn run_helper_simple(state: &AppState, cmd: HelperCommand) -> anyhow::Result<String> {
    let resp = bananas_helper::call(&state.helper_socket, &cmd).await?;
    if !resp.ok {
        anyhow::bail!(resp.error.unwrap_or_else(|| "helper rejected".into()));
    }
    Ok(resp.output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_basic() {
        assert!(version_gt("1.2.0", "1.1.0"));
        assert!(version_gt("2.0.0", "1.99.99"));
        assert!(!version_gt("1.0.0", "1.0.0"));
        assert!(!version_gt("1.0.0", "1.0.1"));
    }
}
