//! Sampling subsystem.
//!
//! `Sampler` owns the polling loop and publishes a `Snapshot` to a
//! `tokio::sync::watch` channel once per `interval_ms`. The UI subscribes for
//! live values, the storage writer subscribes to persist them.

pub mod cpu;
pub mod devices;
pub mod disk;
pub mod mem;
pub mod net;
pub mod partitions;
pub mod temp;

use crate::config::{Devices, Network, Sampling};
use serde::{Deserialize, Serialize};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::SystemTime;
use tokio::sync::watch;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub ts_unix: i64,
    pub cpu: cpu::CpuStats,
    pub mem: mem::MemStats,
    pub network: Vec<net::NetIface>,
    pub disks: Vec<disk::DiskIo>,
    pub parts: Vec<partitions::Partition>,
    /// CPU thermal-zone + drivetemp readings. Empty if neither
    /// `/sys/class/thermal` nor `/sys/block/*/device/hwmon` produces
    /// useful readings. Always serialized — clients can detect
    /// "no temps available" by an empty array.
    #[serde(default)]
    pub temps: Vec<temp::TempReading>,
}

pub struct Sampler {
    cfg: Sampling,
    /// Only the Linux real-sampler path actually consults `devices` for
    /// device-class filtering; on non-Linux dev hosts the field is dead.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    devices: Devices,
    network: Network,
    tx: watch::Sender<Snapshot>,
}

impl Sampler {
    pub fn new(
        cfg: Sampling,
        devices: Devices,
        network: Network,
        tx: watch::Sender<Snapshot>,
    ) -> Self {
        Self {
            cfg,
            devices,
            network,
            tx,
        }
    }

    /// On non-Linux dev hosts (macOS / Windows) the /proc-based
    /// samplers don't exist; we just emit empty Snapshots so the
    /// dashboard's UI smoke-test path still works.
    #[cfg(not(target_os = "linux"))]
    pub async fn run(self) {
        let mut tick = tokio::time::interval(Duration::from_millis(self.cfg.interval_ms.max(100)));
        loop {
            tick.tick().await;
            let _ = self.tx.send(Snapshot::default());
        }
    }

    #[cfg(target_os = "linux")]
    pub async fn run(self) {
        self.run_real().await;
    }

    #[cfg(target_os = "linux")]
    async fn run_real(self) {
        let mut tick = tokio::time::interval(Duration::from_millis(self.cfg.interval_ms.max(100)));
        let mut prev_disk = disk::Counters::default();
        let mut prev_net = net::Counters::default();
        let mut prev_cpu = cpu::Counters::default();

        let device_filter = match devices::classify_all(&self.devices) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(error = ?e, "failed to classify block devices; falling back to all");
                devices::DeviceFilter::allow_all()
            }
        };
        let net_filter = net::NetFilter::from_config(&self.network);

        loop {
            tick.tick().await;

            let now = SystemTime::now();
            let ts_unix = now
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            let (disks, new_disk) = disk::sample(&device_filter, &prev_disk, self.cfg.interval_ms)
                .unwrap_or_else(|e| {
                    tracing::warn!(error = ?e, "disk sample failed");
                    (vec![], prev_disk.clone())
                });
            prev_disk = new_disk;

            let (network, new_net) = net::sample(&net_filter, &prev_net, self.cfg.interval_ms)
                .unwrap_or_else(|e| {
                    tracing::warn!(error = ?e, "net sample failed");
                    (vec![], prev_net.clone())
                });
            prev_net = new_net;

            let parts = partitions::sample(&device_filter).unwrap_or_else(|e| {
                tracing::warn!(error = ?e, "partitions sample failed");
                vec![]
            });

            let (cpu_stats, new_cpu) = cpu::sample(&prev_cpu).unwrap_or_else(|e| {
                tracing::warn!(error = ?e, "cpu sample failed");
                (cpu::CpuStats::default(), prev_cpu)
            });
            prev_cpu = new_cpu;

            let mem_stats = mem::sample().unwrap_or_else(|e| {
                tracing::warn!(error = ?e, "mem sample failed");
                mem::MemStats::default()
            });

            let temps = temp::sample();

            let snap = Snapshot {
                ts_unix,
                cpu: cpu_stats,
                mem: mem_stats,
                network,
                disks,
                parts,
                temps,
            };
            let _ = self.tx.send(snap);
        }
    }
}
