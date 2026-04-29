//! Storage page — currently the /etc/fstab editor (mount-points
//! management). Disk usage and SMART status moved to the Stats tab so
//! this page is purely about *what to mount where*. The DiskCard
//! component is reused by `stats.rs` and stays here as the canonical
//! place to render a single disk.

#![allow(non_snake_case)]

use dioxus::prelude::*;

use crate::{api, mounts::MountsSection};

#[component]
pub fn StoragePage() -> Element {
    rsx! { MountsSection {} }
}

#[derive(Props, Clone, PartialEq)]
pub struct DiskCardProps {
    pub disk: api::Disk,
    /// Live drivetemp reading (°C) plucked from the latest stats
    /// snapshot. None when no temp sensor matches the device name —
    /// the badge is then omitted entirely.
    #[props(default)]
    pub temp_c: Option<f32>,
}

#[component]
pub fn DiskCard(props: DiskCardProps) -> Element {
    let d = &props.disk;
    let model = d.model.clone().unwrap_or_else(|| "Unknown model".into());
    let size = d.size.map(format_bytes).unwrap_or_else(|| "—".into());
    let smart = smart_summary(d);
    // 50/42 °C thresholds match a typical drivetemp red/yellow band
    // for a spinning HDD. NVMe/SATA SSDs run cooler so this rarely
    // trips on solid state.
    let temp_kind = props.temp_c.map(|c| {
        if c >= 50.0 {
            "danger"
        } else if c >= 42.0 {
            "warn"
        } else {
            "ok"
        }
    });

    rsx! {
        section { class: "disk-card",
            header { class: "disk-head",
                div { class: "disk-head-left",
                    div { class: "disk-name", code { "/dev/{d.kname}" } }
                    div { class: "disk-model", "{model}" }
                    div { class: "disk-smart-row",
                        SmartBadge { summary: smart }
                        if d.readonly { span { class: "badge warn", "read-only" } }
                    }
                }
                div { class: "disk-head-right",
                    if let (Some(c), Some(kind)) = (props.temp_c, temp_kind) {
                        span { class: "disk-temp {kind}", "{c:.0}°C" }
                    }
                    span { class: "disk-size", "{size}" }
                }
            }

            if d.partitions.is_empty() {
                p { class: "preview-label", style: "padding: .8em 1em",
                    "No partitions or filesystems detected on this device."
                }
            } else {
                table { class: "rows",
                    thead {
                        tr {
                            th { "Partition" }
                            th { "Mount" }
                            th { "Type" }
                            th { "Label" }
                            th { "Usage" }
                        }
                    }
                    tbody {
                        for p in d.partitions.iter() {
                            PartitionRow { key: "{p.kname}", part: p.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct PartitionRowProps {
    part: api::Partition,
}

#[component]
fn PartitionRow(props: PartitionRowProps) -> Element {
    let p = &props.part;
    let mount_cell = match &p.mountpoint {
        Some(mp) => rsx! { code { "{mp}" } },
        None => rsx! { span { class: "muted", "(unmounted)" } },
    };
    let label = p.label.clone().unwrap_or_default();
    let fstype = p.fstype.clone().unwrap_or_default();

    rsx! {
        tr {
            td { code { "/dev/{p.kname}" } }
            td { {mount_cell} }
            td { code { "{fstype}" } }
            td { "{label}" }
            td {
                match (p.used, p.total) {
                    (Some(used), Some(total)) if total > 0 => rsx! {
                        UsageBar { used, total }
                    },
                    _ => rsx! {
                        span { class: "muted",
                            { p.size.map(format_bytes).unwrap_or_else(|| "—".into()) }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct UsageBarProps {
    used: u64,
    total: u64,
}

#[component]
fn UsageBar(props: UsageBarProps) -> Element {
    let pct = if props.total == 0 {
        0.0
    } else {
        (props.used as f64 / props.total as f64) * 100.0
    };
    let pct_clamped = pct.clamp(0.0, 100.0);
    let kind = if pct >= 90.0 {
        "danger"
    } else if pct >= 75.0 {
        "warn"
    } else {
        "ok"
    };
    let used_label = format_bytes(props.used);
    let total_label = format_bytes(props.total);
    let pct_label = format!("{:.0}%", pct);

    rsx! {
        div { class: "usage",
            div { class: "usage-bar",
                div { class: "usage-fill {kind}", style: "width: {pct_clamped}%" }
            }
            div { class: "usage-text",
                span { "{used_label} / {total_label}" }
                span { class: "usage-pct {kind}", "{pct_label}" }
            }
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
struct SmartSummary {
    /// "ok" | "warn" | "fail" | "unknown"
    kind: &'static str,
    label: String,
    /// Tooltip body — extra detail the user can hover for.
    detail: String,
}

#[derive(Props, Clone, PartialEq)]
struct SmartBadgeProps {
    summary: SmartSummary,
}

#[component]
fn SmartBadge(props: SmartBadgeProps) -> Element {
    let s = &props.summary;
    let class = match s.kind {
        "ok" => "badge",
        "warn" => "badge warn",
        "fail" => "badge ro",
        _ => "badge",
    };
    rsx! {
        span { class: "{class}", "data-tip": "{s.detail}", "SMART: {s.label}" }
    }
}

fn smart_summary(disk: &api::Disk) -> SmartSummary {
    if let Some(err) = &disk.smart_error {
        return SmartSummary {
            kind: "unknown",
            label: "n/a".into(),
            detail: err.clone(),
        };
    }
    let Some(smart) = &disk.smart else {
        return SmartSummary {
            kind: "unknown",
            label: "n/a".into(),
            detail: String::new(),
        };
    };
    let passed = smart
        .get("smart_status")
        .and_then(|v| v.get("passed"))
        .and_then(|v| v.as_bool());
    match passed {
        Some(true) => SmartSummary {
            kind: "ok",
            label: "passed".into(),
            detail: smart_detail_summary(smart),
        },
        Some(false) => SmartSummary {
            kind: "fail",
            label: "FAILING".into(),
            detail: "Drive predicts imminent failure. Replace ASAP.".into(),
        },
        None => {
            // Some firmwares only export raw attributes, not the boolean.
            // Surface that as "unknown" rather than misleadingly green.
            let warn = smart
                .get("messages")
                .and_then(|m| m.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            SmartSummary {
                kind: if warn { "warn" } else { "unknown" },
                label: "unknown".into(),
                detail: messages_or_default(smart),
            }
        }
    }
}

fn smart_detail_summary(smart: &serde_json::Value) -> String {
    let mut bits: Vec<String> = Vec::new();
    if let Some(temp) = smart
        .get("temperature")
        .and_then(|t| t.get("current"))
        .and_then(|t| t.as_i64())
    {
        bits.push(format!("Temp: {}°C", temp));
    }
    if let Some(hours) = smart
        .get("power_on_time")
        .and_then(|p| p.get("hours"))
        .and_then(|h| h.as_u64())
    {
        bits.push(format!("Power-on: {} h", hours));
    }
    if let Some(start_stop) = smart.get("power_cycle_count").and_then(|v| v.as_u64()) {
        bits.push(format!("Power cycles: {}", start_stop));
    }
    if bits.is_empty() {
        "Drive reports healthy.".into()
    } else {
        bits.join(" · ")
    }
}

fn messages_or_default(smart: &serde_json::Value) -> String {
    smart
        .get("messages")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.get("string").and_then(|s| s.as_str()))
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_else(|| "smartctl did not report a pass/fail status".into())
}

fn format_bytes(b: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
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
