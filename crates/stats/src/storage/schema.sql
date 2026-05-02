-- Per-database metadata. Today carries the auto-computed size budget and the
-- first-run timestamp; future migrations can hang flags off it too.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;

-- Raw 1-second samples (default 24 h retention, configurable).

CREATE TABLE IF NOT EXISTS net_samples (
    ts        INTEGER NOT NULL,
    iface     TEXT    NOT NULL,
    rx_bps    INTEGER NOT NULL,
    tx_bps    INTEGER NOT NULL,
    PRIMARY KEY (ts, iface)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_net_ts ON net_samples(ts);

CREATE TABLE IF NOT EXISTS disk_samples (
    ts          INTEGER NOT NULL,
    device      TEXT    NOT NULL,
    read_bps    INTEGER NOT NULL,
    write_bps   INTEGER NOT NULL,
    read_iops   INTEGER NOT NULL,
    write_iops  INTEGER NOT NULL,
    util_pct    REAL    NOT NULL,
    PRIMARY KEY (ts, device)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_disk_ts ON disk_samples(ts);

CREATE TABLE IF NOT EXISTS part_samples (
    ts        INTEGER NOT NULL,
    mount     TEXT    NOT NULL,
    device    TEXT    NOT NULL,
    used      INTEGER NOT NULL,
    total     INTEGER NOT NULL,
    PRIMARY KEY (ts, mount)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_part_ts ON part_samples(ts);

-- CPU + memory: one row per tick (no per-device dimension). Mirrors the
-- shape of `metrics::cpu::CpuStats` and `metrics::mem::MemStats` so the
-- writer is a one-line insert per row.
CREATE TABLE IF NOT EXISTS cpu_samples (
    ts        INTEGER PRIMARY KEY,
    busy_pct  REAL    NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS mem_samples (
    ts          INTEGER PRIMARY KEY,
    total       INTEGER NOT NULL,
    used        INTEGER NOT NULL,
    available   INTEGER NOT NULL,
    free        INTEGER NOT NULL
) WITHOUT ROWID;

-- Temperature readings, one row per (ts, sensor). Sensor names are the
-- thermal_zone "type" string for CPU readings ("cpu_thermal", etc.) or
-- the block-device name for drivetemp readings ("sda", "nvme0n1", …).
CREATE TABLE IF NOT EXISTS temp_samples (
    ts        INTEGER NOT NULL,
    sensor    TEXT    NOT NULL,
    celsius   REAL    NOT NULL,
    PRIMARY KEY (ts, sensor)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_temp_ts ON temp_samples(ts);

-- 1-minute aggregates (default 30 d retention). Same shape; the writer
-- downsamples on the retention pass.

CREATE TABLE IF NOT EXISTS net_samples_1m (
    ts        INTEGER NOT NULL,
    iface     TEXT    NOT NULL,
    rx_bps    INTEGER NOT NULL,
    tx_bps    INTEGER NOT NULL,
    PRIMARY KEY (ts, iface)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS disk_samples_1m (
    ts          INTEGER NOT NULL,
    device      TEXT    NOT NULL,
    read_bps    INTEGER NOT NULL,
    write_bps   INTEGER NOT NULL,
    read_iops   INTEGER NOT NULL,
    write_iops  INTEGER NOT NULL,
    util_pct    REAL    NOT NULL,
    PRIMARY KEY (ts, device)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS part_samples_1m (
    ts        INTEGER NOT NULL,
    mount     TEXT    NOT NULL,
    device    TEXT    NOT NULL,
    used      INTEGER NOT NULL,
    total     INTEGER NOT NULL,
    PRIMARY KEY (ts, mount)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS temp_samples_1m (
    ts        INTEGER NOT NULL,
    sensor    TEXT    NOT NULL,
    celsius   REAL    NOT NULL,
    PRIMARY KEY (ts, sensor)
) WITHOUT ROWID;
