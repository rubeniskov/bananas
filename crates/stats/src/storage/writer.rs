//! Writer task: subscribes to the sampler watch channel, batches samples, and
//! commits them in a single transaction every `flush_interval_ms`.
//!
//! Also runs the retention pass: every 10 min, downsample raw → _1m and prune
//! both tables according to config.

use super::Database;
use super::budget::current_db_bytes;
use crate::config::Storage as StorageCfg;
use crate::metrics::Snapshot;
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::{Instant, interval, interval_at};

pub struct Writer {
    db: Database,
    cfg: StorageCfg,
    rx: watch::Receiver<Snapshot>,
}

impl Writer {
    pub fn new(db: Database, cfg: StorageCfg, rx: watch::Receiver<Snapshot>) -> Self {
        Self { db, cfg, rx }
    }

    pub async fn run(mut self) {
        let mut buf: Vec<Snapshot> = Vec::with_capacity(64);
        let mut flush = interval(Duration::from_millis(self.cfg.flush_interval_ms.max(500)));
        // First retention pass 60 s after start, then every 10 min.
        let mut retention = interval_at(
            Instant::now() + Duration::from_secs(60),
            Duration::from_secs(600),
        );

        loop {
            tokio::select! {
                changed = self.rx.changed() => {
                    if changed.is_err() {
                        // Sampler dropped — flush what we have and exit.
                        self.flush(&mut buf).await;
                        return;
                    }
                    let snap = self.rx.borrow_and_update().clone();
                    if snap.ts_unix > 0 { buf.push(snap); }
                }
                _ = flush.tick() => self.flush(&mut buf).await,
                _ = retention.tick() => {
                    if let Err(e) = self.retention_pass().await {
                        tracing::warn!(error = ?e, "retention pass failed");
                    }
                }
            }
        }
    }

    async fn flush(&self, buf: &mut Vec<Snapshot>) {
        if buf.is_empty() {
            return;
        }
        let snaps = std::mem::take(buf);
        let db = self.db.clone();
        let res = tokio::task::spawn_blocking(move || db.with(|c| insert_batch(c, &snaps))).await;
        if let Err(e) = res {
            tracing::warn!(error = ?e, "flush join error");
        } else if let Ok(Err(e)) = res {
            tracing::warn!(error = ?e, "flush failed");
        }
    }

    async fn retention_pass(&self) -> Result<()> {
        let cfg = self.cfg.clone();
        let db = self.db.clone();
        let budget = self.db.budget_bytes();
        tokio::task::spawn_blocking(move || {
            db.with(|c| {
                run_retention(c, &cfg)?;
                enforce_size_budget(c, budget)
            })
        })
        .await??;
        Ok(())
    }
}

fn insert_batch(conn: &mut rusqlite::Connection, snaps: &[Snapshot]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut net_stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO net_samples (ts, iface, rx_bps, tx_bps) VALUES (?,?,?,?)",
        )?;
        let mut disk_stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO disk_samples (ts, device, read_bps, write_bps, read_iops, write_iops, util_pct) VALUES (?,?,?,?,?,?,?)",
        )?;
        let mut part_stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO part_samples (ts, mount, device, used, total) VALUES (?,?,?,?,?)",
        )?;
        let mut cpu_stmt =
            tx.prepare_cached("INSERT OR REPLACE INTO cpu_samples (ts, busy_pct) VALUES (?,?)")?;
        let mut mem_stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO mem_samples (ts, total, used, available, free) VALUES (?,?,?,?,?)",
        )?;
        let mut temp_stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO temp_samples (ts, sensor, celsius) VALUES (?,?,?)",
        )?;
        for s in snaps {
            for n in &s.network {
                net_stmt.execute(params![s.ts_unix, n.name, n.rx_bps as i64, n.tx_bps as i64])?;
            }
            for d in &s.disks {
                disk_stmt.execute(params![
                    s.ts_unix,
                    d.device,
                    d.read_bps as i64,
                    d.write_bps as i64,
                    d.read_iops as i64,
                    d.write_iops as i64,
                    d.util_pct as f64,
                ])?;
            }
            for p in &s.parts {
                part_stmt.execute(params![
                    s.ts_unix,
                    p.mount,
                    p.device,
                    p.used as i64,
                    p.total as i64,
                ])?;
            }
            cpu_stmt.execute(params![s.ts_unix, s.cpu.busy_pct as f64])?;
            mem_stmt.execute(params![
                s.ts_unix,
                s.mem.total as i64,
                s.mem.used as i64,
                s.mem.available as i64,
                s.mem.free as i64,
            ])?;
            for t in &s.temps {
                temp_stmt.execute(params![s.ts_unix, t.sensor, t.celsius as f64])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// Move raw rows older than the raw cutoff into _1m aggregates, then delete
/// _1m rows older than the agg cutoff. Inclusive bounds: anything strictly
/// older than `now - retention` is rolled up or removed.
fn run_retention(conn: &mut rusqlite::Connection, cfg: &StorageCfg) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let raw_cutoff = now - cfg.raw_retention_hours as i64 * 3600;
    let agg_cutoff = now - cfg.agg_retention_days as i64 * 86400;

    let tx = conn.transaction()?;
    // Roll up any raw rows older than raw_cutoff into _1m by bucketing on
    // (ts / 60) * 60 and averaging (using AVG on bps/IOPS, MAX on util%).
    tx.execute_batch(
        "
        INSERT OR REPLACE INTO net_samples_1m (ts, iface, rx_bps, tx_bps)
        SELECT (ts/60)*60 AS bucket, iface,
               CAST(AVG(rx_bps) AS INTEGER), CAST(AVG(tx_bps) AS INTEGER)
        FROM net_samples
        WHERE ts < strftime('%s','now') - 0
        GROUP BY bucket, iface;

        INSERT OR REPLACE INTO disk_samples_1m
            (ts, device, read_bps, write_bps, read_iops, write_iops, util_pct)
        SELECT (ts/60)*60, device,
               CAST(AVG(read_bps)  AS INTEGER), CAST(AVG(write_bps) AS INTEGER),
               CAST(AVG(read_iops) AS INTEGER), CAST(AVG(write_iops) AS INTEGER),
               MAX(util_pct)
        FROM disk_samples GROUP BY (ts/60)*60, device;

        INSERT OR REPLACE INTO part_samples_1m (ts, mount, device, used, total)
        SELECT (ts/60)*60, mount, device,
               CAST(AVG(used) AS INTEGER), CAST(AVG(total) AS INTEGER)
        FROM part_samples GROUP BY (ts/60)*60, mount, device;

        INSERT OR REPLACE INTO temp_samples_1m (ts, sensor, celsius)
        SELECT (ts/60)*60, sensor, AVG(celsius)
        FROM temp_samples GROUP BY (ts/60)*60, sensor;
        ",
    )?;
    tx.execute(
        "DELETE FROM net_samples  WHERE ts < ?1",
        params![raw_cutoff],
    )?;
    tx.execute(
        "DELETE FROM disk_samples WHERE ts < ?1",
        params![raw_cutoff],
    )?;
    tx.execute(
        "DELETE FROM part_samples WHERE ts < ?1",
        params![raw_cutoff],
    )?;
    tx.execute(
        "DELETE FROM temp_samples WHERE ts < ?1",
        params![raw_cutoff],
    )?;
    tx.execute(
        "DELETE FROM net_samples_1m  WHERE ts < ?1",
        params![agg_cutoff],
    )?;
    tx.execute(
        "DELETE FROM disk_samples_1m WHERE ts < ?1",
        params![agg_cutoff],
    )?;
    tx.execute(
        "DELETE FROM part_samples_1m WHERE ts < ?1",
        params![agg_cutoff],
    )?;
    tx.execute(
        "DELETE FROM temp_samples_1m WHERE ts < ?1",
        params![agg_cutoff],
    )?;
    tx.commit()?;

    conn.execute("PRAGMA incremental_vacuum", [])?;
    Ok(())
}

/// Drop the oldest 1-minute aggregate rows in 1-hour chunks until the file is
/// back under `budget_bytes`. Caps iterations to avoid runaway loops if the
/// budget is unreachably small (file overhead alone exceeds it).
pub fn enforce_size_budget(conn: &mut rusqlite::Connection, budget_bytes: u64) -> Result<()> {
    const MAX_ITERS: usize = 240; // 240 hours = 10 days of 1m data per pass.
    for _ in 0..MAX_ITERS {
        let cur = current_db_bytes(conn)?;
        if cur <= budget_bytes {
            return Ok(());
        }

        // Pick the oldest 1m timestamp across the three aggregate tables.
        let oldest: Option<i64> = oldest_agg_ts(conn)?;
        let Some(cutoff_start) = oldest else {
            return Ok(());
        };
        let cutoff = cutoff_start + 3600; // drop one hour at a time

        let tx = conn.transaction()?;
        tx.execute("DELETE FROM net_samples_1m  WHERE ts < ?1", params![cutoff])?;
        tx.execute("DELETE FROM disk_samples_1m WHERE ts < ?1", params![cutoff])?;
        tx.execute("DELETE FROM part_samples_1m WHERE ts < ?1", params![cutoff])?;
        tx.commit()?;
        conn.execute("PRAGMA incremental_vacuum", [])?;
    }

    let cur = current_db_bytes(conn)?;
    if cur > budget_bytes {
        tracing::warn!(
            current_bytes = cur,
            budget_bytes,
            "could not prune below budget after {MAX_ITERS} iterations"
        );
    }
    Ok(())
}

fn oldest_agg_ts(conn: &rusqlite::Connection) -> Result<Option<i64>> {
    let mut min: Option<i64> = None;
    for table in ["net_samples_1m", "disk_samples_1m", "part_samples_1m"] {
        let sql = format!("SELECT MIN(ts) FROM {table}");
        let v: Option<i64> = conn
            .query_row(&sql, [], |r| r.get::<_, Option<i64>>(0))
            .optional()?
            .flatten();
        if let Some(t) = v {
            min = Some(min.map_or(t, |m| m.min(t)));
        }
    }
    Ok(min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{disk::DiskIo, net::NetIface, partitions::Partition};
    use crate::storage::open_test_db;

    fn snap(ts: i64) -> Snapshot {
        Snapshot {
            ts_unix: ts,
            cpu: Default::default(),
            mem: Default::default(),
            network: vec![NetIface {
                name: "eth0".into(),
                rx_bps: 1000,
                tx_bps: 500,
                rx_total: 0,
                tx_total: 0,
            }],
            disks: vec![DiskIo {
                device: "sda".into(),
                bus: "sata".into(),
                read_bps: 1024,
                write_bps: 2048,
                read_iops: 1,
                write_iops: 2,
                util_pct: 5.0,
            }],
            parts: vec![Partition {
                device: "/dev/sda1".into(),
                parent: "sda".into(),
                mount: "/mnt/data".into(),
                fs: "ext4".into(),
                total: 1_000_000,
                used: 250_000,
                free: 750_000,
            }],
            temps: vec![],
        }
    }

    #[test]
    fn round_trip_insert_and_query() {
        let db = open_test_db();
        let snaps = vec![snap(100), snap(101), snap(102)];
        db.with(|c| insert_batch(c, &snaps).unwrap());

        let r = crate::storage::queries::net_range(&db, "eth0", 0, 1000, "net_samples").unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].rx_bps, 1000);
    }

    #[test]
    fn enforce_size_budget_drops_oldest_agg_rows() {
        let db = open_test_db();
        // Seed _1m tables with 6 hours of synthetic rows.
        db.with(|c| {
            let tx = c.transaction().unwrap();
            for hour in 0..6 {
                let ts = hour * 3600;
                tx.execute(
                    "INSERT INTO net_samples_1m (ts, iface, rx_bps, tx_bps) VALUES (?,?,?,?)",
                    params![ts, "eth0", 100i64, 100i64],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        });

        // Pick a budget the table is already over (1 byte).
        db.with(|c| enforce_size_budget(c, 1).unwrap());

        // After enforcement the table should have at most one row (the loop
        // bails when it can't make further progress; in-memory page overhead
        // alone exceeds 1 byte).
        let count: i64 = db.with(|c| {
            c.query_row("SELECT COUNT(*) FROM net_samples_1m", [], |r| r.get(0))
                .unwrap()
        });
        assert!(count <= 1, "expected ≤1 row, got {count}");
    }

    #[test]
    fn retention_prunes_old_raw_rows() {
        use crate::config::Storage;
        let db = open_test_db();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let snaps = vec![
            snap(now - 100_000), // older than 24h => should be pruned
            snap(now - 10),
        ];
        db.with(|c| insert_batch(c, &snaps).unwrap());

        let cfg = Storage {
            path: "ignored".into(),
            raw_retention_hours: 24,
            agg_retention_days: 30,
            flush_interval_ms: 5000,
            max_db_mb: None,
        };
        db.with(|c| run_retention(c, &cfg).unwrap());

        let r = crate::storage::queries::net_range(&db, "eth0", 0, now + 1, "net_samples").unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].ts >= now - 10);
    }
}
