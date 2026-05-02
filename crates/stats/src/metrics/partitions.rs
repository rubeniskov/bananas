//! Per-partition capacity from `/proc/mounts` + `statvfs(2)`.
//!
//! We list every entry in `/proc/mounts`, drop pseudo-filesystems, and call
//! `statvfs` to compute total/used/free. The result is filtered to partitions
//! that live on a device the user wants to track (SATA + USB by default).

use super::devices::DeviceFilter;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Partition {
    pub device: String, // "/dev/sda1"
    pub parent: String, // "sda"
    pub mount: String,  // "/mnt/data"
    pub fs: String,     // "ext4"
    pub total: u64,
    pub used: u64,
    pub free: u64,
}

pub fn sample(filter: &DeviceFilter) -> Result<Vec<Partition>> {
    let mounts = read_mounts()?;
    let entries = parse_mounts(&mounts);
    let mut out = Vec::new();
    for e in entries {
        let parent = parent_of(&e.device);
        if !is_real_device(&e.device) || !filter.contains(&parent) {
            continue;
        }
        let (total, free) = match statvfs_bytes(&e.mount) {
            Ok(t) => t,
            Err(err) => {
                tracing::debug!(mount = %e.mount, error = ?err, "statvfs failed");
                continue;
            }
        };
        let used = total.saturating_sub(free);
        out.push(Partition {
            device: e.device,
            parent,
            mount: e.mount,
            fs: e.fs,
            total,
            used,
            free,
        });
    }
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    Ok(out)
}

#[cfg(target_os = "linux")]
fn read_mounts() -> Result<String> {
    Ok(std::fs::read_to_string("/proc/mounts")?)
}

#[cfg(not(target_os = "linux"))]
fn read_mounts() -> Result<String> {
    Ok(String::new())
}

#[cfg(target_os = "linux")]
fn statvfs_bytes(mount: &str) -> Result<(u64, u64)> {
    use nix::sys::statvfs::statvfs;
    use std::path::Path;
    let st = statvfs(Path::new(mount))?;
    let bsize = st.fragment_size() as u64;
    let total = st.blocks() as u64 * bsize;
    let free = st.blocks_available() as u64 * bsize;
    Ok((total, free))
}

#[cfg(not(target_os = "linux"))]
fn statvfs_bytes(_mount: &str) -> Result<(u64, u64)> {
    Ok((0, 0))
}

#[derive(Debug, Clone)]
pub struct MountEntry {
    pub device: String,
    pub mount: String,
    pub fs: String,
}

pub fn parse_mounts(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let device = fields.next()?.to_string();
            let mount = unescape_mount(fields.next()?);
            let fs = fields.next()?.to_string();
            Some(MountEntry { device, mount, fs })
        })
        .collect()
}

/// Mount paths in /proc/mounts use octal escapes for spaces (\040), tabs
/// (\011), backslashes (\134), etc. Decode the common ones.
fn unescape_mount(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Ok(n) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 4]).unwrap_or("000"),
                8,
            ) {
                out.push(n as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn is_real_device(dev: &str) -> bool {
    dev.starts_with("/dev/")
        && !dev.starts_with("/dev/loop")
        && !dev.starts_with("/dev/ram")
        && !dev.starts_with("/dev/zram")
}

/// "/dev/sda1" -> "sda", "/dev/mmcblk0p1" -> "mmcblk0", "/dev/nvme0n1p1" -> "nvme0n1".
pub fn parent_of(dev: &str) -> String {
    let name = dev.strip_prefix("/dev/").unwrap_or(dev);
    // Strip trailing digits + optional 'p' separator (mmcblk0p1, nvme0n1p1).
    // For sdXY, just strip trailing digits.
    let bytes = name.as_bytes();
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1].is_ascii_digit() {
        end -= 1;
    }
    if end > 0 && bytes[end - 1] == b'p' && end >= 2 && bytes[end - 2].is_ascii_digit() {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_strips_partition_suffix() {
        assert_eq!(parent_of("/dev/sda1"), "sda");
        assert_eq!(parent_of("/dev/sdb"), "sdb");
        assert_eq!(parent_of("/dev/mmcblk0p1"), "mmcblk0");
        assert_eq!(parent_of("/dev/nvme0n1p3"), "nvme0n1");
    }

    #[test]
    fn unescape_decodes_octal() {
        assert_eq!(unescape_mount("/mnt/with\\040space"), "/mnt/with space");
        assert_eq!(unescape_mount("/no/escapes"), "/no/escapes");
    }

    #[test]
    fn parse_mounts_skips_pseudo_with_real_filter() {
        let text = "\
proc /proc proc rw,relatime 0 0
/dev/sda1 /mnt/data ext4 rw 0 0
tmpfs /run tmpfs rw 0 0
/dev/sdb1 /mnt/usb\\040drive exfat rw 0 0
";
        let entries = parse_mounts(text);
        assert_eq!(entries.len(), 4);
        let real: Vec<_> = entries
            .iter()
            .filter(|e| is_real_device(&e.device))
            .collect();
        assert_eq!(real.len(), 2);
        assert_eq!(real[1].mount, "/mnt/usb drive");
    }
}
