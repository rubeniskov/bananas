//! Memory sampling from `/proc/meminfo`.
//!
//! "Used" follows the modern correct definition: `MemTotal − MemAvailable`.
//! `MemAvailable` (kernel ≥ 3.14) accounts for reclaimable cache, so it
//! matches what `htop`/`free -h` show as "used" today.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemStats {
    pub total: u64,      // bytes
    pub used: u64,
    pub available: u64,
    pub free: u64,
}

pub fn sample() -> Result<MemStats> {
    let raw = read_proc_meminfo()?;
    Ok(parse(&raw))
}

#[cfg(target_os = "linux")]
fn read_proc_meminfo() -> Result<String> {
    Ok(std::fs::read_to_string("/proc/meminfo")?)
}

#[cfg(not(target_os = "linux"))]
fn read_proc_meminfo() -> Result<String> {
    Ok(String::new())
}

pub fn parse(text: &str) -> MemStats {
    let mut total = 0u64;
    let mut available = 0u64;
    let mut free = 0u64;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemFree:") {
            free = parse_kb(rest);
        }
    }
    // On older kernels MemAvailable is missing; fall back to MemFree.
    let effective_avail = if available == 0 { free } else { available };
    let used = total.saturating_sub(effective_avail);
    MemStats { total, used, available: effective_avail, free }
}

fn parse_kb(line: &str) -> u64 {
    line.split_whitespace()
        .find_map(|t| t.parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
MemTotal:        1015464 kB
MemFree:          234688 kB
MemAvailable:     567816 kB
Buffers:           23456 kB
Cached:           187320 kB
SwapCached:            0 kB
Active:           500000 kB
Inactive:         300000 kB
";

    #[test]
    fn parses_used_via_memavailable() {
        let m = parse(SAMPLE);
        assert_eq!(m.total, 1015464 * 1024);
        assert_eq!(m.available, 567816 * 1024);
        // used = total - available
        assert_eq!(m.used, (1015464 - 567816) * 1024);
    }

    #[test]
    fn falls_back_to_memfree_on_old_kernels() {
        let no_avail = "MemTotal: 1000 kB\nMemFree: 200 kB\n";
        let m = parse(no_avail);
        assert_eq!(m.total, 1000 * 1024);
        assert_eq!(m.available, 200 * 1024);
        assert_eq!(m.used, 800 * 1024);
    }

    #[test]
    fn empty_input_yields_zeros() {
        let m = parse("");
        assert_eq!(m.total, 0);
        assert_eq!(m.used, 0);
    }
}
