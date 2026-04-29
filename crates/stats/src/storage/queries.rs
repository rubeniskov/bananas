//! Read-side helpers for the UI history view.

use super::Database;
use crate::metrics::{cpu::CpuStats, disk::DiskIo, mem::MemStats, net::NetIface, partitions::Partition, Snapshot};
use anyhow::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetSeriesPoint {
    pub ts: i64,
    pub rx_bps: u64,
    pub tx_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskSeriesPoint {
    pub ts: i64,
    pub read_bps: u64,
    pub write_bps: u64,
    pub util_pct: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempSeriesPoint {
    pub ts: i64,
    pub celsius: f32,
}

/// Return all rows in [from, to) for the given iface. Caller chooses table
/// (raw vs _1m) to control resolution.
pub fn net_range(
    db: &Database,
    iface: &str,
    from: i64,
    to: i64,
    table: &str,
) -> Result<Vec<NetSeriesPoint>> {
    let sql = format!(
        "SELECT ts, rx_bps, tx_bps FROM {table} WHERE iface = ?1 AND ts >= ?2 AND ts < ?3 ORDER BY ts"
    );
    db.with(|c| {
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt
            .query_map(params![iface, from, to], |r| {
                Ok(NetSeriesPoint {
                    ts: r.get(0)?,
                    rx_bps: r.get::<_, i64>(1)? as u64,
                    tx_bps: r.get::<_, i64>(2)? as u64,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

pub fn disk_range(
    db: &Database,
    device: &str,
    from: i64,
    to: i64,
    table: &str,
) -> Result<Vec<DiskSeriesPoint>> {
    let sql = format!(
        "SELECT ts, read_bps, write_bps, util_pct FROM {table} WHERE device = ?1 AND ts >= ?2 AND ts < ?3 ORDER BY ts"
    );
    db.with(|c| {
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt
            .query_map(params![device, from, to], |r| {
                Ok(DiskSeriesPoint {
                    ts: r.get(0)?,
                    read_bps: r.get::<_, i64>(1)? as u64,
                    write_bps: r.get::<_, i64>(2)? as u64,
                    util_pct: r.get::<_, f64>(3)? as f32,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

pub fn temp_range(
    db: &Database,
    sensor: &str,
    from: i64,
    to: i64,
    table: &str,
) -> Result<Vec<TempSeriesPoint>> {
    let sql = format!(
        "SELECT ts, celsius FROM {table} WHERE sensor = ?1 AND ts >= ?2 AND ts < ?3 ORDER BY ts"
    );
    db.with(|c| {
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt
            .query_map(params![sensor, from, to], |r| {
                Ok(TempSeriesPoint {
                    ts: r.get(0)?,
                    celsius: r.get::<_, f64>(1)? as f32,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Return the distinct iface names + disk device names + temperature
/// sensors that have ever been recorded. Used by the web admin's
/// /api/stats/series so the UI knows what charts to render without
/// hard-coding device names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesKeys {
    pub interfaces: Vec<String>,
    pub disks: Vec<String>,
    #[serde(default)]
    pub temps: Vec<String>,
}

pub fn series_keys(db: &Database) -> Result<SeriesKeys> {
    db.with(|c| {
        let interfaces = {
            let mut stmt =
                c.prepare("SELECT DISTINCT iface FROM net_samples ORDER BY iface")?;
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let disks = {
            let mut stmt =
                c.prepare("SELECT DISTINCT device FROM disk_samples ORDER BY device")?;
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let temps = {
            let mut stmt =
                c.prepare("SELECT DISTINCT sensor FROM temp_samples ORDER BY sensor")?;
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(SeriesKeys { interfaces, disks, temps })
    })
}

/// Pull the latest row from each metric table and stitch them into a
/// single `Snapshot`. Used by the LCD dashboard's polling loop and by
/// the web admin's `/api/stats/snapshot`. `ts_unix` is set to the most
/// recent ts seen across all tables; consumers that need finer
/// per-metric staleness can fall back to `*_range` with `to=now`.
pub fn latest_snapshot(db: &Database) -> Result<Snapshot> {
    db.with(|c| {
        // CPU + mem are single-row-per-tick; pick the max ts.
        let cpu: CpuStats = c
            .query_row(
                "SELECT busy_pct FROM cpu_samples ORDER BY ts DESC LIMIT 1",
                [],
                |r| Ok(CpuStats { busy_pct: r.get::<_, f64>(0)? as f32 }),
            )
            .ok()
            .unwrap_or_default();
        let mem: MemStats = c
            .query_row(
                "SELECT total, used, available, free FROM mem_samples ORDER BY ts DESC LIMIT 1",
                [],
                |r| Ok(MemStats {
                    total: r.get::<_, i64>(0)? as u64,
                    used: r.get::<_, i64>(1)? as u64,
                    available: r.get::<_, i64>(2)? as u64,
                    free: r.get::<_, i64>(3)? as u64,
                }),
            )
            .ok()
            .unwrap_or_default();

        // For per-device tables, take the latest row per (iface|device|mount).
        let network: Vec<NetIface> = {
            let mut stmt = c.prepare(
                "SELECT iface, rx_bps, tx_bps FROM net_samples \
                 WHERE ts = (SELECT MAX(ts) FROM net_samples WHERE iface = net_samples.iface) \
                 GROUP BY iface ORDER BY iface",
            )?;
            stmt.query_map([], |r| {
                Ok(NetIface {
                    name: r.get(0)?,
                    rx_bps: r.get::<_, i64>(1)? as u64,
                    tx_bps: r.get::<_, i64>(2)? as u64,
                    // Cumulative totals aren't stored — this snapshot is
                    // for live display, not byte accounting.
                    rx_total: 0,
                    tx_total: 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let disks: Vec<DiskIo> = {
            let mut stmt = c.prepare(
                "SELECT device, read_bps, write_bps, read_iops, write_iops, util_pct FROM disk_samples \
                 WHERE ts = (SELECT MAX(ts) FROM disk_samples WHERE device = disk_samples.device) \
                 GROUP BY device ORDER BY device",
            )?;
            stmt.query_map([], |r| {
                Ok(DiskIo {
                    device: r.get(0)?,
                    bus: String::new(),
                    read_bps: r.get::<_, i64>(1)? as u64,
                    write_bps: r.get::<_, i64>(2)? as u64,
                    read_iops: r.get::<_, i64>(3)? as u64,
                    write_iops: r.get::<_, i64>(4)? as u64,
                    util_pct: r.get::<_, f64>(5)? as f32,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let parts: Vec<Partition> = {
            let mut stmt = c.prepare(
                "SELECT mount, device, used, total FROM part_samples \
                 WHERE ts = (SELECT MAX(ts) FROM part_samples WHERE mount = part_samples.mount) \
                 GROUP BY mount ORDER BY mount",
            )?;
            stmt.query_map([], |r| {
                let used = r.get::<_, i64>(2)? as u64;
                let total = r.get::<_, i64>(3)? as u64;
                Ok(Partition {
                    mount: r.get(0)?,
                    device: r.get(1)?,
                    used,
                    total,
                    free: total.saturating_sub(used),
                    parent: String::new(),
                    fs: String::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let temps: Vec<crate::metrics::temp::TempReading> = {
            let mut stmt = c.prepare(
                "SELECT sensor, celsius FROM temp_samples \
                 WHERE ts = (SELECT MAX(ts) FROM temp_samples WHERE sensor = temp_samples.sensor) \
                 GROUP BY sensor ORDER BY sensor",
            )?;
            stmt.query_map([], |r| {
                Ok(crate::metrics::temp::TempReading {
                    sensor: r.get(0)?,
                    celsius: r.get::<_, f64>(1)? as f32,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };

        // Use the freshest ts across all tables.
        let ts_unix: i64 = c
            .query_row(
                "SELECT MAX(ts) FROM (
                    SELECT MAX(ts) AS ts FROM cpu_samples UNION ALL
                    SELECT MAX(ts) FROM mem_samples UNION ALL
                    SELECT MAX(ts) FROM net_samples UNION ALL
                    SELECT MAX(ts) FROM disk_samples UNION ALL
                    SELECT MAX(ts) FROM part_samples UNION ALL
                    SELECT MAX(ts) FROM temp_samples
                )",
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
            .unwrap_or(0);

        Ok(Snapshot { ts_unix, cpu, mem, network, disks, parts, temps })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::open_test_db;

    #[test]
    fn empty_range_returns_empty() {
        let db = open_test_db();
        let r = net_range(&db, "eth0", 0, 100, "net_samples").unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn latest_snapshot_on_empty_db_yields_defaults() {
        let db = open_test_db();
        let s = latest_snapshot(&db).unwrap();
        assert_eq!(s.ts_unix, 0);
        assert_eq!(s.cpu.busy_pct, 0.0);
        assert!(s.network.is_empty());
        assert!(s.disks.is_empty());
        assert!(s.parts.is_empty());
    }
}
