//! Automatic SQLite size budget.
//!
//! On first run we look at the free space on the filesystem that holds the DB,
//! pick a sensible cap (5 % of free, clamped to [10 MB, 100 MB]), and persist
//! it in the `meta` table. From then on the budget is fixed regardless of
//! host changes — the retention pass uses it as a hard ceiling and prunes the
//! oldest 1-minute aggregates whenever the file would otherwise exceed it.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub const MIN_BUDGET_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_BUDGET_BYTES: u64 = 100 * 1024 * 1024;

/// Read the persisted budget, or compute and persist one on first run.
pub fn ensure_budget(conn: &Connection, db_path: &Path, override_mb: Option<u64>) -> Result<u64> {
    if let Some(b) = read_budget(conn)? {
        return Ok(b);
    }
    let budget = match override_mb {
        Some(mb) => mb * 1024 * 1024,
        None => {
            let parent = db_path.parent().unwrap_or_else(|| Path::new("/"));
            let free = available_bytes_at(parent).unwrap_or(1_000_000_000);
            auto_budget(free)
        }
    };
    write_budget(conn, budget)?;
    Ok(budget)
}

pub fn read_budget(conn: &Connection) -> Result<Option<u64>> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'max_db_bytes'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.map(|n| n as u64))
}

fn write_budget(conn: &Connection, budget: u64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO meta (key, value) VALUES ('max_db_bytes', ?1)",
        params![(budget as i64).to_string()],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO meta (key, value) VALUES ('created_at', strftime('%s','now'))",
        [],
    )?;
    Ok(())
}

/// Current SQLite file size from PRAGMAs (page_count * page_size). Excludes
/// WAL/SHM, but those are bounded and recycled — the page count is the real
/// growth signal.
pub fn current_db_bytes(conn: &Connection) -> Result<u64> {
    let pages: i64 = conn.pragma_query_value(None, "page_count", |r| r.get(0))?;
    let size: i64 = conn.pragma_query_value(None, "page_size", |r| r.get(0))?;
    Ok((pages.max(0) as u64) * (size.max(0) as u64))
}

/// 5 % of free space, clamped to [MIN_BUDGET, MAX_BUDGET]. Pure for tests.
pub fn auto_budget(free_bytes: u64) -> u64 {
    (free_bytes / 20).clamp(MIN_BUDGET_BYTES, MAX_BUDGET_BYTES)
}

#[cfg(target_os = "linux")]
pub fn available_bytes_at(path: &Path) -> Option<u64> {
    use nix::sys::statvfs::statvfs;
    let st = statvfs(path).ok()?;
    Some(st.fragment_size() as u64 * st.blocks_available() as u64)
}

#[cfg(not(target_os = "linux"))]
pub fn available_bytes_at(_path: &Path) -> Option<u64> {
    // Off-Linux dev hosts (macOS): pretend we have 1 GB free so the auto
    // path still picks a sane budget for testing.
    Some(1_000_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_budget_clamps_low() {
        assert_eq!(auto_budget(0), MIN_BUDGET_BYTES);
        assert_eq!(auto_budget(1_000_000), MIN_BUDGET_BYTES); // 50 KB → 10 MB floor
    }

    #[test]
    fn auto_budget_takes_5pct_in_middle() {
        // 800 MB free → 5 % = 40 MB, in range.
        assert_eq!(auto_budget(800 * 1024 * 1024), 40 * 1024 * 1024);
    }

    #[test]
    fn auto_budget_clamps_high() {
        // 100 GB free → 5 % = 5 GB, capped at 100 MB.
        assert_eq!(auto_budget(100 * 1024 * 1024 * 1024), MAX_BUDGET_BYTES);
    }

    #[test]
    fn ensure_budget_persists_first_run() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("schema.sql")).unwrap();
        let path = std::path::PathBuf::from("/tmp/test.sqlite");
        let b1 = ensure_budget(&conn, &path, None).unwrap();
        assert!(b1 >= MIN_BUDGET_BYTES);
        let b2 = ensure_budget(&conn, &path, None).unwrap();
        assert_eq!(b1, b2, "second call must read persisted value");
    }

    #[test]
    fn ensure_budget_uses_override_when_no_meta_row() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("schema.sql")).unwrap();
        let path = std::path::PathBuf::from("/tmp/x.sqlite");
        let b = ensure_budget(&conn, &path, Some(25)).unwrap();
        assert_eq!(b, 25 * 1024 * 1024);
    }

    #[test]
    fn ensure_budget_prefers_persisted_over_override() {
        // If a previous run already chose 40 MB, an override on a later run
        // shouldn't quietly change it — the user has to drop the meta row
        // explicitly. (Avoids surprising shrinkage that would prune history.)
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("schema.sql")).unwrap();
        let path = std::path::PathBuf::from("/tmp/x.sqlite");
        let b1 = ensure_budget(&conn, &path, Some(40)).unwrap();
        let b2 = ensure_budget(&conn, &path, Some(80)).unwrap();
        assert_eq!(b1, b2);
        assert_eq!(b1, 40 * 1024 * 1024);
    }
}
