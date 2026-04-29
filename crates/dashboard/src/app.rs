//! Wires `tokio::sync::watch::Receiver<Snapshot>` into the Slint event loop.
//!
//! Slint owns the main thread; we keep a `tokio::runtime::Handle` so the
//! caller can hand us the receiver and we can drive a small task that funnels
//! snapshots back into the UI via `slint::invoke_from_event_loop`.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use anyhow::Result;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tokio::sync::watch;

use bananas_stats::config::Ui as UiCfg;
use bananas_stats::metrics::Snapshot;

slint::include_modules!();

/// Rolling per-key history kept in-process for live sparklines.
struct History {
    buf: HashMap<String, VecDeque<u64>>,
    cap: usize,
}

impl History {
    fn new(cap: usize) -> Self {
        Self { buf: HashMap::new(), cap }
    }
    fn push(&mut self, key: &str, v: u64) {
        let q = self.buf.entry(key.to_string()).or_default();
        if q.len() == self.cap {
            q.pop_front();
        }
        q.push_back(v);
    }
    /// Return values for `keys`, normalised to [0..1] against the per-key max
    /// so the .slint sparklines can draw without re-deriving the peak.
    fn normalised_for(&self, keys: &[String]) -> Vec<Vec<f32>> {
        keys.iter()
            .map(|k| {
                let q = match self.buf.get(k) {
                    Some(q) if !q.is_empty() => q,
                    _ => return Vec::<f32>::new(),
                };
                let peak = (*q.iter().max().unwrap_or(&1)).max(1) as f32;
                q.iter().map(|&v| (v as f32) / peak).collect()
            })
            .collect()
    }
}

pub fn launch(
    cfg: UiCfg,
    mut rx: watch::Receiver<Snapshot>,
    local_offset: time::UtcOffset,
) -> Result<()> {
    let main = MainWindow::new()?;
    main.window().set_size(slint::PhysicalSize::new(cfg.width, cfg.height));
    main.set_ts_text(SharedString::from("--:--:--"));

    // Seed the theme from config. "auto" picks dark/light from the local
    // hour and starts a watcher that flips at noon/midnight; "dark" /
    // "light" pin the theme.
    let initial_dark = match cfg.theme.as_str() {
        "light" => false,
        "dark" => true,
        _ => is_pm_local(local_offset),
    };
    main.global::<Theme>().set_dark(initial_dark);

    if cfg.theme == "auto" {
        let weak = main.as_weak();
        tokio::runtime::Handle::current().spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            let mut last_pm = is_pm_local(local_offset);
            loop {
                tick.tick().await;
                let now_pm = is_pm_local(local_offset);
                if now_pm == last_pm {
                    continue;
                }
                last_pm = now_pm;
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = weak.upgrade() {
                        w.global::<Theme>().set_dark(now_pm);
                    }
                });
            }
        });
    }

    let weak = main.as_weak();
    let spark_window = cfg.spark_window;

    // Borrow the *current* tokio runtime — `main.rs` enters a multi-thread
    // runtime before calling us, so `Handle::current` works.
    let rt = tokio::runtime::Handle::current();
    let _pump = rt.spawn(async move {
        let mut net_rx = History::new(spark_window);
        let mut net_tx = History::new(spark_window);
        let mut disk_r = History::new(spark_window);
        let mut disk_w = History::new(spark_window);

        loop {
            if rx.changed().await.is_err() {
                tracing::debug!("sampler channel closed; UI pump exiting");
                break;
            }
            let snap = rx.borrow_and_update().clone();
            tracing::debug!(
                ts = snap.ts_unix,
                ifaces = snap.network.len(),
                disks = snap.disks.len(),
                parts = snap.parts.len(),
                "ui pump tick"
            );

            for n in &snap.network {
                net_rx.push(&n.name, n.rx_bps);
                net_tx.push(&n.name, n.tx_bps);
            }
            for d in &snap.disks {
                disk_r.push(&d.device, d.read_bps);
                disk_w.push(&d.device, d.write_bps);
            }

            let net_keys: Vec<String> = snap.network.iter().map(|n| n.name.clone()).collect();
            let disk_keys: Vec<String> = snap.disks.iter().map(|d| d.device.clone()).collect();
            let net_rx_norm = net_rx.normalised_for(&net_keys);
            let net_tx_norm = net_tx.normalised_for(&net_keys);
            let disk_r_norm = disk_r.normalised_for(&disk_keys);
            let disk_w_norm = disk_w.normalised_for(&disk_keys);

            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = weak.upgrade() {
                    apply_snapshot(&w, &snap, net_rx_norm, net_tx_norm, disk_r_norm, disk_w_norm);
                }
            });
        }
    });

    main.run()?;
    Ok(())
}

fn apply_snapshot(
    w: &MainWindow,
    snap: &Snapshot,
    net_rx_norm: Vec<Vec<f32>>,
    net_tx_norm: Vec<Vec<f32>>,
    disk_r_norm: Vec<Vec<f32>>,
    disk_w_norm: Vec<Vec<f32>>,
) {
    w.set_ts_text(SharedString::from(format_ts(snap.ts_unix)));

    // Compact header values
    w.set_cpu_pct((snap.cpu.busy_pct / 100.0).clamp(0.0, 1.0));
    let mem_pct = if snap.mem.total == 0 {
        0.0
    } else {
        (snap.mem.used as f64 / snap.mem.total as f64).clamp(0.0, 1.0) as f32
    };
    w.set_mem_pct(mem_pct);

    let net: Vec<NetIfaceUi> = snap
        .network
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let (rx_v, rx_u) = format_rate_split(n.rx_bps);
            let (tx_v, tx_u) = format_rate_split(n.tx_bps);
            let (rx_peak, rx_mid) = peak_labels(net_rx_norm.get(i), n.rx_bps);
            let (tx_peak, tx_mid) = peak_labels(net_tx_norm.get(i), n.tx_bps);
            NetIfaceUi {
                name: SharedString::from(n.name.clone()),
                rx_value: SharedString::from(rx_v),
                rx_unit: SharedString::from(rx_u),
                rx_peak_text: SharedString::from(rx_peak),
                rx_mid_text: SharedString::from(rx_mid),
                tx_value: SharedString::from(tx_v),
                tx_unit: SharedString::from(tx_u),
                tx_peak_text: SharedString::from(tx_peak),
                tx_mid_text: SharedString::from(tx_mid),
            }
        })
        .collect();
    w.set_network(ModelRc::new(VecModel::from(net)));

    let disks: Vec<DiskIoUi> = snap
        .disks
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let (r_v, r_u) = format_rate_split(d.read_bps);
            let (w_v, w_u) = format_rate_split(d.write_bps);
            let (r_peak, r_mid) = peak_labels(disk_r_norm.get(i), d.read_bps);
            let (w_peak, w_mid) = peak_labels(disk_w_norm.get(i), d.write_bps);
            DiskIoUi {
                device: SharedString::from(d.device.clone()),
                bus: SharedString::from(d.bus.to_uppercase()),
                read_value: SharedString::from(r_v),
                read_unit: SharedString::from(r_u),
                read_peak_text: SharedString::from(r_peak),
                read_mid_text: SharedString::from(r_mid),
                write_value: SharedString::from(w_v),
                write_unit: SharedString::from(w_u),
                write_peak_text: SharedString::from(w_peak),
                write_mid_text: SharedString::from(w_mid),
                util_text: SharedString::from(format!("{:.0}% busy", d.util_pct)),
            }
        })
        .collect();
    w.set_disks(ModelRc::new(VecModel::from(disks)));

    let parts: Vec<PartitionUi> = snap
        .parts
        .iter()
        .map(|p| {
            let pct = if p.total == 0 {
                0.0
            } else {
                (p.used as f64 / p.total as f64).clamp(0.0, 1.0) as f32
            };
            PartitionUi {
                device: SharedString::from(p.device.clone()),
                mount: SharedString::from(p.mount.clone()),
                fs: SharedString::from(p.fs.clone()),
                used_text: SharedString::from(format_bytes(p.used)),
                total_text: SharedString::from(format_bytes(p.total)),
                pct,
                pct_text: SharedString::from(format!("{:.0}%", pct * 100.0)),
            }
        })
        .collect();
    w.set_partitions(ModelRc::new(VecModel::from(parts)));

    w.set_net_rx_spark(spark_model(net_rx_norm));
    w.set_net_tx_spark(spark_model(net_tx_norm));
    w.set_disk_read_spark(spark_model(disk_r_norm));
    w.set_disk_write_spark(spark_model(disk_w_norm));
}

fn spark_model(rows: Vec<Vec<f32>>) -> ModelRc<ModelRc<f32>> {
    let inner: Vec<ModelRc<f32>> = rows
        .into_iter()
        .map(|r| ModelRc::new(VecModel::from(r)))
        .collect();
    ModelRc::new(VecModel::from(inner))
}

/// True when the current local hour is in the PM half (12:00 – 23:59).
fn is_pm_local(offset: time::UtcOffset) -> bool {
    time::OffsetDateTime::now_utc()
        .to_offset(offset)
        .hour()
        >= 12
}

fn format_ts(ts_unix: i64) -> String {
    if ts_unix <= 0 {
        return String::new();
    }
    time::OffsetDateTime::from_unix_timestamp(ts_unix)
        .ok()
        .and_then(|t| {
            let f = time::format_description::parse("[hour]:[minute]:[second]").ok()?;
            t.format(&f).ok()
        })
        .unwrap_or_default()
}

fn format_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

/// Same idea as `format_bytes`, but returns the numeric value and the unit
/// separately so the .slint side can render them at different font sizes.
fn format_rate_split(bps: u64) -> (String, String) {
    const UNITS: &[&str] = &["B/s", "KB/s", "MB/s", "GB/s"];
    let mut v = bps as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let val = if i == 0 {
        format!("{bps}")
    } else if v >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    };
    (val, UNITS[i].to_string())
}

/// Peak / mid labels for the sparkline Y-axis, formatted with binary units
/// (B / KB / MB / GB) so the axis matches what a human reads in the
/// headline rate. The peak is rounded up to a 1/2/5 × 1024^k boundary so
/// labels fall on tidy values (10 KB, 200 KB, 1 MB) rather than 437 B.
fn peak_labels(history: Option<&Vec<f32>>, current_bps: u64) -> (String, String) {
    let peak = history
        .and_then(|h| h.iter().copied().fold(None, |acc, v| {
            Some(acc.map_or(v, |a: f32| a.max(v)))
        }))
        .map(|max_norm| {
            (max_norm * current_bps as f32).max(current_bps as f32)
        })
        .unwrap_or(current_bps as f32)
        .max(1.0);

    let peak_round = nice_ceiling_binary(peak);
    let mid = peak_round / 2.0;

    (short_bytes(peak_round), short_bytes(mid))
}

/// Round `v` up to the next 1 / 2 / 5 × 1024^k boundary. Picks tidy axis
/// values (1 KB, 2 KB, 5 KB, 10 KB, …) at byte-aware breakpoints.
fn nice_ceiling_binary(v: f32) -> f32 {
    if v <= 0.0 {
        return 1.0;
    }
    // Find the right 1024^k bucket.
    let mut bucket = 1.0_f32;
    while v >= bucket * 1024.0 {
        bucket *= 1024.0;
    }
    let m = v / bucket;
    let n = if m <= 1.0 { 1.0 }
            else if m <= 2.0 { 2.0 }
            else if m <= 5.0 { 5.0 }
            else if m <= 10.0 { 10.0 }
            else if m <= 20.0 { 20.0 }
            else if m <= 50.0 { 50.0 }
            else if m <= 100.0 { 100.0 }
            else if m <= 200.0 { 200.0 }
            else if m <= 500.0 { 500.0 }
            else { 1024.0 };
    n * bucket
}

/// Compact byte-rate label: "200", "5 KB", "1 MB", "2 GB", etc. Drops the
/// "/s" suffix since the time axis is implicit on a sparkline.
fn short_bytes(v: f32) -> String {
    let abs = v.abs();
    if abs >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.0} GB", v / (1024.0 * 1024.0 * 1024.0))
    } else if abs >= 1024.0 * 1024.0 {
        format!("{:.0} MB", v / (1024.0 * 1024.0))
    } else if abs >= 1024.0 {
        format!("{:.0} KB", v / 1024.0)
    } else {
        format!("{:.0} B", v)
    }
}

// Keep `Rc` import wired in; some Slint API surfaces still expect it.
#[allow(dead_code)]
fn _rc_marker(_: Rc<()>, _: &dyn Model<Data = i32>) {}
