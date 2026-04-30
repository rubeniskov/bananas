//! `bananas-config` — operator console for the BPI.
//!
//! Two faces:
//!   - With no arguments (or `tui`), opens an interactive ratatui
//!     dashboard — status of the four units, current versions, a
//!     timezone editor, and a reboot button. Designed to fit the LCD
//!     panel (800×480) but works on any terminal.
//!   - With a subcommand, runs the same operations non-interactively
//!     so they're scriptable over SSH or a serial console:
//!       bananas-config status
//!       bananas-config tz Europe/Madrid
//!       bananas-config reboot
//!       bananas-config update check
//!       bananas-config update install stats
//!
//! All privileged operations go through the `bananas-helper` Unix
//! socket — no per-binary capabilities, no setuid. The user must be
//! in the `bananas` group (the helper socket is `srw-rw---- root:bananas`).

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

mod cli;
mod tui;

/// Default helper socket path; mirrors the server / helper convention.
fn default_socket() -> PathBuf {
    std::env::var_os("BANANAS_HELPER_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/bananas/helper.sock"))
}

#[derive(Debug, Parser)]
#[command(
    name = "bananas-config",
    version,
    about = "BanaNAS operator console (TUI + CLI)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Open the interactive TUI (default when no subcommand given).
    Tui,
    /// Print a one-shot status table (versions + service state).
    Status,
    /// Read or set the system timezone (IANA tzdata name).
    Tz {
        /// New timezone (e.g. Europe/Madrid). When omitted, prints
        /// the currently set zone.
        #[arg(value_name = "ZONE")]
        zone: Option<String>,
    },
    /// systemctl reboot — drops the SSH session, comes back in ~30 s.
    Reboot {
        /// Skip the y/n confirmation prompt. Required for use in
        /// non-interactive contexts (cron, scripts).
        #[arg(long)]
        yes: bool,
    },
    /// In-place package operations against the opkg feed.
    Update {
        #[command(subcommand)]
        sub: UpdateCmd,
    },
}

#[derive(Debug, Subcommand)]
enum UpdateCmd {
    /// Refresh the feed index + print upgradable bananas-* packages.
    Check,
    /// Run `opkg upgrade` for one or more packages. Each argument
    /// without a `bananas-` prefix gets one prepended automatically,
    /// so `bananas-config update install stats dashboard` is
    /// equivalent to `... bananas-stats bananas-dashboard`.
    Install {
        #[arg(value_name = "PACKAGE", required = true)]
        packages: Vec<String>,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // Skip env-filter to match the helper's binary-size policy. The
    // operator running this won't typically want trace logs.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_target(false)
        .compact()
        .init();

    let socket = default_socket();
    let cli = Cli::parse();

    match cli.cmd.unwrap_or(Cmd::Tui) {
        Cmd::Tui => tui::run(&socket).await,
        Cmd::Status => cli::status(&socket).await,
        Cmd::Tz { zone } => cli::timezone(&socket, zone.as_deref()).await,
        Cmd::Reboot { yes } => cli::reboot(&socket, yes).await,
        Cmd::Update {
            sub: UpdateCmd::Check,
        } => cli::update_check(&socket).await,
        Cmd::Update {
            sub: UpdateCmd::Install { packages },
        } => cli::update_install(&socket, &packages).await,
    }
}
