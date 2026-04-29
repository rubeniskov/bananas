//! Temperature readings from /sys.
//!
//! Two sources, both surface in `Snapshot.temps` as a flat list:
//!
//! - **CPU** (and any other `thermal_zone*` the kernel exposes):
//!   `/sys/class/thermal/thermal_zoneN/{type,temp}`. The `temp` file is
//!   in millidegrees Celsius. We use the zone's `type` as the sensor
//!   name, falling back to `zone<N>` if it's empty.
//!
//! - **Block devices** (SATA/NVMe/MMC) via the kernel `drivetemp`
//!   driver: each disk shows up at
//!   `/sys/block/<dev>/device/hwmon/hwmonN/temp1_input` (also
//!   millidegrees). drivetemp reads SMART attribute 194 in-kernel, so
//!   the sampler runs at 1 Hz without forking smartctl. CONFIG_DRIVETEMP
//!   must be enabled in the kernel; we ship that fragment in
//!   recipes-kernel/linux/files/.
//!
//! Failures (missing files, unparseable values) are logged at debug
//! level and skipped — temp data is best-effort, never fatal.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TempReading {
    /// Sensor identifier — `cpu_thermal`, `sda`, `nvme0n1`, …
    pub sensor: String,
    pub celsius: f32,
}

#[cfg(not(target_os = "linux"))]
pub fn sample() -> Vec<TempReading> {
    Vec::new()
}

#[cfg(target_os = "linux")]
pub fn sample() -> Vec<TempReading> {
    let mut out = Vec::new();
    sample_thermal_zones(&mut out);
    sample_block_hwmon(&mut out);
    out
}

#[cfg(target_os = "linux")]
fn sample_thermal_zones(out: &mut Vec<TempReading>) {
    let dir = match std::fs::read_dir("/sys/class/thermal") {
        Ok(d) => d,
        Err(_) => return,
    };
    for entry in dir.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.starts_with("thermal_zone") {
            continue;
        }
        let typ = std::fs::read_to_string(path.join("type"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| name.replacen("thermal_", "", 1));
        let Ok(s) = std::fs::read_to_string(path.join("temp")) else { continue };
        if let Some(milli) = s.trim().parse::<i32>().ok() {
            out.push(TempReading { sensor: typ, celsius: milli as f32 / 1000.0 });
        }
    }
}

#[cfg(target_os = "linux")]
fn sample_block_hwmon(out: &mut Vec<TempReading>) {
    let blocks = match std::fs::read_dir("/sys/block") {
        Ok(d) => d,
        Err(_) => return,
    };
    for blk in blocks.flatten() {
        let bname = blk.file_name();
        let bname_s = bname.to_string_lossy().to_string();
        // Only physical media the kernel might attach drivetemp to.
        if !(bname_s.starts_with("sd")
            || bname_s.starts_with("nvme")
            || bname_s.starts_with("mmcblk"))
        {
            continue;
        }
        let hwmon_dir = blk.path().join("device").join("hwmon");
        let Ok(entries) = std::fs::read_dir(&hwmon_dir) else { continue };
        for hw in entries.flatten() {
            let temp_path = hw.path().join("temp1_input");
            let Ok(s) = std::fs::read_to_string(&temp_path) else { continue };
            if let Ok(milli) = s.trim().parse::<i32>() {
                out.push(TempReading { sensor: bname_s.clone(), celsius: milli as f32 / 1000.0 });
                break; // first hwmon per block dev is enough
            }
        }
    }
}
