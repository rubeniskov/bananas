//! /api/storage — read-only disk dashboard.
//!
//! Strategy:
//!  1. Shell out to `lsblk -J -b` for the block-device tree (sizes, models,
//!     mountpoints, fstypes). `lsblk` works as the unprivileged `bananas`
//!     user — it reads /sys/class/block.
//!  2. For every leaf with a mountpoint, call `statvfs(2)` to get accurate
//!     used/available bytes (lsblk's FSUSE% column is rounded).
//!  3. For every "disk" type entry, ask the helper for SMART JSON in
//!     parallel via `JoinSet`. The server passes the helper's response
//!     straight through; the UI extracts whichever fields it wants.
//!  4. Cache the full report for `CACHE_TTL` so a Refresh-spamming
//!     operator doesn't wake spun-down disks every click. The TTL is
//!     short enough (30 s) that it feels live but long enough that
//!     repeat hits skip the smartctl spawn entirely.

use std::ffi::CString;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bananas_helper::{Command, Response as HelperResponse};
use serde::Serialize;
use serde_json::Value;
use tokio::task::JoinSet;

use crate::AppState;

/// How long a cached StorageReport stays fresh. SMART self-checks run at
/// kernel/drive cadence (typically minutes-to-hours apart), and lsblk
/// output rarely changes between operator clicks. 30 s strikes a balance
/// between "feels live" and "doesn't churn the disk on every refresh".
const CACHE_TTL: Duration = Duration::from_secs(30);

/// Process-wide cache of the most recent StorageReport. Wrapped in an
/// `Arc<Mutex<…>>` so it's cheap to clone into the AppState. The Mutex
/// is `std::sync::Mutex` (not tokio's) because every critical section
/// is a non-async clone-and-drop; we never hold it across `.await`.
#[derive(Clone, Default)]
pub struct StorageCache {
    inner: Arc<Mutex<Option<(Instant, StorageReport)>>>,
}

impl StorageCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_fresh(&self) -> Option<StorageReport> {
        let guard = self.inner.lock().ok()?;
        let (stored_at, report) = guard.as_ref()?;
        if stored_at.elapsed() < CACHE_TTL {
            Some(report.clone())
        } else {
            None
        }
    }

    fn store(&self, report: StorageReport) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Some((Instant::now(), report));
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct StorageReport {
    pub disks: Vec<Disk>,
    /// Anything that lsblk reported but couldn't classify (e.g. loop
    /// devices) — surfaced for transparency rather than swallowed.
    pub other: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Disk {
    pub name: String,
    pub kname: String,
    pub model: Option<String>,
    pub size: Option<u64>,
    pub readonly: bool,
    pub partitions: Vec<Partition>,
    /// Raw smartctl JSON from the helper (or null on failure).
    pub smart: Option<Value>,
    /// User-facing reason if smart is null (e.g. "smartctl unavailable").
    pub smart_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Partition {
    pub name: String,
    pub kname: String,
    pub size: Option<u64>,
    pub fstype: Option<String>,
    pub label: Option<String>,
    pub uuid: Option<String>,
    pub mountpoint: Option<String>,
    pub used: Option<u64>,
    pub available: Option<u64>,
    pub total: Option<u64>,
}

pub async fn get_storage(state: &AppState) -> Result<StorageReport, String> {
    // Cache hit short-circuits both the lsblk shellout and the per-disk
    // smartctl fan-out — important because smartctl wakes spun-down
    // drives on every poll. The TTL is short enough to feel live.
    if let Some(cached) = state.storage_cache.get_fresh() {
        return Ok(cached);
    }

    let lsblk = run_lsblk().await.map_err(|e| e.to_string())?;
    let mut report = StorageReport::default();

    let blockdevices = lsblk
        .get("blockdevices")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Pass 1: parse the lsblk tree and stage every disk into the report.
    // Record each disk's index + device path so pass 2 can write the
    // SMART result back to the right slot.
    let mut disk_targets: Vec<(usize, String)> = Vec::new();
    for entry in blockdevices {
        match classify(&entry) {
            EntryKind::Disk => {
                let disk = parse_disk(&entry);
                let device_path = format!("/dev/{}", disk.kname);
                disk_targets.push((report.disks.len(), device_path));
                report.disks.push(disk);
            }
            EntryKind::Other => report.other.push(entry),
        }
    }

    // Pass 2: fan smartctl calls out in parallel. The helper RPC is the
    // serial bottleneck — each call spawns smartctl on the BPI, which
    // takes a beat to wake the drive. Running them concurrently means
    // total latency = max(per-disk) instead of sum(per-disk).
    let mut tasks = JoinSet::new();
    for (idx, device_path) in disk_targets {
        let state = state.clone();
        tasks.spawn(async move {
            let res = fetch_smart(&state, &device_path).await;
            (idx, res)
        });
    }
    while let Some(Ok((idx, res))) = tasks.join_next().await {
        match res {
            Ok(json) => report.disks[idx].smart = Some(json),
            Err(e) => report.disks[idx].smart_error = Some(e),
        }
    }

    state.storage_cache.store(report.clone());
    Ok(report)
}

enum EntryKind {
    Disk,
    Other,
}

fn classify(entry: &Value) -> EntryKind {
    match entry.get("type").and_then(|v| v.as_str()) {
        Some("disk") => EntryKind::Disk,
        _ => EntryKind::Other,
    }
}

fn parse_disk(entry: &Value) -> Disk {
    let mut disk = Disk {
        name: get_str(entry, "name").unwrap_or_default(),
        kname: get_str(entry, "kname").unwrap_or_default(),
        model: get_str(entry, "model")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        size: get_u64(entry, "size"),
        readonly: matches!(get_bool_or_str(entry, "ro"), Some(true)),
        partitions: vec![],
        smart: None,
        smart_error: None,
    };
    if let Some(children) = entry.get("children").and_then(|v| v.as_array()) {
        for child in children {
            disk.partitions.push(parse_partition(child));
        }
    }
    // A disk with no partitions but a mountpoint of its own (raw fs on the
    // device, no partition table) — model it as a single self-partition so
    // the UI shows usage just like the partitioned case.
    if disk.partitions.is_empty() && entry.get("mountpoint").and_then(|v| v.as_str()).is_some() {
        disk.partitions.push(parse_partition(entry));
    }
    disk
}

fn parse_partition(entry: &Value) -> Partition {
    let mountpoint = get_str(entry, "mountpoint");
    let (used, available, total) = match mountpoint.as_deref() {
        Some(mp) => match statvfs_for(mp) {
            Some(stats) => (Some(stats.used), Some(stats.available), Some(stats.total)),
            None => (None, None, None),
        },
        None => (None, None, None),
    };
    Partition {
        name: get_str(entry, "name").unwrap_or_default(),
        kname: get_str(entry, "kname").unwrap_or_default(),
        size: get_u64(entry, "size"),
        fstype: get_str(entry, "fstype"),
        label: get_str(entry, "label"),
        uuid: get_str(entry, "uuid"),
        mountpoint,
        used,
        available,
        total,
    }
}

async fn run_lsblk() -> anyhow::Result<Value> {
    use anyhow::Context;
    let out = tokio::process::Command::new("lsblk")
        .args([
            "-J",
            "-b",
            "-o",
            "NAME,KNAME,SIZE,MODEL,TYPE,MOUNTPOINT,FSTYPE,LABEL,UUID,RO",
        ])
        .output()
        .await
        .context("running lsblk")?;
    if !out.status.success() {
        anyhow::bail!(
            "lsblk failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).context("parsing lsblk JSON")
}

async fn fetch_smart(state: &AppState, device: &str) -> Result<Value, String> {
    let cmd = Command::Smart {
        device: device.to_string(),
    };
    match bananas_helper::call(&state.helper_socket, &cmd).await {
        Ok(HelperResponse {
            ok: true, output, ..
        }) => serde_json::from_str(&output).map_err(|e| format!("smartctl JSON: {e}")),
        Ok(HelperResponse { error, .. }) => Err(error.unwrap_or_else(|| "smartctl failed".into())),
        Err(e) => Err(format!("helper call: {e}")),
    }
}

struct VfsStats {
    used: u64,
    available: u64,
    total: u64,
}

/// Direct statvfs(2) call. Returns None if the path doesn't exist or the
/// call errors — Storage rendering treats this the same as "unmounted".
fn statvfs_for(path: &str) -> Option<VfsStats> {
    let c_path = CString::new(path).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    let bsize = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * bsize;
    let available = stat.f_bavail as u64 * bsize;
    let free = stat.f_bfree as u64 * bsize;
    let used = total.saturating_sub(free);
    Some(VfsStats {
        used,
        available,
        total,
    })
}

// --- small helpers for digging into lsblk's loose JSON --------------------

fn get_str(entry: &Value, key: &str) -> Option<String> {
    entry
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn get_u64(entry: &Value, key: &str) -> Option<u64> {
    entry.get(key).and_then(|v| v.as_u64())
}

/// lsblk reports `ro` as either a bool or a "0"/"1" string depending on
/// version. Coerce both.
fn get_bool_or_str(entry: &Value, key: &str) -> Option<bool> {
    let v = entry.get(key)?;
    if let Some(b) = v.as_bool() {
        return Some(b);
    }
    if let Some(s) = v.as_str() {
        return Some(s == "1" || s.eq_ignore_ascii_case("true"));
    }
    if let Some(n) = v.as_u64() {
        return Some(n != 0);
    }
    None
}
