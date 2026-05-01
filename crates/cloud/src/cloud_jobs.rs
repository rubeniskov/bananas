//! Async job manager for cloud syncs.
//!
//! `/api/cloud/syncs/<idx>/run` used to block the HTTP request for the
//! entire duration of an `rclone copy` (potentially hours on terabytes).
//! That's fine for typical home-folder backups but ugly for big trees,
//! and it means the browser tab has to stay open with the connection
//! held.
//!
//! This module owns a small in-process job table:
//!
//!   - `enqueue(idx)` returns a job_id immediately and spawns a tokio
//!     task that calls the helper's blocking `RunCloudSync` command.
//!   - The task updates the job's status / output / finished_unix
//!     when rclone exits.
//!   - The UI polls `/api/cloud/runs/<job_id>` to follow the run.
//!
//! It also handles **scheduled runs**: a separate tokio task wakes
//! once a minute, reads `/etc/bananas/cloud.toml`, and enqueues a job
//! for every sync entry whose 5-field cron string matches the current
//! wall-clock minute (and whose previous run hasn't already happened
//! within this minute).
//!
//! Storage: jobs are kept in memory only. `last_run` per sync index
//! lives next to them so the scheduler can debounce. A daemon restart
//! loses recent-job history but the cron schedule re-evaluates from
//! "now" — operators don't get a flood of "missed" runs on boot.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use bananas_proto::engine::v1::{RunCloudSyncRequest, engine_service_client::EngineServiceClient};
use bananas_proto::engine_client;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

use crate::cloud::CloudConfig;

const JOB_HISTORY_CAP: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Success,
    Failure,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobState {
    pub id: u64,
    pub sync_idx: usize,
    pub status: JobStatus,
    pub started_unix: i64,
    pub finished_unix: Option<i64>,
    /// Combined stdout+stderr from rclone. Truncated to the last
    /// `MAX_OUTPUT_BYTES` chars to bound memory; the UI only needs a
    /// tail summary.
    pub output: String,
    /// When the job has source/destination context attached (set by
    /// the dispatcher), this echoes the human-readable label so the
    /// UI can render "personal: /srv/photos → personal:photos" without
    /// re-fetching cloud.toml.
    pub label: String,
    /// Live transfer percentage (0..=100) tee'd by the helper while
    /// rclone is running. `None` when the job is not running, or while
    /// rclone is still in its initial directory scan and hasn't
    /// produced a progress line yet. Read off
    /// `/run/bananas/sync-progress/<sync_idx>.progress` on each
    /// JobManager `get`/`list` so the value is fresh per request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u32>,
}

const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct Inner {
    jobs: HashMap<u64, JobState>,
    /// Insertion order for trimming. Bounded at `JOB_HISTORY_CAP`.
    order: VecDeque<u64>,
    next_id: u64,
    /// last successful or failed start ts per sync_idx, for the
    /// scheduler to know "did we already fire this minute".
    last_run: HashMap<usize, i64>,
}

#[derive(Clone)]
pub struct JobManager {
    inner: Arc<RwLock<Inner>>,
    /// Per-sync running guard so we don't enqueue the same sync twice
    /// in parallel — rclone running concurrently against the same
    /// remote+path is a recipe for corruption.
    running: Arc<Mutex<HashMap<usize, u64>>>,
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner::default())),
            running: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Returns the running job_id for `idx` if one is in flight.
    pub async fn running_for(&self, idx: usize) -> Option<u64> {
        self.running.lock().await.get(&idx).copied()
    }

    /// Enqueue a run for the given sync index. Returns immediately
    /// with the job_id; the rclone call happens on a tokio task. If
    /// the same sync is already running, returns the existing job_id
    /// instead of starting a duplicate.
    pub async fn enqueue(&self, sync_idx: usize, label: String, helper_socket: PathBuf) -> u64 {
        {
            let running = self.running.lock().await;
            if let Some(existing) = running.get(&sync_idx) {
                return *existing;
            }
        }

        let id = {
            let mut inner = self.inner.write().await;
            inner.next_id += 1;
            let id = inner.next_id;
            let now = unix_now();
            let state = JobState {
                id,
                sync_idx,
                status: JobStatus::Running,
                started_unix: now,
                finished_unix: None,
                output: String::new(),
                label,
                progress: None,
            };
            inner.jobs.insert(id, state);
            inner.order.push_back(id);
            while inner.order.len() > JOB_HISTORY_CAP {
                if let Some(old) = inner.order.pop_front() {
                    inner.jobs.remove(&old);
                }
            }
            inner.last_run.insert(sync_idx, now);
            id
        };

        self.running.lock().await.insert(sync_idx, id);

        let mgr = self.clone();
        tokio::spawn(async move {
            // Build a fresh tonic Channel per spawn — rclone runs
            // are typically minutes-to-hours apart, so caching the
            // client wouldn't save anything meaningful.
            let res = match engine_client::channel(&helper_socket).await {
                Ok(channel) => {
                    let mut client = EngineServiceClient::new(channel);
                    client
                        .run_cloud_sync(RunCloudSyncRequest {
                            idx: sync_idx as u32,
                        })
                        .await
                }
                Err(e) => Err(tonic::Status::unavailable(format!(
                    "engine unreachable: {e}"
                ))),
            };
            let mut inner = mgr.inner.write().await;
            if let Some(state) = inner.jobs.get_mut(&id) {
                state.finished_unix = Some(unix_now());
                match res {
                    Ok(resp) => {
                        state.status = JobStatus::Success;
                        state.output = truncate_tail(&resp.into_inner().output, MAX_OUTPUT_BYTES);
                    }
                    Err(status) => {
                        state.status = JobStatus::Failure;
                        state.output = format!("rclone failed: {status}");
                    }
                }
            }
            drop(inner);
            mgr.running.lock().await.remove(&sync_idx);
        });

        id
    }

    pub async fn list(&self) -> Vec<JobState> {
        let inner = self.inner.read().await;
        // Newest first. For each running job, overlay the live percent
        // the helper has been writing to /run/bananas/sync-progress/.
        inner
            .order
            .iter()
            .rev()
            .filter_map(|id| inner.jobs.get(id).cloned())
            .map(|mut s| {
                if s.status == JobStatus::Running {
                    s.progress = read_sync_progress(s.sync_idx);
                }
                s
            })
            .collect()
    }

    pub async fn get(&self, id: u64) -> Option<JobState> {
        let mut state = self.inner.read().await.jobs.get(&id).cloned()?;
        if state.status == JobStatus::Running {
            state.progress = read_sync_progress(state.sync_idx);
        }
        Some(state)
    }

    pub async fn last_run(&self, sync_idx: usize) -> Option<i64> {
        self.inner.read().await.last_run.get(&sync_idx).copied()
    }
}

/// Best-effort read of the live progress percent the helper writes to
/// `/run/bananas/sync-progress/<idx>.progress`. Returns None when the
/// file is absent (rclone hasn't started, finished, or crashed without
/// cleaning up — the latter is fine, the next run for this idx wipes
/// stale state on entry) or when the contents fail to parse.
fn read_sync_progress(sync_idx: usize) -> Option<u32> {
    let path = format!("/run/bananas/sync-progress/{sync_idx}.progress");
    let text = std::fs::read_to_string(&path).ok()?;
    text.trim().parse().ok().filter(|n: &u32| *n <= 100)
}

fn truncate_tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    let mut out = String::with_capacity(max + 32);
    out.push_str("[…output truncated…]\n");
    out.push_str(&s[start..]);
    out
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ─────────────────────────── cron scheduler ───────────────────────────

/// Spawn the cron scheduler. Wakes once a minute, reads cloud.toml via
/// the helper, and enqueues a job for every sync entry whose schedule
/// matches the current wall-clock minute. Idempotent within a minute
/// because `enqueue` debounces on the running set, and we update
/// `last_run` to the start-of-minute tick to skip duplicate fires.
pub fn spawn_scheduler(manager: JobManager, helper_socket: PathBuf) {
    tokio::spawn(async move {
        // Sleep until the start of the next minute so ticks line up
        // with cron's natural minute boundaries.
        let sleep_to_next_minute = || {
            let now = unix_now();
            let secs_into = (now % 60) as u64;
            std::time::Duration::from_secs(60 - secs_into.max(1))
        };
        loop {
            tokio::time::sleep(sleep_to_next_minute()).await;
            if let Err(e) = scheduler_tick(&manager, &helper_socket).await {
                tracing::warn!(error = %e, "cloud scheduler tick failed");
            }
        }
    });
}

async fn scheduler_tick(
    manager: &JobManager,
    helper_socket: &std::path::Path,
) -> anyhow::Result<()> {
    use bananas_proto::engine::v1::ReadServiceConfigRequest;
    let channel = engine_client::channel(helper_socket).await?;
    let mut client = EngineServiceClient::new(channel);
    let resp = match client
        .read_service_config(ReadServiceConfigRequest {
            name: "cloud".into(),
        })
        .await
    {
        Ok(r) => r.into_inner(),
        Err(_) => {
            // No cloud.toml yet — nothing to schedule.
            return Ok(());
        }
    };
    if resp.content.trim().is_empty() {
        return Ok(());
    }
    let cfg: CloudConfig = match toml::from_str(&resp.content) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "scheduler: cloud.toml parse failed");
            return Ok(());
        }
    };

    let now_secs = unix_now();
    let now_minute = now_secs / 60 * 60;

    for (idx, entry) in cfg.syncs.iter().enumerate() {
        let schedule = entry.schedule.trim();
        if schedule.is_empty() || schedule.eq_ignore_ascii_case("manual") {
            continue;
        }
        // Skip if we already fired in the same minute.
        if let Some(last) = manager.last_run(idx).await
            && last >= now_minute
        {
            continue;
        }
        if !cron_matches(schedule, now_secs) {
            continue;
        }
        let label = format!(
            "{}:{} → {}:{}",
            entry.account, entry.local_path, entry.account, entry.remote_path
        );
        let job_id = manager
            .enqueue(idx, label.clone(), helper_socket.to_path_buf())
            .await;
        tracing::info!(
            sync_idx = idx,
            job_id,
            schedule = schedule,
            label = %label,
            "scheduled cloud sync fired"
        );
    }
    Ok(())
}

// ─────────────────────────── cron matcher ───────────────────────────
//
// Tiny 5-field POSIX cron evaluator: minute hour day-of-month month
// day-of-week. Supports `*`, exact integers, `a-b` ranges, `a/n` step,
// and comma lists. No special @hourly / @daily aliases (operators can
// always type `0 * * * *`). UTC clock — same as systemd timers without
// a Persistent= override; matches what bananas-stats already assumes.

fn cron_matches(expr: &str, ts: i64) -> bool {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }
    let (min, hr, dom, mon, dow) = decompose_unix(ts);
    field_matches(fields[0], 0, 59, min)
        && field_matches(fields[1], 0, 23, hr)
        && field_matches(fields[2], 1, 31, dom)
        && field_matches(fields[3], 1, 12, mon)
        && field_matches(fields[4], 0, 6, dow)
}

/// Convert a Unix timestamp to (minute, hour, day-of-month, month,
/// day-of-week-Sun=0) all in UTC. Avoids pulling chrono just for this.
fn decompose_unix(ts: i64) -> (u32, u32, u32, u32, u32) {
    let secs = ts.rem_euclid(86_400) as u32;
    let min = (secs / 60) % 60;
    let hr = secs / 3600;
    let days_since_epoch = ts.div_euclid(86_400);
    // Jan 1 1970 was Thursday → dow 4 (Sun=0).
    let dow = ((days_since_epoch + 4).rem_euclid(7)) as u32;
    let (_year, month, dom) = ymd_from_days(days_since_epoch);
    (min, hr, dom, month, dow)
}

fn field_matches(field: &str, lo: u32, hi: u32, value: u32) -> bool {
    field
        .split(',')
        .any(|tok| token_matches(tok, lo, hi, value))
}

fn token_matches(tok: &str, lo: u32, hi: u32, value: u32) -> bool {
    if tok == "*" {
        return value >= lo && value <= hi;
    }
    // step form: <range>/<step>
    let (range_part, step) = match tok.split_once('/') {
        Some((r, s)) => (r, s.parse::<u32>().unwrap_or(1).max(1)),
        None => (tok, 1),
    };
    let (start, end) = if range_part == "*" {
        (lo, hi)
    } else if let Some((a, b)) = range_part.split_once('-') {
        let a: u32 = match a.parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        let b: u32 = match b.parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        (a, b)
    } else {
        let v: u32 = match range_part.parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        (v, v)
    };
    if value < start || value > end {
        return false;
    }
    (value - start) % step == 0
}

/// Date arithmetic ported from Howard Hinnant's algorithm — converts a
/// "days since 1970-01-01" count into (year, month, day-of-month).
/// Used by `decompose_unix`. Months 1-12, days 1-31. Branches map cleanly
/// to wraparound at March, which simplifies leap-year accounting.
fn ymd_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_star_matches_anything() {
        assert!(cron_matches("* * * * *", 0));
        assert!(cron_matches("* * * * *", 1_700_000_000));
    }

    #[test]
    fn cron_exact_minute() {
        // 2024-01-01 03:30:00 UTC = 1704079800
        assert!(cron_matches("30 3 * * *", 1_704_079_800));
        assert!(!cron_matches("31 3 * * *", 1_704_079_800));
    }

    #[test]
    fn cron_step() {
        // every 15 minutes — matches at minute 0, 15, 30, 45.
        assert!(cron_matches("*/15 * * * *", 1_704_067_200)); // :00
        assert!(cron_matches("*/15 * * * *", 1_704_068_100)); // :15
        assert!(!cron_matches("*/15 * * * *", 1_704_068_400)); // :20
    }

    #[test]
    fn cron_range() {
        // 2-4 in the minute field
        assert!(cron_matches("2-4 * * * *", 1_704_067_320)); // :02
        assert!(cron_matches("2-4 * * * *", 1_704_067_440)); // :04
        assert!(!cron_matches("2-4 * * * *", 1_704_067_500)); // :05
    }

    #[test]
    fn cron_dow() {
        // 2024-01-01 was a Monday → dow=1
        assert!(cron_matches("0 0 * * 1", 1_704_067_200)); // Mon 00:00
        assert!(!cron_matches("0 0 * * 0", 1_704_067_200)); // Sun expected, got Mon
    }
}
