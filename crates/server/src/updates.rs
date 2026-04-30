//! opkg-backed update endpoints.
//!
//! Step 6 cleanup: the API shapes match the underlying opkg model
//! directly. No more legacy "Component" enum, no more
//! component-slug-to-package mapping, no more `asset_url` / `sha256`
//! fields that were always None. Frontends (webadmin SPA, bananas-
//! config CLI) consume `{ name, installed, candidate }` rows the
//! same shape opkg itself uses.
//!
//!   GET  /api/version          — installed `bananas-*` packages
//!   GET  /api/updates/check    — upgradable `bananas-*` packages
//!   POST /api/updates/install  — `opkg upgrade <packages>`
//!   GET  /api/updates/status   — SSE stream of the active install
//!
//! Filtering is on `bananas-*` prefix: opkg's view of the system
//! includes ~1000 base-OS packages (libc, busybox, …) that the
//! webadmin doesn't manage. Operators who want raw opkg can SSH in.

use std::{convert::Infallible, sync::Arc, time::SystemTime};

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
};
use bananas_helper::{Command as HelperCommand, Response as HelperResponse};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::AppState;

/// Only packages whose name starts with this prefix are exposed via
/// the API. Keeps the SPA's view focused on what BanaNAS itself
/// ships and avoids surfacing every libc / busybox upgrade.
const PACKAGE_PREFIX: &str = "bananas-";

// ─── /api/version ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct InstalledVersionsResponse {
    pub packages: Vec<InstalledPackage>,
}

pub async fn get_version(State(state): State<AppState>) -> Json<InstalledVersionsResponse> {
    let mut packages = installed_packages(&state).await.unwrap_or_default();
    packages.retain(|p| p.name.starts_with(PACKAGE_PREFIX));
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Json(InstalledVersionsResponse { packages })
}

async fn installed_packages(state: &AppState) -> Option<Vec<InstalledPackage>> {
    let resp = bananas_helper::call(&state.helper_socket, &HelperCommand::OpkgListInstalled)
        .await
        .ok()?;
    if !resp.ok {
        tracing::warn!(error=?resp.error, "opkg list-installed failed");
        return None;
    }
    serde_json::from_str(&resp.output).ok()
}

// ─── /api/updates/check ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradablePackage {
    pub name: String,
    pub installed: String,
    pub candidate: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdatesCheckResponse {
    pub packages: Vec<UpgradablePackage>,
    /// Set when the upstream `opkg update` failed (offline feed,
    /// rate-limited, DNS hiccup). The list-upgradable still runs
    /// against cached metadata, so `packages` may be populated even
    /// when `error` is set.
    pub error: Option<String>,
}

pub async fn get_updates_check(
    State(state): State<AppState>,
) -> Result<Json<UpdatesCheckResponse>, (StatusCode, String)> {
    let mut error: Option<String> = None;
    if let Err(e) = run_helper_simple(&state, HelperCommand::OpkgUpdate).await {
        error = Some(format!("opkg update: {e}"));
    }

    let upgradable: Vec<UpgradablePackage> = match bananas_helper::call(
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

    let mut packages: Vec<UpgradablePackage> = upgradable
        .into_iter()
        .filter(|p| p.name.starts_with(PACKAGE_PREFIX))
        .collect();
    packages.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Json(UpdatesCheckResponse { packages, error }))
}

// ─── POST /api/updates/install + SSE stream ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Install,
    Done,
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
    pub packages: Vec<String>,
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
pub struct UpgradeRequest {
    /// One or more package names. Must each start with the
    /// `bananas-` prefix; anything else is rejected so the API can't
    /// be used to opkg-upgrade arbitrary system packages.
    pub packages: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct InstallAccepted {
    pub id: String,
    pub started_at: u64,
}

pub async fn post_updates_install(
    State(state): State<AppState>,
    Json(req): Json<UpgradeRequest>,
) -> Result<(StatusCode, Json<InstallAccepted>), (StatusCode, String)> {
    if req.packages.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no packages specified".to_string()));
    }
    for p in &req.packages {
        if !p.starts_with(PACKAGE_PREFIX) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("package {p:?} is not in the {PACKAGE_PREFIX}* allowlist"),
            ));
        }
    }

    {
        let guard = state.install.inner.lock().await;
        if let Some(a) = guard.as_ref() {
            if !a.finished {
                return Err((
                    StatusCode::CONFLICT,
                    format!("install already in flight: {}", a.packages.join(", ")),
                ));
            }
        }
    }

    let started_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let id = format!("opkg-{started_at}");
    let active = ActiveInstall {
        id: id.clone(),
        packages: req.packages.clone(),
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
    let packages = req.packages.clone();
    tokio::spawn(async move {
        run_install(task_state, packages).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(InstallAccepted { id, started_at }),
    ))
}

async fn run_install(state: AppState, packages: Vec<String>) {
    state
        .install
        .push(
            Phase::Install,
            format!("running opkg upgrade {}", packages.join(" ")),
        )
        .await;

    let cmd = HelperCommand::OpkgUpgrade {
        packages: packages.clone(),
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
                format!("upgraded {} successfully", packages.join(", ")),
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
