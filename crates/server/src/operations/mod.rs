//! Unified, kind-agnostic tracker for long-running operations.
//!
//! The webadmin SPA needs every long-running operation (opkg upgrade,
//! config import, cloud sync) to:
//!
//! 1. **Survive a browser refresh** — a fresh page load discovers any
//!    in-flight ops via `GET /api/operations/active` and restores its
//!    progress UI.
//! 2. **Survive a server restart** — the JSON journal at
//!    `BANANAS_OPERATIONS_JOURNAL` (default `/var/lib/bananas/operations.json`)
//!    persists state across `systemctl restart bananas-server`. Ops
//!    whose state lives off-server (opkg in `bananas-helper`'s log,
//!    cloud sync's `.progress` file) can reinstate themselves cleanly;
//!    in-process ops (config import) get marked `Failure` with an
//!    `[interrupted]` note so the operator at least sees what happened.
//!
//! This file owns the manager, types, journal IO, and the
//! `/api/operations/*` HTTP surface. Per-kind handlers live as siblings
//! (`opkg.rs`, `config_import.rs`, `cloud_sync.rs`) and are wired in
//! later sequencing steps. Step 1 ships the skeleton with no handlers
//! attached; existing flows continue running through their legacy
//! endpoints.

use std::{
    collections::HashMap, convert::Infallible, path::PathBuf, sync::Arc, time::Duration,
    time::SystemTime,
};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::AppState;

pub mod config_import;
pub mod opkg;

const HISTORY_CAP: usize = 100;
const SSE_POLL: Duration = Duration::from_millis(500);
const INTERRUPTED_NOTE: &str = "\n[interrupted: server restart]";

/// What kind of operation. Drives per-kind dispatch (run, cancel,
/// reinstate). The variant list grows as each operation is migrated;
/// the journal is forward-compatible because unknown kinds deserialise
/// as `Unknown` and are ignored on load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// `opkg upgrade <packages>` via the helper's transient unit.
    OpkgUpgrade,
    /// POST /api/config — applies an entire ConfigBundle.
    ConfigImport,
    /// One run of a single cloud-sync entry from cloud.toml.
    CloudSyncRun,
    /// Forward-compat sentinel for journal entries written by a newer
    /// server version; ignored on load.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Running,
    Success,
    Failure,
    Cancelled,
}

impl OperationStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, OperationStatus::Running)
    }
}

/// On-disk + in-memory representation of a single operation. The
/// journal serialises `Vec<OperationState>` directly (no wrapper), so
/// renaming a field requires either a `serde(alias)` or a journal
/// schema bump.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationState {
    pub id: u64,
    pub kind: OperationKind,
    pub status: OperationStatus,
    /// Human-readable. Surfaced as the modal title / busy-overlay
    /// caption. Never parsed.
    pub label: String,
    /// Unix seconds.
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// Combined log buffer, capped at `MAX_OUTPUT_BYTES`. For ops with
    /// off-server log files (opkg's `/var/lib/bananas-helper/opkg.log`,
    /// cloud sync's progress files), this carries the *summary tail*
    /// after completion; mid-flight log bytes are streamed via the
    /// per-kind SSE proxy.
    pub output: String,
    /// 0..=100 when known; `None` for indeterminate spinners.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub progress: Option<u32>,
    /// Untyped per-kind context the dispatcher passes to handlers (sync
    /// index, package list, …). Persisted so a reinstated op has the
    /// arguments it was launched with.
    #[serde(default)]
    pub ctx: serde_json::Value,
}

const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct Inner {
    /// All operations, keyed by id. Bounded at HISTORY_CAP via `order`
    /// (oldest finished entries get evicted on insert).
    jobs: HashMap<u64, OperationState>,
    /// FIFO of ids in insertion order. Trims `jobs` to HISTORY_CAP on
    /// every insert.
    order: std::collections::VecDeque<u64>,
    /// Monotonic id source, persisted across restarts so reinstated
    /// ops keep their handle.
    next_id: u64,
}

/// Cheap to clone — internal state is `Arc`'d.
#[derive(Clone)]
pub struct OperationManager {
    inner: Arc<RwLock<Inner>>,
    journal_path: Arc<PathBuf>,
}

impl OperationManager {
    /// Load the journal from disk (or start empty if missing /
    /// unparseable). Running entries are kept as-is so per-kind
    /// reinstaters can decide whether the op is genuinely still alive
    /// (e.g. opkg's transient unit is still running on the helper
    /// side) before any blanket flush. Callers MUST follow up with
    /// `flush_orphan_running` after running per-kind reinstaters,
    /// otherwise zombie Running ops will linger forever.
    pub async fn load(journal_path: PathBuf) -> Self {
        let entries = read_journal(&journal_path).await;
        let mut next_id = 0u64;
        let mut order = std::collections::VecDeque::new();
        let mut jobs = HashMap::new();
        for op in entries {
            if matches!(op.kind, OperationKind::Unknown) {
                continue;
            }
            // next_id tracks the LAST issued id (enqueue does
            // `next_id += 1; let id = next_id;`), so on reload we
            // restore it to max stored id, not max + 1 — otherwise the
            // next enqueue would skip one.
            if op.id > next_id {
                next_id = op.id;
            }
            order.push_back(op.id);
            jobs.insert(op.id, op);
        }
        Self {
            inner: Arc::new(RwLock::new(Inner {
                jobs,
                order,
                next_id,
            })),
            journal_path: Arc::new(journal_path),
        }
    }

    /// After per-kind reinstaters have had their chance, flip every
    /// remaining `Running` op to `Failure` with the
    /// `[interrupted: server restart]` note. Idempotent. Callers with
    /// no per-kind reinstaters can call this immediately after `load`.
    pub async fn flush_orphan_running(&self) {
        let now = unix_now();
        let to_flip: Vec<u64> = {
            let inner = self.inner.read().await;
            inner
                .jobs
                .iter()
                .filter(|(_, op)| op.status == OperationStatus::Running)
                .map(|(id, _)| *id)
                .collect()
        };
        if to_flip.is_empty() {
            return;
        }
        {
            let mut inner = self.inner.write().await;
            for id in &to_flip {
                if let Some(op) = inner.jobs.get_mut(id) {
                    if op.status != OperationStatus::Running {
                        continue;
                    }
                    op.status = OperationStatus::Failure;
                    op.finished_at = Some(now);
                    if !op.output.ends_with(INTERRUPTED_NOTE) {
                        op.output.push_str(INTERRUPTED_NOTE);
                    }
                }
            }
        }
        self.write_journal().await;
    }

    /// Find the most recent Running op of the given kind. Used by
    /// per-kind reinstaters to locate the journal entry they should
    /// reattach to.
    pub async fn most_recent_running_of(&self, kind: OperationKind) -> Option<OperationState> {
        let inner = self.inner.read().await;
        inner
            .order
            .iter()
            .rev()
            .filter_map(|id| inner.jobs.get(id))
            .find(|op| op.kind == kind && op.status == OperationStatus::Running)
            .cloned()
    }

    /// Reserve a fresh id and insert the op as `Running`. Returns the
    /// id. Caller is expected to spawn the work and call `update` /
    /// `finish` as it progresses.
    ///
    /// Callable from handlers but also useful for tests; once handlers
    /// are wired (later steps) most callers will go through
    /// `start::<H>(...)` which composes this with the per-kind run().
    pub async fn enqueue(&self, kind: OperationKind, label: String, ctx: serde_json::Value) -> u64 {
        let id = {
            let mut inner = self.inner.write().await;
            inner.next_id += 1;
            let id = inner.next_id;
            let now = unix_now();
            let op = OperationState {
                id,
                kind,
                status: OperationStatus::Running,
                label,
                started_at: now,
                finished_at: None,
                output: String::new(),
                progress: None,
                ctx,
            };
            inner.order.push_back(id);
            inner.jobs.insert(id, op);
            trim_history(&mut inner);
            id
        };
        self.write_journal().await;
        id
    }

    /// Append a log chunk to an in-flight op. Bounded at
    /// `MAX_OUTPUT_BYTES`; older bytes are dropped from the front when
    /// the chunk overflows. The first call after eviction prefixes a
    /// `[truncated …]` line so consumers see a discontinuity.
    pub async fn append_output(&self, id: u64, chunk: &str) {
        {
            let mut inner = self.inner.write().await;
            if let Some(op) = inner.jobs.get_mut(&id) {
                let combined_len = op.output.len() + chunk.len();
                if combined_len > MAX_OUTPUT_BYTES {
                    let drop_n = combined_len - MAX_OUTPUT_BYTES;
                    if drop_n >= op.output.len() {
                        op.output = format!(
                            "[truncated {drop_n} bytes from earlier output]\n{}",
                            chunk
                                .get(chunk.len().saturating_sub(MAX_OUTPUT_BYTES)..)
                                .unwrap_or(chunk)
                        );
                    } else {
                        op.output.drain(..drop_n);
                        op.output.push_str(chunk);
                    }
                } else {
                    op.output.push_str(chunk);
                }
            }
        }
        self.write_journal().await;
    }

    /// Mutate progress percent. `None` clears it.
    pub async fn set_progress(&self, id: u64, progress: Option<u32>) {
        {
            let mut inner = self.inner.write().await;
            if let Some(op) = inner.jobs.get_mut(&id) {
                op.progress = progress;
            }
        }
        self.write_journal().await;
    }

    /// Transition to a terminal status. Idempotent: a second call with
    /// a different status is ignored (the first finish wins) so a
    /// race between cancel + completion doesn't flap the status.
    pub async fn finish(&self, id: u64, status: OperationStatus, summary: Option<String>) {
        {
            let mut inner = self.inner.write().await;
            if let Some(op) = inner.jobs.get_mut(&id) {
                if op.status.is_terminal() {
                    return;
                }
                op.status = status;
                op.finished_at = Some(unix_now());
                if let Some(s) = summary {
                    if !op.output.is_empty() && !op.output.ends_with('\n') {
                        op.output.push('\n');
                    }
                    op.output.push_str(&s);
                }
            }
        }
        self.write_journal().await;
    }

    pub async fn get(&self, id: u64) -> Option<OperationState> {
        self.inner.read().await.jobs.get(&id).cloned()
    }

    /// Snapshot of every op the manager knows about. Sorted by
    /// `started_at` ascending so the UI can render a stable timeline.
    pub async fn list(&self) -> Vec<OperationState> {
        let inner = self.inner.read().await;
        let mut v: Vec<OperationState> = inner.jobs.values().cloned().collect();
        v.sort_by_key(|op| op.started_at);
        v
    }

    pub async fn list_active(&self) -> Vec<OperationState> {
        self.list()
            .await
            .into_iter()
            .filter(|op| op.status == OperationStatus::Running)
            .collect()
    }

    /// Atomic-rename write of the entire history. Cheap (<100 KB at
    /// HISTORY_CAP) and safe under crash because the rename is
    /// atomic on a single filesystem. Failures are warn-logged but
    /// non-fatal; a missing journal at next startup means a fresh
    /// in-memory state, not a crashed manager.
    async fn write_journal(&self) {
        let snapshot: Vec<OperationState> = {
            let inner = self.inner.read().await;
            // Iterate in insertion order so the on-disk array reads
            // chronologically without a sort step on load.
            inner
                .order
                .iter()
                .filter_map(|id| inner.jobs.get(id).cloned())
                .collect()
        };
        let path = self.journal_path.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut tmp = path.as_ref().clone();
            tmp.set_extension("json.tmp");
            let bytes = serde_json::to_vec_pretty(&snapshot).map_err(std::io::Error::other)?;
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, path.as_ref())?;
            Ok(())
        })
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e)))
        {
            tracing::warn!(error=%e, "operations journal write failed");
        }
    }
}

fn trim_history(inner: &mut Inner) {
    while inner.order.len() > HISTORY_CAP {
        if let Some(victim) = inner.order.pop_front() {
            // Never evict a still-running op even if it pushes us over
            // the cap; better to grow the journal slightly than to lose
            // a live operation's UI.
            if let Some(op) = inner.jobs.get(&victim) {
                if op.status == OperationStatus::Running {
                    inner.order.push_front(victim);
                    break;
                }
            }
            inner.jobs.remove(&victim);
        }
    }
}

async fn read_journal(path: &std::path::Path) -> Vec<OperationState> {
    match tokio::fs::read(path).await {
        Ok(bytes) => match serde_json::from_slice::<Vec<OperationState>>(&bytes) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(path=%path.display(), error=%e, "operations journal parse failed; starting empty");
                Vec::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::warn!(path=%path.display(), error=%e, "operations journal read failed; starting empty");
            Vec::new()
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ─── HTTP surface ───────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub operations: Vec<OperationState>,
}

pub async fn list(State(state): State<AppState>) -> Json<ListResponse> {
    Json(ListResponse {
        operations: state.operations.list().await,
    })
}

pub async fn list_active(State(state): State<AppState>) -> Json<ListResponse> {
    Json(ListResponse {
        operations: state.operations.list_active().await,
    })
}

pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Json<OperationState>, StatusCode> {
    state
        .operations
        .get(id)
        .await
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// SSE log stream. Polls the manager every `SSE_POLL` ms and emits any
/// new bytes since the previous poll. Closes with `event: close` once
/// the op transitions to a terminal state. Returns 404 (well, an
/// immediately-closing stream) if the id is unknown — we use SSE for
/// status transport, so we can't fail with HTTP 404 once the
/// connection is upgraded; instead a single `event: close` carries
/// `not_found` for the client.
pub async fn log_stream(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let manager = state.operations.clone();
    let stream = async_stream::stream! {
        let mut sent: usize = 0;
        loop {
            let op = match manager.get(id).await {
                Some(o) => o,
                None => {
                    yield Ok::<_, Infallible>(Event::default().event("close").data("not_found"));
                    break;
                }
            };
            if op.output.len() > sent {
                let chunk = &op.output[sent..];
                sent = op.output.len();
                let payload = serde_json::json!({
                    "phase": "log",
                    "text": chunk,
                    "progress": op.progress,
                });
                yield Ok(Event::default().json_data(&payload).unwrap_or_default());
            }
            if op.status.is_terminal() {
                let kind = match op.status {
                    OperationStatus::Success => "ok",
                    OperationStatus::Failure => "error",
                    OperationStatus::Cancelled => "cancelled",
                    OperationStatus::Running => "ok",
                };
                yield Ok(Event::default().event("close").data(kind));
                break;
            }
            tokio::time::sleep(SSE_POLL).await;
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Best-effort cancel. Step 1 has no handlers wired, so this just
/// marks the op `Cancelled` in the journal — useful for the UI to
/// stop polling. Per-kind cancellation (SIGTERM the rclone child,
/// `systemctl stop bananas-opkg-upgrade.service`, drop a
/// CancellationToken) lands when each handler is wired.
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<StatusCode, StatusCode> {
    let op = state
        .operations
        .get(id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    if op.status.is_terminal() {
        return Err(StatusCode::CONFLICT);
    }
    state
        .operations
        .finish(
            id,
            OperationStatus::Cancelled,
            Some("[cancelled by operator]".into()),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

// ─── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_journal() -> (tempfile::TempDir, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("operations.json");
        (td, p)
    }

    #[tokio::test]
    async fn enqueue_then_finish_persists_to_journal() {
        let (_td, p) = temp_journal();
        let mgr = OperationManager::load(p.clone()).await;
        let id = mgr
            .enqueue(
                OperationKind::ConfigImport,
                "Importing config".into(),
                serde_json::json!({}),
            )
            .await;
        mgr.append_output(id, "step 1\n").await;
        mgr.finish(id, OperationStatus::Success, Some("done".into()))
            .await;

        let raw = std::fs::read_to_string(&p).unwrap();
        let entries: Vec<OperationState> = serde_json::from_str(&raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
        assert_eq!(entries[0].status, OperationStatus::Success);
        assert!(entries[0].output.contains("step 1"));
        assert!(entries[0].output.ends_with("done"));
        assert!(entries[0].finished_at.is_some());
    }

    #[tokio::test]
    async fn flush_orphan_running_marks_running_as_failure_with_interrupt_note() {
        let (_td, p) = temp_journal();
        // First lifetime: enqueue an op and don't finish it.
        let mgr1 = OperationManager::load(p.clone()).await;
        let id = mgr1
            .enqueue(
                OperationKind::ConfigImport,
                "Importing".into(),
                serde_json::json!({}),
            )
            .await;
        mgr1.append_output(id, "applying exports\n").await;

        // Second lifetime: load alone leaves Running as-is so per-kind
        // reinstaters can decide. flush_orphan_running then flips the
        // ones nobody claimed.
        let mgr2 = OperationManager::load(p).await;
        assert_eq!(mgr2.get(id).await.unwrap().status, OperationStatus::Running);
        mgr2.flush_orphan_running().await;

        let op = mgr2.get(id).await.unwrap();
        assert_eq!(op.status, OperationStatus::Failure);
        assert!(op.finished_at.is_some());
        assert!(op.output.ends_with(INTERRUPTED_NOTE));
        // Original log preserved.
        assert!(op.output.contains("applying exports"));
    }

    #[tokio::test]
    async fn most_recent_running_of_finds_match() {
        let (_td, p) = temp_journal();
        let mgr = OperationManager::load(p).await;
        let _old = mgr
            .enqueue(
                OperationKind::OpkgUpgrade,
                "old".into(),
                serde_json::json!({}),
            )
            .await;
        mgr.finish(_old, OperationStatus::Success, None).await;
        let new = mgr
            .enqueue(
                OperationKind::OpkgUpgrade,
                "new".into(),
                serde_json::json!({}),
            )
            .await;
        let found = mgr
            .most_recent_running_of(OperationKind::OpkgUpgrade)
            .await
            .unwrap();
        assert_eq!(found.id, new);
        // No Running ConfigImport → None.
        assert!(
            mgr.most_recent_running_of(OperationKind::ConfigImport)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn next_id_continues_across_reload() {
        let (_td, p) = temp_journal();
        let mgr1 = OperationManager::load(p.clone()).await;
        let id1 = mgr1
            .enqueue(
                OperationKind::ConfigImport,
                "a".into(),
                serde_json::json!({}),
            )
            .await;
        mgr1.finish(id1, OperationStatus::Success, None).await;

        let mgr2 = OperationManager::load(p).await;
        let id2 = mgr2
            .enqueue(
                OperationKind::ConfigImport,
                "b".into(),
                serde_json::json!({}),
            )
            .await;
        assert_eq!(id2, id1 + 1, "ids must be monotonic across restarts");
    }

    #[tokio::test]
    async fn finish_is_idempotent_first_writer_wins() {
        let (_td, p) = temp_journal();
        let mgr = OperationManager::load(p).await;
        let id = mgr
            .enqueue(
                OperationKind::OpkgUpgrade,
                "upgrade".into(),
                serde_json::json!({}),
            )
            .await;
        mgr.finish(id, OperationStatus::Success, Some("ok".into()))
            .await;
        mgr.finish(id, OperationStatus::Failure, Some("oops".into()))
            .await;
        let op = mgr.get(id).await.unwrap();
        assert_eq!(op.status, OperationStatus::Success);
        assert!(op.output.ends_with("ok"));
    }

    #[tokio::test]
    async fn append_output_truncates_oversized_buffer() {
        let (_td, p) = temp_journal();
        let mgr = OperationManager::load(p).await;
        let id = mgr
            .enqueue(
                OperationKind::ConfigImport,
                "x".into(),
                serde_json::json!({}),
            )
            .await;
        // Push way over the 64 KiB cap.
        let big = "a".repeat(MAX_OUTPUT_BYTES + 5_000);
        mgr.append_output(id, &big).await;
        let op = mgr.get(id).await.unwrap();
        assert!(op.output.len() <= MAX_OUTPUT_BYTES + 100); // allow truncation marker
    }

    #[tokio::test]
    async fn unknown_kind_dropped_on_load() {
        let (_td, p) = temp_journal();
        // Hand-craft a journal with a future kind.
        let json = r#"[{
            "id": 7,
            "kind": "future_thing",
            "status": "running",
            "label": "future",
            "started_at": 100,
            "finished_at": null,
            "output": "",
            "ctx": {}
        }]"#;
        std::fs::write(&p, json).unwrap();
        let mgr = OperationManager::load(p).await;
        assert!(mgr.list().await.is_empty(), "unknown kinds must be ignored");
    }
}
