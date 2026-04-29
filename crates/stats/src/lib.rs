//! BanaNAS stats library — vendored from the standalone `bananas-dashboard`
//! repo (see /home/rubeniskov/Workspaces/rubeniskov/bananas-dashboard for the
//! upstream snapshot we forked from). Re-exports four submodules:
//!
//! - `metrics` — `/proc` / `/sys` samplers + the `Snapshot` wire shape
//! - `storage` — WAL-mode SQLite writer/reader + retention budget
//! - `config`  — TOML config types (sampling intervals, retention windows,
//!                device filters, network filters, UI prefs)
//!
//! Two binaries depend on this lib: the daemon at `bin/main.rs` (writer)
//! and `crates/dashboard` (reader, LCD UI). The HTTP admin
//! (`crates/server`) also reads through these queries to expose
//! `/api/stats/*`.

pub mod config;
pub mod metrics;
pub mod storage;
