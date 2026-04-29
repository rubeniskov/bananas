//! Aggregate CPU sampling from `/proc/stat`.
//!
//! Format (from `man 5 proc`):
//!   cpu  user nice system idle iowait irq softirq steal guest guest_nice
//! All values are jiffies. Busy = total − idle − iowait.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CpuStats {
    pub busy_pct: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Counters {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl Counters {
    pub fn total(self) -> u64 {
        self.user + self.nice + self.system + self.idle
            + self.iowait + self.irq + self.softirq + self.steal
    }
    pub fn busy(self) -> u64 {
        self.user + self.nice + self.system + self.irq + self.softirq + self.steal
    }
}

pub fn sample(prev: &Counters) -> Result<(CpuStats, Counters)> {
    let raw = read_proc_stat()?;
    Ok(compute(&raw, prev))
}

#[cfg(target_os = "linux")]
fn read_proc_stat() -> Result<String> {
    Ok(std::fs::read_to_string("/proc/stat")?)
}

#[cfg(not(target_os = "linux"))]
fn read_proc_stat() -> Result<String> {
    Ok(String::new())
}

pub fn parse(text: &str) -> Counters {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("cpu ") {
            let nums: Vec<u64> = rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            if nums.len() < 4 {
                return Counters::default();
            }
            return Counters {
                user: nums[0],
                nice: nums[1],
                system: nums[2],
                idle: nums[3],
                iowait: nums.get(4).copied().unwrap_or(0),
                irq: nums.get(5).copied().unwrap_or(0),
                softirq: nums.get(6).copied().unwrap_or(0),
                steal: nums.get(7).copied().unwrap_or(0),
            };
        }
    }
    Counters::default()
}

pub fn compute(text: &str, prev: &Counters) -> (CpuStats, Counters) {
    let cur = parse(text);
    let dt_total = cur.total().saturating_sub(prev.total());
    let dt_busy = cur.busy().saturating_sub(prev.busy());
    let pct = if dt_total == 0 {
        0.0
    } else {
        (dt_busy as f32 / dt_total as f32) * 100.0
    };
    (CpuStats { busy_pct: pct.clamp(0.0, 100.0) }, cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: &str = "\
cpu  100 0 50 1000 10 0 5 0 0 0
cpu0 50 0 25 500 5 0 2 0 0 0
cpu1 50 0 25 500 5 0 3 0 0 0
intr 12345
";
    const T2: &str = "\
cpu  300 0 100 1500 20 0 10 0 0 0
cpu0 150 0 50 750 10 0 5 0 0 0
cpu1 150 0 50 750 10 0 5 0 0 0
intr 22345
";

    #[test]
    fn parses_cpu_aggregate_line() {
        let c = parse(T1);
        assert_eq!(c.user, 100);
        assert_eq!(c.idle, 1000);
        assert_eq!(c.iowait, 10);
    }

    #[test]
    fn computes_busy_percent() {
        let prev = parse(T1);
        let (stats, _) = compute(T2, &prev);
        // delta busy = (300-100) + (100-50) + (10-5) = 255
        // delta total = (300+0+100+1500+20+0+10) - (100+0+50+1000+10+0+5)
        //             = 1930 - 1165 = 765
        // pct = 255/765 ≈ 33.33
        assert!((stats.busy_pct - 33.33).abs() < 0.5);
    }

    #[test]
    fn returns_zero_when_no_progress() {
        let prev = parse(T1);
        let (stats, _) = compute(T1, &prev);
        assert_eq!(stats.busy_pct, 0.0);
    }
}
