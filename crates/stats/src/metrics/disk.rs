//! Per-device disk I/O sampling from `/proc/diskstats`.
//!
//! Format is documented in `Documentation/admin-guide/iostats.rst`. The fields
//! we care about (1-indexed, post-major+minor+name):
//!   1: reads completed
//!   3: sectors read       (each sector is 512 bytes — kernel constant)
//!   5: writes completed
//!   7: sectors written
//!   9: I/Os in flight
//!  10: time spent doing I/Os (ms)  ← used for util_pct

use super::devices::DeviceFilter;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const SECTOR_BYTES: u64 = 512;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskIo {
    pub device: String,
    pub bus: String,
    pub read_bps: u64,
    pub write_bps: u64,
    pub read_iops: u64,
    pub write_iops: u64,
    pub util_pct: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Counters {
    pub per_dev: HashMap<String, RawCounters>,
}

#[derive(Debug, Clone, Default, Copy)]
pub struct RawCounters {
    pub reads: u64,
    pub sectors_read: u64,
    pub writes: u64,
    pub sectors_written: u64,
    pub io_ms: u64,
}

pub fn sample(
    filter: &DeviceFilter,
    prev: &Counters,
    interval_ms: u64,
) -> Result<(Vec<DiskIo>, Counters)> {
    let raw = read_diskstats()?;
    Ok(compute(&raw, prev, filter, interval_ms))
}

#[cfg(target_os = "linux")]
fn read_diskstats() -> Result<String> {
    Ok(std::fs::read_to_string("/proc/diskstats")?)
}

#[cfg(not(target_os = "linux"))]
fn read_diskstats() -> Result<String> {
    Ok(String::new())
}

/// Parse `/proc/diskstats` text into `Counters`. Public for testing.
pub fn parse_diskstats(text: &str) -> Counters {
    let mut out = Counters::default();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        // major minor name reads reads_merged sectors_read time_reading
        // writes writes_merged sectors_written time_writing in_flight io_ms ...
        let _major = fields.next();
        let _minor = fields.next();
        let name = match fields.next() {
            Some(n) => n.to_string(),
            None => continue,
        };
        let nums: Vec<u64> = fields.filter_map(|f| f.parse().ok()).collect();
        if nums.len() < 10 {
            continue;
        }
        out.per_dev.insert(
            name,
            RawCounters {
                reads: nums[0],
                sectors_read: nums[2],
                writes: nums[4],
                sectors_written: nums[6],
                io_ms: nums[9],
            },
        );
    }
    out
}

/// Diff two snapshots and produce per-device rates. Pure function for testing.
pub fn compute(
    text_or_counters: &str,
    prev: &Counters,
    filter: &DeviceFilter,
    interval_ms: u64,
) -> (Vec<DiskIo>, Counters) {
    let now = parse_diskstats(text_or_counters);
    let dt_s = (interval_ms as f32 / 1000.0).max(0.001);

    let mut rows = Vec::new();
    for (name, cur) in &now.per_dev {
        if !filter.contains(name) {
            continue;
        }
        let prev = prev.per_dev.get(name).copied().unwrap_or_default();
        let d_reads = cur.reads.saturating_sub(prev.reads);
        let d_writes = cur.writes.saturating_sub(prev.writes);
        let d_sread = cur.sectors_read.saturating_sub(prev.sectors_read);
        let d_swrit = cur.sectors_written.saturating_sub(prev.sectors_written);
        let d_io_ms = cur.io_ms.saturating_sub(prev.io_ms);

        let read_bps = (d_sread * SECTOR_BYTES) as f32 / dt_s;
        let write_bps = (d_swrit * SECTOR_BYTES) as f32 / dt_s;
        let read_iops = d_reads as f32 / dt_s;
        let write_iops = d_writes as f32 / dt_s;
        // util% is busy-time as fraction of wall-clock, capped at 100.
        let util_pct = ((d_io_ms as f32) / (interval_ms as f32) * 100.0).min(100.0);

        rows.push(DiskIo {
            device: name.clone(),
            bus: filter.bus_of(name).as_str().to_string(),
            read_bps: read_bps as u64,
            write_bps: write_bps as u64,
            read_iops: read_iops as u64,
            write_iops: write_iops as u64,
            util_pct,
        });
    }
    rows.sort_by(|a, b| a.device.cmp(&b.device));
    (rows, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Devices;
    use crate::metrics::devices::Bus;

    const T1: &str = "\
   8       0 sda 1000 0 8000 50 200 0 1600 30 0 100 80
   8      16 sdb 500 0 4000 25 100 0 800 15 0 50 40
 179       0 mmcblk0 100 0 800 5 50 0 400 3 0 10 8
";
    const T2: &str = "\
   8       0 sda 1500 0 12000 75 400 0 3200 60 0 200 160
   8      16 sdb 600 0 4800 30 150 0 1200 22 0 75 60
 179       0 mmcblk0 110 0 880 6 55 0 440 3 0 11 9
";

    fn filter_sata_usb() -> DeviceFilter {
        use crate::metrics::devices::{BlockDevice, build_filter};
        let devs = vec![
            BlockDevice {
                name: "sda".into(),
                bus: Bus::Sata,
            },
            BlockDevice {
                name: "sdb".into(),
                bus: Bus::Usb,
            },
            BlockDevice {
                name: "mmcblk0".into(),
                bus: Bus::Mmc,
            },
        ];
        build_filter(
            &devs,
            &Devices {
                include_buses: vec!["sata".into(), "usb".into()],
                exclude_names: vec![],
            },
        )
    }

    #[test]
    fn skips_mmc_device() {
        let prev = parse_diskstats(T1);
        let (rows, _) = compute(T2, &prev, &filter_sata_usb(), 1000);
        let names: Vec<_> = rows.iter().map(|r| r.device.as_str()).collect();
        assert_eq!(names, vec!["sda", "sdb"]);
    }

    #[test]
    fn computes_throughput_in_bytes_per_sec() {
        let prev = parse_diskstats(T1);
        let (rows, _) = compute(T2, &prev, &filter_sata_usb(), 1000);
        let sda = rows.iter().find(|r| r.device == "sda").unwrap();
        // sda: sectors_read 8000 -> 12000 in 1s = 4000 sectors * 512 = 2 048 000 B/s
        assert_eq!(sda.read_bps, 2_048_000);
        // writes: 1600 -> 3200 in 1s = 1600 * 512 = 819 200
        assert_eq!(sda.write_bps, 819_200);
        // read IOPS = 500 in 1s
        assert_eq!(sda.read_iops, 500);
        assert_eq!(sda.write_iops, 200);
    }

    #[test]
    fn util_pct_caps_at_100() {
        // io_ms delta 200 over 100ms interval -> would be 200% un-capped.
        let prev_text = "8 0 sda 0 0 0 0 0 0 0 0 0 0 0\n";
        let cur_text = "8 0 sda 0 0 0 0 0 0 0 0 0 200 0\n";
        let prev = parse_diskstats(prev_text);
        let (rows, _) = compute(cur_text, &prev, &filter_sata_usb(), 100);
        let sda = rows.iter().find(|r| r.device == "sda").unwrap();
        assert_eq!(sda.util_pct, 100.0);
    }

    #[test]
    fn handles_counter_wrap_with_saturating_sub() {
        // 32-bit kernels can wrap counters; saturating_sub keeps us at 0.
        let prev_text = "8 0 sda 100 0 100 0 100 0 100 0 0 100 0\n";
        let cur_text = "8 0 sda  50 0  50 0  50 0  50 0 0  50 0\n";
        let prev = parse_diskstats(prev_text);
        let (rows, _) = compute(cur_text, &prev, &filter_sata_usb(), 1000);
        let sda = &rows[0];
        assert_eq!(sda.read_bps, 0);
        assert_eq!(sda.write_bps, 0);
    }
}
