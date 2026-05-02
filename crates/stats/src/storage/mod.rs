//! SQLite-backed time-series store. WAL mode so the UI can read while the
//! writer commits.

pub mod budget;
pub mod queries;
pub mod writer;

pub use writer::Writer;

use crate::config::Storage as StorageCfg;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA: &str = include_str!("schema.sql");

#[derive(Clone)]
pub struct Database {
    inner: Arc<Mutex<Connection>>,
    /// Hard size cap chosen on first run and persisted in the `meta` table.
    budget_bytes: u64,
}

impl Database {
    pub fn open(cfg: &StorageCfg) -> Result<Self> {
        if let Some(parent) = cfg.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating db dir {}", parent.display()))?;
        }
        let conn = Connection::open(&cfg.path)
            .with_context(|| format!("opening sqlite at {}", cfg.path.display()))?;
        Self::tune(&conn)?;
        conn.execute_batch(SCHEMA).context("applying schema")?;
        let budget_bytes = budget::ensure_budget(&conn, &cfg.path, cfg.max_db_mb)?;
        tracing::info!(
            budget_mb = budget_bytes / (1024 * 1024),
            db_path = %cfg.path.display(),
            "db size budget"
        );
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
            budget_bytes,
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::tune(&conn)?;
        conn.execute_batch(SCHEMA)?;
        let budget_bytes = budget::ensure_budget(&conn, Path::new("/tmp/in-memory"), None)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
            budget_bytes,
        })
    }

    /// Open the DB read-only — used by readers (web admin, dashboard)
    /// that share `/var/lib/bananas/stats.db` with the daemon. WAL means
    /// multiple readers don't block the writer or each other.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        use rusqlite::OpenFlags;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening sqlite read-only at {}", path.display()))?;
        // Even read-only connections want a generous mmap so range scans
        // don't trigger small per-page reads.
        conn.pragma_update(None, "mmap_size", 64_000_000_i64).ok();
        conn.pragma_update(None, "temp_store", "MEMORY").ok();
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
            budget_bytes: 0,
        })
    }

    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    fn tune(conn: &Connection) -> Result<()> {
        // WAL = concurrent reads with one writer. NORMAL synchronous is fine
        // for a dashboard — we tolerate a few lost samples on a hard crash.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.pragma_update(None, "mmap_size", 64_000_000_i64)?;
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        Ok(())
    }

    pub fn with<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut Connection) -> T,
    {
        let mut g = self.inner.lock().expect("db mutex poisoned");
        f(&mut g)
    }
}

/// Convenience for tests.
#[cfg(test)]
pub fn open_test_db() -> Database {
    Database::open_in_memory().expect("in-memory db")
}

#[allow(dead_code)]
fn _path_marker(_p: &Path) {}
