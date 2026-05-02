//! Default landing page — read-only dashboard with live system tiles
//! (CPU / memory) plus per-interface and per-disk sparklines, then the
//! disks card grid + the read-only NFS exports summary.
//!
//! Data sources:
//!   /api/stats/snapshot  — current values (refreshed every 2s)
//!   /api/stats/series    — names of available interfaces / disks
//!   /api/stats/range     — last `window` of points per metric+key
//!   /api/storage         — disk health/usage cards (existing)
//!   /api/exports         — NFS exports summary (existing)

#![allow(non_snake_case)]

use dioxus::prelude::*;

use crate::{
    AuthCtx, api, api::ApiError, icons::Icon, stats_config::StatsConfigModal, storage::DiskCard,
};

const SPARKLINE_WINDOW: &str = "5m";

#[component]
pub fn StatsPage() -> Element {
    let auth_ctx = use_context::<AuthCtx>();
    let mut snapshot: Signal<Option<api::StatsSnapshot>> = use_signal(|| None);
    let mut series: Signal<api::SeriesKeys> = use_signal(api::SeriesKeys::default);
    let mut storage: Signal<Option<api::StorageReport>> = use_signal(|| None);
    let mut exports: Signal<Vec<api::ExportRow>> = use_signal(Vec::new);
    let mut errors: Signal<Vec<String>> = use_signal(Vec::new);
    let mut tick = use_signal(|| 0u32);
    let mut modal_open = use_signal(|| false);

    use_effect(move || {
        let _ = tick();
        // Snapshot — fast, served from latest SQLite rows.
        spawn(async move {
            match api::fetch_stats_snapshot().await {
                Ok(s) => snapshot.set(Some(s)),
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => push_err(&mut errors, format!("snapshot: {e}")),
            }
        });
        spawn(async move {
            match api::fetch_stats_series().await {
                Ok(s) => series.set(s),
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => push_err(&mut errors, format!("series: {e}")),
            }
        });
        spawn(async move {
            match api::fetch_storage().await {
                Ok(r) => storage.set(Some(r)),
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => push_err(&mut errors, format!("storage: {e}")),
            }
        });
        spawn(async move {
            match api::list_exports().await {
                Ok(list) => exports.set(list.rows),
                Err(ApiError::Unauthorized) => auth_ctx.signal_unauthorized(),
                Err(e) => push_err(&mut errors, format!("exports: {e}")),
            }
        });
    });

    // Live snapshot via WebSocket — server pushes once a second; we
    // never poll. On socket close (network blip, server restart) we
    // back off and reconnect indefinitely. Range queries (sparkline
    // history) continue to be plain HTTP and refetch when the user
    // hits the Refresh button.
    use_effect(move || {
        spawn(async move {
            let mut backoff_ms = 500u32;
            loop {
                match api::open_stats_ws().await {
                    Ok(mut ws) => {
                        backoff_ms = 500;
                        while let Some(snap) = ws.next_snapshot().await {
                            snapshot.set(Some(snap));
                        }
                    }
                    Err(e) => {
                        tracing::warn!(?e, "stats live open failed; backing off {}ms", backoff_ms);
                    }
                }
                gloo_timers::future::TimeoutFuture::new(backoff_ms).await;
                backoff_ms = (backoff_ms * 2).min(15_000);
            }
        });
    });

    rsx! {
        div { class: "section-header",
            h2 { "Live system" }
            span { class: "spacer" }
            button {
                class: "ghost",
                "data-tip": "Re-fetch storage + exports state. CPU/mem/network/disk poll every 2s automatically.",
                onclick: move |_| {
                    errors.set(Vec::new());
                    tick.set(tick() + 1);
                },
                Icon { name: "rotate-cw" }
                "Refresh"
            }
            button {
                class: "ghost",
                "data-tip": "Quick-edit /etc/bananas/stats.toml in a popover. The full editor lives in Settings → Stats.",
                onclick: move |_| modal_open.set(true),
                Icon { name: "settings" }
                "Stats config"
            }
        }

        for msg in errors.read().iter() {
            div { class: "banner err", pre { "{msg}" } }
        }

        if modal_open() {
            StatsConfigModal { on_close: move |_| modal_open.set(false) }
        }

        if let Some(s) = snapshot() {
            if !s.stats_db_present && s.ts_unix == 0 && s.network.is_empty() && s.disks.is_empty() {
                p { class: "preview-label",
                    "bananas-stats hasn't written its first row yet. Live tiles will populate within a few seconds of the daemon starting."
                }
            }
            LiveTiles { snap: s.clone() }
            NetworkSparklines { ifaces: series.read().interfaces.clone(), snap: s.clone() }
            DiskSparklines {
                disks: series.read().disks.clone(),
                snap: s,
                storage: storage(),
            }
        } else {
            p { class: "preview-label", "Loading live stats…" }
        }

        h2 { "Disks" }
        match storage() {
            None => rsx! { p { class: "preview-label", "Loading disks…" } },
            Some(r) if r.disks.is_empty() => rsx! { p { class: "empty", "No disks detected." } },
            Some(r) => rsx! {
                for disk in r.disks.iter() {
                    DiskCard {
                        key: "{disk.kname}",
                        disk: disk.clone(),
                        temp_c: snapshot().as_ref().and_then(|s| s.temps.iter()
                            .find(|t| t.sensor == disk.kname || t.sensor == disk.name)
                            .map(|t| t.celsius)),
                    }
                }
            }
        }

        h2 { "NFS exports" }
        if exports.read().is_empty() {
            p { class: "empty", "No exports defined yet — head to the Exports tab to add one." }
        } else {
            table { class: "rows",
                thead { tr { th { "Path" } th { "Client" } th { "Options" } } }
                tbody {
                    for row in exports.read().iter() {
                        ExportSummaryRow { key: "{row.idx}", row: row.clone() }
                    }
                }
            }
        }
    }
}

fn push_err(errors: &mut Signal<Vec<String>>, msg: String) {
    let mut current = errors.write();
    if !current.iter().any(|m| m == &msg) {
        current.push(msg);
    }
}

#[derive(Props, Clone, PartialEq)]
struct LiveTilesProps {
    snap: api::StatsSnapshot,
}

#[component]
fn LiveTiles(props: LiveTilesProps) -> Element {
    let s = &props.snap;
    let cpu_pct = s.cpu.busy_pct.clamp(0.0, 100.0);
    let mem_pct = if s.mem.total == 0 {
        0.0
    } else {
        (s.mem.used as f32 / s.mem.total as f32 * 100.0).clamp(0.0, 100.0)
    };
    // Pluck the on-die thermal-zone reading. Drivetemp readings for
    // disks aren't rendered as their own tiles anymore — the dedicated
    // "Temperatures" section was redundant; CPU temp lives inline in
    // the CPU card now (top-right corner badge), and disk temps can
    // come back inside the per-disk sparkline tiles if/when needed.
    let cpu_temp = s
        .temps
        .iter()
        .find(|t| t.sensor.starts_with("cpu") || t.sensor.contains("thermal"))
        .map(|t| t.celsius);

    // CPU tile: load% headline (the meaningful health signal), with
    // current clock as small secondary text when cpufreq exposes it.
    // Earlier this was reversed (MHz prominent, % small) but the
    // governor on the BPI scales down to 144 MHz when idle, making
    // the headline read "144 MHz" most of the time — useful info,
    // but worse at-a-glance signal than load%.
    let cpu_sub = match s.cpu.current_mhz {
        Some(mhz) if mhz >= 1000 => format!("{:.2} GHz", mhz as f32 / 1000.0),
        Some(mhz) => format!("{mhz} MHz"),
        None => "load".to_string(),
    };
    rsx! {
        div { class: "live-tiles",
            Gauge {
                label: "CPU",
                pct: cpu_pct,
                detail: cpu_sub,
                temp_c: cpu_temp,
            }
            Gauge {
                label: "Memory",
                pct: mem_pct,
                detail: format!("{} / {}", format_bytes(s.mem.used), format_bytes(s.mem.total)),
                temp_c: None,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct GaugeProps {
    label: &'static str,
    pct: f32,
    detail: String,
    /// Override the prominent number (defaults to `{pct}%` when None).
    /// CPU passes the clock here so the headline is "960 MHz" instead
    /// of "55%"; Memory leaves it None and keeps the percentage.
    #[props(default)]
    headline: Option<String>,
    /// Optional °C badge rendered top-right of the tile. Used for the
    /// CPU card; Memory passes `None`. Color flips ok/warn/danger at
    /// 60 °C and 75 °C — matches the BPI's Mali-400 throttle point and
    /// a typical SoC warning band.
    #[props(default)]
    temp_c: Option<f32>,
}

#[component]
fn Gauge(props: GaugeProps) -> Element {
    let kind = if props.pct >= 90.0 {
        "danger"
    } else if props.pct >= 75.0 {
        "warn"
    } else {
        "ok"
    };
    let pct_clamped = props.pct.clamp(0.0, 100.0);
    rsx! {
        div { class: "tile gauge-tile",
            div { class: "tile-label",
                "{props.label}"
                if let Some(c) = props.temp_c {
                    {
                        let temp_kind = if c >= 75.0 { "danger" } else if c >= 60.0 { "warn" } else { "ok" };
                        rsx! {
                            span { class: "tile-temp {temp_kind}", "{c:.0}°C" }
                        }
                    }
                }
            }
            div { class: "gauge-bar",
                div { class: "gauge-fill {kind}", style: "width: {pct_clamped}%" }
            }
            div { class: "tile-detail",
                {
                    let headline = props.headline.clone()
                        .unwrap_or_else(|| format!("{pct_clamped:.0}%"));
                    rsx! { span { class: "tile-pct {kind}", "{headline}" } }
                }
                span { class: "tile-sub", "{props.detail}" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct NetworkSparklinesProps {
    ifaces: Vec<String>,
    snap: api::StatsSnapshot,
}

#[component]
fn NetworkSparklines(props: NetworkSparklinesProps) -> Element {
    if props.ifaces.is_empty() {
        return rsx! {};
    }
    rsx! {
        h3 { class: "stats-subhead", "Network" }
        div { class: "spark-grid",
            for name in props.ifaces.iter() {
                NetSparkCard {
                    key: "{name}",
                    name: name.clone(),
                    current: props.snap.network.iter().find(|n| &n.name == name).cloned()
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct NetSparkCardProps {
    name: String,
    current: Option<api::NetIface>,
}

#[component]
fn NetSparkCard(props: NetSparkCardProps) -> Element {
    let name = props.name.clone();
    let series = use_resource({
        let name = name.clone();
        move || {
            let n = name.clone();
            async move { api::fetch_net_range(&n, SPARKLINE_WINDOW).await.ok() }
        }
    });

    let (rx, tx) = match &props.current {
        Some(n) => (n.rx_bps, n.tx_bps),
        None => (0, 0),
    };

    rsx! {
        div { class: "spark-card",
            div { class: "spark-head",
                code { class: "spark-name", "{props.name}" }
                div { class: "spark-now",
                    span { class: "spark-rx", "↓ {format_rate(rx)}" }
                    span { class: "spark-tx", "↑ {format_rate(tx)}" }
                }
            }
            match &*series.read_unchecked() {
                None => rsx! { div { class: "spark-empty", "Loading history…" } },
                Some(None) => rsx! { div { class: "spark-empty", "(unavailable)" } },
                Some(Some(points)) if points.is_empty() => rsx! {
                    div { class: "spark-empty", "No data in last {SPARKLINE_WINDOW} yet." }
                },
                Some(Some(points)) => rsx! {
                    NetSparkSvg { points: points.clone() }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct NetSparkSvgProps {
    points: Vec<api::NetSeriesPoint>,
}

#[component]
fn NetSparkSvg(props: NetSparkSvgProps) -> Element {
    let max = props
        .points
        .iter()
        .map(|p| p.rx_bps.max(p.tx_bps))
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let rx_path = build_polyline(&props.points, |p| p.rx_bps, max);
    let tx_path = build_polyline(&props.points, |p| p.tx_bps, max);
    rsx! {
        svg {
            class: "sparkline net",
            "viewBox": "0 0 100 30",
            "preserveAspectRatio": "none",
            polyline { class: "spark-rx-line", points: "{rx_path}" }
            polyline { class: "spark-tx-line", points: "{tx_path}" }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct DiskSparklinesProps {
    disks: Vec<String>,
    snap: api::StatsSnapshot,
    storage: Option<api::StorageReport>,
}

#[component]
fn DiskSparklines(props: DiskSparklinesProps) -> Element {
    if props.disks.is_empty() {
        return rsx! {};
    }
    rsx! {
        h3 { class: "stats-subhead", "Disk I/O" }
        div { class: "spark-grid",
            for dev in props.disks.iter() {
                DiskSparkCard {
                    key: "{dev}",
                    device: dev.clone(),
                    current: props.snap.disks.iter().find(|d| &d.device == dev).cloned(),
                    model: props.storage.as_ref()
                        .and_then(|r| r.disks.iter()
                            .find(|d| &d.kname == dev || &d.name == dev)
                            .and_then(|d| d.model.clone())),
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct DiskSparkCardProps {
    device: String,
    current: Option<api::DiskIo>,
    /// Plucked from /api/storage on the Stats page so the sparkline
    /// header gets the human-readable model under the kernel name —
    /// matches the "Disks" card layout (`/dev/sda` on top, model
    /// stacked below).
    model: Option<String>,
}

#[component]
fn DiskSparkCard(props: DiskSparkCardProps) -> Element {
    let dev = props.device.clone();
    let series = use_resource({
        let dev = dev.clone();
        move || {
            let d = dev.clone();
            async move { api::fetch_disk_range(&d, SPARKLINE_WINDOW).await.ok() }
        }
    });

    let (r, w, util) = match &props.current {
        Some(d) => (d.read_bps, d.write_bps, d.util_pct),
        None => (0, 0, 0.0),
    };

    rsx! {
        div { class: "spark-card",
            div { class: "spark-head",
                div { class: "spark-id",
                    code { class: "spark-name", "/dev/{props.device}" }
                    if let Some(m) = props.model.as_ref() {
                        span { class: "spark-model", "{m}" }
                    }
                }
                div { class: "spark-now",
                    span { class: "spark-rx", "R {format_rate(r)}" }
                    span { class: "spark-tx", "W {format_rate(w)}" }
                    span { class: "spark-util", title: "Utilization (% of time the device was busy)", "{util:.0}%" }
                }
            }
            match &*series.read_unchecked() {
                None => rsx! { div { class: "spark-empty", "Loading history…" } },
                Some(None) => rsx! { div { class: "spark-empty", "(unavailable)" } },
                Some(Some(points)) if points.is_empty() => rsx! {
                    div { class: "spark-empty", "No data in last {SPARKLINE_WINDOW} yet." }
                },
                Some(Some(points)) => rsx! {
                    DiskSparkSvg { points: points.clone() }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct DiskSparkSvgProps {
    points: Vec<api::DiskSeriesPoint>,
}

#[component]
fn DiskSparkSvg(props: DiskSparkSvgProps) -> Element {
    let max = props
        .points
        .iter()
        .map(|p| p.read_bps.max(p.write_bps))
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let r_path = build_polyline(&props.points, |p| p.read_bps, max);
    let w_path = build_polyline(&props.points, |p| p.write_bps, max);
    rsx! {
        svg {
            class: "sparkline disk",
            "viewBox": "0 0 100 30",
            "preserveAspectRatio": "none",
            polyline { class: "spark-rx-line", points: "{r_path}" }
            polyline { class: "spark-tx-line", points: "{w_path}" }
        }
    }
}

/// Build a `points="x1,y1 x2,y2 …"` string from a series of `T`s, with X
/// linearly spaced across [0,100] and Y inverted so taller is "more"
/// (SVG's Y grows downward; we flip it so the line draws like a regular
/// chart). Caller passes in the per-point value extractor and the max.
fn build_polyline<T, F>(pts: &[T], extract: F, max: f32) -> String
where
    F: Fn(&T) -> u64,
{
    if pts.len() < 2 {
        return String::new();
    }
    let n = pts.len() as f32;
    let mut out = String::with_capacity(pts.len() * 12);
    for (i, p) in pts.iter().enumerate() {
        let x = (i as f32) * 100.0 / (n - 1.0);
        let y = 30.0 - (extract(p) as f32 / max * 28.0).min(28.0);
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{x:.1},{y:.1}"));
    }
    out
}

#[derive(Props, Clone, PartialEq)]
struct ExportSummaryRowProps {
    row: api::ExportRow,
}

#[component]
fn ExportSummaryRow(props: ExportSummaryRowProps) -> Element {
    let r = &props.row;
    let p = &r.parsed;
    let mut bits: Vec<String> = Vec::new();
    bits.push((if p.rw { "rw" } else { "ro" }).into());
    if p.sync {
        bits.push("sync".into());
    } else {
        bits.push("async".into());
    }
    if p.no_subtree_check {
        bits.push("no_subtree_check".into());
    }
    if !p.squash.is_empty() {
        bits.push(p.squash.clone());
    }
    if let Some(uid) = p.anonuid {
        bits.push(format!("anonuid={uid}"));
    }
    if let Some(gid) = p.anongid {
        bits.push(format!("anongid={gid}"));
    }
    if p.insecure {
        bits.push("insecure".into());
    }
    let summary = bits.join(", ");
    rsx! {
        tr {
            td { code { "{r.path}" } }
            td { code { "{r.host}" } }
            td { code { "{summary}" } }
        }
    }
}

fn format_bytes(b: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if b == 0 {
        return "0 B".into();
    }
    let mut idx = 0;
    let mut v = b as f64;
    while v >= 1024.0 && idx < UNITS.len() - 1 {
        v /= 1024.0;
        idx += 1;
    }
    if v >= 100.0 || idx == 0 {
        format!("{:.0} {}", v, UNITS[idx])
    } else {
        format!("{:.1} {}", v, UNITS[idx])
    }
}

/// Bytes per second formatted as the most readable rate (B/s, KB/s, …).
fn format_rate(bps: u64) -> String {
    if bps == 0 {
        return "—".into();
    }
    let s = format_bytes(bps);
    format!("{s}/s")
}
