//! Block-device classification.
//!
//! On a Banana Pi BPI-M1+ we want to monitor SATA + USB drives but skip the
//! eMMC/SD card the OS boots from. Classification is done by reading
//! `/sys/block/<dev>` and inspecting the canonical path:
//!
//!   /sys/devices/.../usb1/.../block/sdX        → USB
//!   /sys/devices/.../ahci.0/ata1/.../sdX       → SATA
//!   /sys/devices/.../mmc_host/mmc0/.../mmcblk0 → MMC (skipped)

use crate::config::Devices;
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bus {
    Sata,
    Usb,
    Mmc,
    Unknown,
}

impl Bus {
    pub fn as_str(self) -> &'static str {
        match self {
            Bus::Sata => "sata",
            Bus::Usb => "usb",
            Bus::Mmc => "mmc",
            Bus::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlockDevice {
    pub name: String, // e.g. "sda"
    pub bus: Bus,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceFilter {
    /// Names of disks (e.g. "sda", "sdb") that pass the filter.
    allowed: HashSet<String>,
    /// Bus per allowed name.
    bus_of: std::collections::HashMap<String, Bus>,
    /// True if filtering is disabled (used as a fallback).
    pass_all: bool,
}

impl DeviceFilter {
    pub fn allow_all() -> Self {
        Self {
            pass_all: true,
            ..Self::default()
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.pass_all || self.allowed.contains(name)
    }

    pub fn bus_of(&self, name: &str) -> Bus {
        if self.pass_all {
            return Bus::Unknown;
        }
        self.bus_of.get(name).copied().unwrap_or(Bus::Unknown)
    }

    pub fn allowed_names(&self) -> impl Iterator<Item = &str> {
        self.allowed.iter().map(String::as_str)
    }
}

/// Classify every entry under `/sys/block`, then apply `cfg.include_buses` and
/// `cfg.exclude_names` to build the final filter.
pub fn classify_all(cfg: &Devices) -> Result<DeviceFilter> {
    let devs = enumerate_block_devices(std::path::Path::new("/sys/block"))?;
    Ok(build_filter(&devs, cfg))
}

pub fn build_filter(devs: &[BlockDevice], cfg: &Devices) -> DeviceFilter {
    let include: HashSet<&str> = cfg.include_buses.iter().map(String::as_str).collect();
    let exclude: HashSet<&str> = cfg.exclude_names.iter().map(String::as_str).collect();

    let mut f = DeviceFilter::default();
    for d in devs {
        if exclude.contains(d.name.as_str()) {
            continue;
        }
        if include.contains(d.bus.as_str()) {
            f.allowed.insert(d.name.clone());
            f.bus_of.insert(d.name.clone(), d.bus);
        }
    }
    f
}

#[cfg(target_os = "linux")]
fn enumerate_block_devices(root: &std::path::Path) -> Result<Vec<BlockDevice>> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // `/sys/block/<name>` is a symlink into /sys/devices/...; canonicalize
        // to get the bus-bearing path.
        let canonical = std::fs::canonicalize(entry.path()).unwrap_or_else(|_| entry.path());
        let bus = classify_path(&canonical, &name);
        out.push(BlockDevice { name, bus });
    }
    Ok(out)
}

#[cfg(not(target_os = "linux"))]
fn enumerate_block_devices(_root: &std::path::Path) -> Result<Vec<BlockDevice>> {
    // Off-Linux dev hosts: nothing to enumerate. Sampler will fall back to
    // an allow-all filter, which combined with no /proc/diskstats means
    // empty disk vectors — fine for compile checks.
    Ok(Vec::new())
}

/// Classify a canonical sysfs path. Public for testing.
pub fn classify_path(path: &Path, name: &str) -> Bus {
    let s = path.to_string_lossy();
    if name.starts_with("mmcblk") || s.contains("/mmc_host/") {
        Bus::Mmc
    } else if s.contains("/usb") {
        Bus::Usb
    } else if s.contains("/ata") || s.contains("/ahci") || s.contains("/sata") {
        Bus::Sata
    } else {
        Bus::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn classifies_mmc_by_name() {
        let p = PathBuf::from(
            "/sys/devices/platform/soc/1c0f000.mmc/mmc_host/mmc0/mmc0:0001/block/mmcblk0",
        );
        assert_eq!(classify_path(&p, "mmcblk0"), Bus::Mmc);
    }

    #[test]
    fn classifies_usb_drive() {
        let p = PathBuf::from(
            "/sys/devices/platform/soc/1c1a000.usb/usb1/1-1/1-1:1.0/host3/target3:0:0/3:0:0:0/block/sdb",
        );
        assert_eq!(classify_path(&p, "sdb"), Bus::Usb);
    }

    #[test]
    fn classifies_sata_drive() {
        let p = PathBuf::from(
            "/sys/devices/platform/soc/1c18000.sata/ata1/host0/target0:0:0/0:0:0:0/block/sda",
        );
        assert_eq!(classify_path(&p, "sda"), Bus::Sata);
    }

    #[test]
    fn filter_keeps_only_included_buses() {
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
        let cfg = Devices {
            include_buses: vec!["sata".into(), "usb".into()],
            exclude_names: vec![],
        };
        let f = build_filter(&devs, &cfg);
        assert!(f.contains("sda"));
        assert!(f.contains("sdb"));
        assert!(!f.contains("mmcblk0"));
        assert_eq!(f.bus_of("sda"), Bus::Sata);
        assert_eq!(f.bus_of("sdb"), Bus::Usb);
    }

    #[test]
    fn exclude_names_overrides_include() {
        let devs = vec![BlockDevice {
            name: "sda".into(),
            bus: Bus::Sata,
        }];
        let cfg = Devices {
            include_buses: vec!["sata".into()],
            exclude_names: vec!["sda".into()],
        };
        let f = build_filter(&devs, &cfg);
        assert!(!f.contains("sda"));
    }
}
