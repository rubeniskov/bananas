//! Per-interface network sampling from `/proc/net/dev`.

use crate::config::Network as NetCfg;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetIface {
    pub name: String,
    pub rx_bps: u64,
    pub tx_bps: u64,
    pub rx_total: u64,
    pub tx_total: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Counters {
    pub per_iface: HashMap<String, RawCounters>,
}

#[derive(Debug, Clone, Default, Copy)]
pub struct RawCounters {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Filter applied to interface names. Built from `Network` config.
#[derive(Debug, Clone, Default)]
pub struct NetFilter {
    include: Vec<String>,
    exclude: HashSet<String>,
}

impl NetFilter {
    pub fn from_config(cfg: &NetCfg) -> Self {
        Self {
            include: cfg.include_patterns.clone(),
            exclude: cfg.exclude_names.iter().cloned().collect(),
        }
    }

    /// Allow-all filter (used as a fallback if config parsing fails).
    pub fn allow_all() -> Self {
        Self {
            include: vec!["*".into()],
            exclude: HashSet::new(),
        }
    }

    pub fn allows(&self, name: &str) -> bool {
        if self.exclude.contains(name) {
            return false;
        }
        self.include.iter().any(|p| matches_pattern(p, name))
    }
}

/// Tiny glob: `*` matches anything, `prefix*` matches by prefix, anything
/// else is an exact-name match. Sufficient for interface naming, no need
/// for a full glob crate.
fn matches_pattern(pat: &str, name: &str) -> bool {
    if pat == "*" {
        return true;
    }
    if let Some(prefix) = pat.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    pat == name
}

pub fn sample(
    filter: &NetFilter,
    prev: &Counters,
    interval_ms: u64,
) -> Result<(Vec<NetIface>, Counters)> {
    let raw = read_proc_net_dev()?;
    Ok(compute(&raw, filter, prev, interval_ms))
}

#[cfg(target_os = "linux")]
fn read_proc_net_dev() -> Result<String> {
    Ok(std::fs::read_to_string("/proc/net/dev")?)
}

#[cfg(not(target_os = "linux"))]
fn read_proc_net_dev() -> Result<String> {
    Ok(String::new())
}

/// Parse `/proc/net/dev`. Public for testing.
pub fn parse(text: &str) -> Counters {
    let mut out = Counters::default();
    // First two lines are headers.
    for line in text.lines().skip(2) {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        if name == "lo" {
            continue;
        }
        let nums: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|f| f.parse().ok())
            .collect();
        // Receive bytes is field 0, transmit bytes is field 8.
        if nums.len() < 9 {
            continue;
        }
        out.per_iface.insert(
            name,
            RawCounters {
                rx_bytes: nums[0],
                tx_bytes: nums[8],
            },
        );
    }
    out
}

pub fn compute(
    text: &str,
    filter: &NetFilter,
    prev: &Counters,
    interval_ms: u64,
) -> (Vec<NetIface>, Counters) {
    let now = parse(text);
    let dt_s = (interval_ms as f32 / 1000.0).max(0.001);

    let mut rows = Vec::new();
    for (name, cur) in &now.per_iface {
        if !filter.allows(name) {
            continue;
        }
        let prev = prev.per_iface.get(name).copied().unwrap_or_default();
        let d_rx = cur.rx_bytes.saturating_sub(prev.rx_bytes);
        let d_tx = cur.tx_bytes.saturating_sub(prev.tx_bytes);
        rows.push(NetIface {
            name: name.clone(),
            rx_bps: (d_rx as f32 / dt_s) as u64,
            tx_bps: (d_tx as f32 / dt_s) as u64,
            rx_total: cur.rx_bytes,
            tx_total: cur.tx_bytes,
        });
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    (rows, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  100      1    0    0    0     0          0         0        100      1    0    0    0     0       0          0
  eth0:  1000   10    0    0    0     0          0         0       2000     20    0    0    0     0       0          0
 wlan0:   500    5    0    0    0     0          0         0        300      3    0    0    0     0       0          0
";
    const T2: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  200      2    0    0    0     0          0         0        200      2    0    0    0     0       0          0
  eth0:  3000   30    0    0    0     0          0         0       4000     40    0    0    0     0       0          0
 wlan0:   500    5    0    0    0     0          0         0        300      3    0    0    0     0       0          0
";

    fn allow_all() -> NetFilter {
        NetFilter::allow_all()
    }

    #[test]
    fn skips_loopback() {
        let c = parse(T1);
        assert!(!c.per_iface.contains_key("lo"));
        assert!(c.per_iface.contains_key("eth0"));
        assert!(c.per_iface.contains_key("wlan0"));
    }

    #[test]
    fn computes_per_interface_rates() {
        let prev = parse(T1);
        let (rows, _) = compute(T2, &allow_all(), &prev, 1000);
        let eth = rows.iter().find(|r| r.name == "eth0").unwrap();
        // rx 1000 -> 3000 in 1s = 2000 B/s. tx 2000 -> 4000 = 2000 B/s.
        assert_eq!(eth.rx_bps, 2000);
        assert_eq!(eth.tx_bps, 2000);

        let wlan = rows.iter().find(|r| r.name == "wlan0").unwrap();
        assert_eq!(wlan.rx_bps, 0);
        assert_eq!(wlan.tx_bps, 0);
    }

    #[test]
    fn matches_pattern_handles_wildcard_prefix_and_exact() {
        assert!(matches_pattern("*", "anything"));
        assert!(matches_pattern("eth*", "eth0"));
        assert!(matches_pattern("eth*", "ethX"));
        assert!(!matches_pattern("eth*", "wlan0"));
        assert!(matches_pattern("eth0", "eth0"));
        assert!(!matches_pattern("eth0", "eth1"));
    }

    #[test]
    fn default_filter_keeps_ethernet_excludes_wireless() {
        let f = NetFilter::from_config(&crate::config::Network::default());
        assert!(f.allows("eth0"));
        assert!(f.allows("enp0s3"));
        assert!(!f.allows("wlan0"));
        assert!(!f.allows("docker0"));
        assert!(!f.allows("br-abc123"));
    }

    #[test]
    fn exclude_names_overrides_include() {
        let cfg = crate::config::Network {
            include_patterns: vec!["eth*".into()],
            exclude_names: vec!["eth0".into()],
        };
        let f = NetFilter::from_config(&cfg);
        assert!(!f.allows("eth0"));
        assert!(f.allows("eth1"));
    }

    #[test]
    fn compute_drops_filtered_interfaces() {
        let cfg = crate::config::Network::default();
        let f = NetFilter::from_config(&cfg);
        let prev = parse(T1);
        let (rows, _) = compute(T2, &f, &prev, 1000);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["eth0"]);
    }
}
