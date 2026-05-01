//! BanaNAS stats library — vendored from the standalone `bananas-dashboard`
//! repo (see /home/rubeniskov/Workspaces/rubeniskov/bananas-dashboard for the
//! upstream snapshot we forked from). Re-exports four submodules:
//!
//! - `metrics` — `/proc` / `/sys` samplers + the `Snapshot` wire shape
//! - `storage` — WAL-mode SQLite writer/reader + retention budget
//! - `config`  — TOML config types (sampling intervals, retention windows,
//!   device filters, network filters, UI prefs)
//!
//! Native-only — the wasm SPA bin (`bananas-stats-web-ui`) lives in this
//! same package as a separate `[[bin]]` and does not consume the lib;
//! gating the whole module with `cfg(not(target_arch = "wasm32"))`
//! keeps tokio, rusqlite, and friends out of the wasm32 dep tree.

#![cfg(not(target_arch = "wasm32"))]

pub mod config;
pub mod live_socket;
pub mod metrics;
pub mod storage;
