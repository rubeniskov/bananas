//! Form-based stats-service config editor. Renders inline inside the
//! Settings → Stats tab. Loads /etc/bananas/stats.toml through
//! /api/stats/config (server falls back to compiled-in defaults when
//! the file is missing), parses it into typed form fields, lets the
//! operator edit each value, and serializes a clean TOML payload on
//! save. The composed TOML is also surfaced in a read-only textarea
//! so a power user can copy / inspect it without leaving the page.
//!
//! [ui] / [live_socket] sections moved to dashboard.toml after the
//! split, so this module no longer surfaces dashboard appearance
//! fields. Those live in `dashboard_config::DashboardConfigForm`.

#![allow(non_snake_case)]

use dioxus::prelude::*;
use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{AuthCtx, components::TextareaWithCopy};

#[derive(Debug, Clone, Deserialize)]
struct ConfigResp {
    #[serde(default)]
    config: String,
}

/// Mirror of `bananas_stats::config::Config` minus the dashboard-side
/// fields. Mirrored here (rather than shared via a workspace crate) so
/// the wasm bundle doesn't pull rusqlite + nix + tokio in transitively.
/// Keep in sync with the upstream defaults — the recipe at
/// `recipes-bsp/bananas-stats/files/stats.toml` is the canonical
/// source-of-truth for the on-disk format.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Cfg {
    sampling: Sampling,
    storage: Storage,
    devices: Devices,
    network: Network,
    live_socket: LiveSocket,
}

/// Path of the Unix socket bananas-stats binds for live pub/sub. Not
/// surfaced in the form (operators rarely change it) but round-tripped
/// in the TOML so saving from the form doesn't drop the section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct LiveSocket {
    path: String,
}
impl Default for LiveSocket {
    fn default() -> Self {
        Self {
            path: "/run/bananas-stats/live.sock".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Sampling {
    interval_ms: u64,
}
impl Default for Sampling {
    fn default() -> Self {
        Self { interval_ms: 1000 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Storage {
    path: String,
    raw_retention_hours: u32,
    agg_retention_days: u32,
    flush_interval_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_db_mb: Option<u64>,
}
impl Default for Storage {
    fn default() -> Self {
        Self {
            path: "/var/lib/bananas/stats.db".into(),
            raw_retention_hours: 24,
            agg_retention_days: 30,
            flush_interval_ms: 5000,
            max_db_mb: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Devices {
    include_buses: Vec<String>,
    exclude_names: Vec<String>,
}
impl Default for Devices {
    fn default() -> Self {
        Self {
            include_buses: vec!["sata".into(), "usb".into()],
            exclude_names: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Network {
    include_patterns: Vec<String>,
    exclude_names: Vec<String>,
}
impl Default for Network {
    fn default() -> Self {
        Self {
            include_patterns: vec!["eth*".into(), "en*".into()],
            exclude_names: vec![],
        }
    }
}

fn parse_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn join_csv(v: &[String]) -> String {
    v.join(", ")
}

#[component]
pub fn StatsConfigForm() -> Element {
    let auth_ctx = use_context::<AuthCtx>();

    // We keep the loaded Cfg around so non-form fields (e.g.
    // live_socket.path) round-trip on save without ever being shown
    // in the UI.
    let mut cfg = use_signal(Cfg::default);
    let mut interval_ms = use_signal(|| "1000".to_string());
    let mut storage_path = use_signal(|| "/var/lib/bananas/stats.db".to_string());
    let mut raw_hours = use_signal(|| "24".to_string());
    let mut agg_days = use_signal(|| "30".to_string());
    let mut flush_ms = use_signal(|| "5000".to_string());
    let mut max_db_mb = use_signal(String::new); // empty = auto
    let mut devices_buses = use_signal(|| "sata, usb".to_string());
    let mut devices_excl = use_signal(String::new);
    let mut net_include = use_signal(|| "eth*, en*".to_string());
    let mut net_exclude = use_signal(String::new);

    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut info: Signal<Option<String>> = use_signal(|| None);
    let mut busy = use_signal(|| false);
    let mut hydrated = use_signal(|| false);
    let mut load_failed = use_signal(|| false);

    use_effect(move || {
        if hydrated() {
            return;
        }
        spawn(async move {
            match Request::get("/api/stats/config").send().await {
                Ok(r) if r.status() == 401 => auth_ctx.signal_unauthorized(),
                Ok(r) if r.ok() => match r.json::<ConfigResp>().await {
                    Ok(body) => {
                        let parsed: Cfg = toml::from_str(&body.config).unwrap_or_default();
                        interval_ms.set(parsed.sampling.interval_ms.to_string());
                        storage_path.set(parsed.storage.path.clone());
                        raw_hours.set(parsed.storage.raw_retention_hours.to_string());
                        agg_days.set(parsed.storage.agg_retention_days.to_string());
                        flush_ms.set(parsed.storage.flush_interval_ms.to_string());
                        max_db_mb.set(
                            parsed
                                .storage
                                .max_db_mb
                                .map(|n| n.to_string())
                                .unwrap_or_default(),
                        );
                        devices_buses.set(join_csv(&parsed.devices.include_buses));
                        devices_excl.set(join_csv(&parsed.devices.exclude_names));
                        net_include.set(join_csv(&parsed.network.include_patterns));
                        net_exclude.set(join_csv(&parsed.network.exclude_names));
                        cfg.set(parsed);
                        hydrated.set(true);
                    }
                    Err(e) => {
                        error.set(Some(format!("Could not parse config: {e}")));
                        load_failed.set(true);
                    }
                },
                Ok(r) => {
                    error.set(Some(format!("HTTP {} loading config", r.status())));
                    load_failed.set(true);
                }
                Err(e) => {
                    error.set(Some(format!("Network error: {e}")));
                    load_failed.set(true);
                }
            }
        });
    });

    let composed_toml = move || -> String {
        let mut new = Cfg::default();
        new.sampling.interval_ms = interval_ms().parse().unwrap_or(1000);
        new.storage.path = storage_path();
        new.storage.raw_retention_hours = raw_hours().parse().unwrap_or(24);
        new.storage.agg_retention_days = agg_days().parse().unwrap_or(30);
        new.storage.flush_interval_ms = flush_ms().parse().unwrap_or(5000);
        new.storage.max_db_mb = max_db_mb().trim().parse().ok();
        new.devices.include_buses = parse_csv(&devices_buses());
        new.devices.exclude_names = parse_csv(&devices_excl());
        new.network.include_patterns = parse_csv(&net_include());
        new.network.exclude_names = parse_csv(&net_exclude());
        new.live_socket = cfg().live_socket.clone();
        toml::to_string_pretty(&new).unwrap_or_default()
    };

    let preview = composed_toml();

    let mut submit = move |_| {
        if busy() || !hydrated() {
            return;
        }
        busy.set(true);
        error.set(None);
        info.set(None);
        let body = json!({ "config": composed_toml() });
        spawn(async move {
            let resp = Request::put("/api/stats/config")
                .header("content-type", "application/json")
                .body(body.to_string());
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    busy.set(false);
                    error.set(Some(format!("Could not build request: {e}")));
                    return;
                }
            };
            match resp.send().await {
                Ok(r) if r.status() == 401 => {
                    busy.set(false);
                    auth_ctx.signal_unauthorized();
                }
                Ok(r) if r.ok() => {
                    busy.set(false);
                    info.set(Some("Saved & restarted bananas-stats.".into()));
                }
                Ok(r) => {
                    let status = r.status();
                    let txt = r.text().await.unwrap_or_default();
                    busy.set(false);
                    let msg = serde_json::from_str::<serde_json::Value>(&txt)
                        .ok()
                        .and_then(|v| v.get("error").and_then(|e| e.as_str().map(String::from)))
                        .unwrap_or_else(|| format!("HTTP {status}"));
                    error.set(Some(msg));
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(format!("Network error: {e}")));
                }
            }
        });
    };

    rsx! {
        form {
            class: "settings-form",
            onsubmit: move |e| { e.prevent_default(); submit(()); },

            p { class: "preview-label",
                "Edits land in "
                code { "/etc/bananas/stats.toml" }
                " and atomically restart "
                code { "bananas-stats.service" }
                ". Hover any label for a tooltip."
            }

            if let Some(msg) = error() {
                div { class: "banner err", pre { "{msg}" } }
            }
            if let Some(msg) = info() {
                div { class: "banner ok", pre { "{msg}" } }
            }

            if !hydrated() && !load_failed() {
                p { class: "preview-label", "Loading…" }
            }

            fieldset {
                legend { "Sampling" }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "How often the sampler reads /proc + /sys (in milliseconds). 1000 = once per second.",
                        "Interval (ms)" }
                    input { r#type: "number", min: "100", step: "100", required: true,
                        value: "{interval_ms()}",
                        oninput: move |e| interval_ms.set(e.value()) }
                    span {}
                }
            }

            fieldset {
                legend { "Storage" }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Path to the WAL-mode SQLite database. The bananas user owns this directory via systemd's StateDirectory.",
                        "Database path" }
                    input { r#type: "text", required: true,
                        value: "{storage_path()}",
                        oninput: move |e| storage_path.set(e.value()) }
                    span {}
                }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Hours of raw 1-second samples kept before they're aggregated into the longer-window tables.",
                        "Raw retention (h)" }
                    input { r#type: "number", min: "1", required: true,
                        value: "{raw_hours()}",
                        oninput: move |e| raw_hours.set(e.value()) }
                    span {}
                }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Days the per-minute aggregated rows are kept. Older data is dropped.",
                        "Aggregated retention (d)" }
                    input { r#type: "number", min: "1", required: true,
                        value: "{agg_days()}",
                        oninput: move |e| agg_days.set(e.value()) }
                    span {}
                }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "How often the writer fsyncs the WAL. Lower = fewer rows lost on power cut, higher = fewer disk writes.",
                        "Flush interval (ms)" }
                    input { r#type: "number", min: "100", step: "100", required: true,
                        value: "{flush_ms()}",
                        oninput: move |e| flush_ms.set(e.value()) }
                    span {}
                }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Hard ceiling on the SQLite file (in MB). Empty = auto: min(5% free space, 100 MB) at first run.",
                        "Max DB size (MB)" }
                    input { r#type: "number", min: "10",
                        placeholder: "auto",
                        value: "{max_db_mb()}",
                        oninput: move |e| max_db_mb.set(e.value()) }
                    span {}
                }
            }

            fieldset {
                legend { "Devices" }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Comma-separated list of block-device buses to track. Add 'nvme' if the board ever ships an NVMe drive.",
                        "Include buses" }
                    input { r#type: "text",
                        placeholder: "sata, usb, nvme",
                        value: "{devices_buses()}",
                        oninput: move |e| devices_buses.set(e.value()) }
                    span {}
                }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Comma-separated exact device names to skip (e.g. 'sda1' to drop a partition).",
                        "Exclude names" }
                    input { r#type: "text",
                        placeholder: "(none)",
                        value: "{devices_excl()}",
                        oninput: move |e| devices_excl.set(e.value()) }
                    span {}
                }
            }

            fieldset {
                legend { "Network" }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Comma-separated interface name patterns. '*' matches anything; 'eth*' matches kernel-classic names; 'en*' matches systemd predictable names.",
                        "Include patterns" }
                    input { r#type: "text",
                        placeholder: "eth*, en*",
                        value: "{net_include()}",
                        oninput: move |e| net_include.set(e.value()) }
                    span {}
                }
                div { class: "row",
                    label { class: "hint",
                        "data-tip": "Comma-separated exact interface names to skip (e.g. 'docker0' if patterns are too broad).",
                        "Exclude names" }
                    input { r#type: "text",
                        placeholder: "(none)",
                        value: "{net_exclude()}",
                        oninput: move |e| net_exclude.set(e.value()) }
                    span {}
                }
            }

            div { class: "settings-actions",
                button { class: "primary", r#type: "submit",
                    disabled: busy() || !hydrated(),
                    if busy() { "Saving…" } else { "Save & restart service" }
                }
            }

            h4 { style: "margin: 1.2em 0 .4em",
                "Generated TOML"
            }
            p { class: "preview-label",
                "Read-only — exactly what we'll write to /etc/bananas/stats.toml on save. "
                "Copy the block if you'd rather edit it by hand on the device."
            }
            TextareaWithCopy { value: preview, id: "stats-config-toml" }
        }
    }
}
