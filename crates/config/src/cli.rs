//! Subcommand handlers — the same operations the TUI exposes, but
//! wired to stdout/stderr so they can be scripted over SSH.

use std::{io::Write, path::Path};

use anyhow::{Context, Result, bail};
use bananas_helper::Command as HelperCommand;
use serde::Deserialize;

const UNITS: &[&str] = &[
    "bananas-server.service",
    "bananas-helper.service",
    "bananas-stats.service",
    "bananas-dashboard.service",
];

const PACKAGE_PREFIX: &str = "bananas-";

pub async fn status(socket: &Path) -> Result<()> {
    let versions = read_versions(socket).await.unwrap_or_default();

    println!("BanaNAS {} on {}", current_self_version(), hostname());
    println!();
    println!("  COMPONENT       INSTALLED      SERVICE");
    println!("  ─────────────── ────────────── ──────────────");

    let units = unit_states().await;
    for (component, unit) in &[
        ("server", Some("bananas-server.service")),
        ("helper", Some("bananas-helper.service")),
        ("stats", Some("bananas-stats.service")),
        ("dashboard", Some("bananas-dashboard.service")),
        ("webadmin", None),
    ] {
        let installed = versions.get(*component).map(|s| s.as_str()).unwrap_or("—");
        let unit_state = unit
            .and_then(|u| units.get(u).cloned())
            .unwrap_or_else(|| "n/a".to_string());
        println!("  {component:<15} {installed:<14} {unit_state}");
    }
    Ok(())
}

pub async fn timezone(socket: &Path, zone: Option<&str>) -> Result<()> {
    match zone {
        Some(tz) => {
            let resp = bananas_helper::call(socket, &HelperCommand::SetTimezone { tz: tz.into() })
                .await
                .context("calling helper")?;
            if resp.ok {
                println!("Timezone set to {tz}.");
                Ok(())
            } else {
                bail!(resp.error.unwrap_or_else(|| "unknown helper error".into()))
            }
        }
        None => {
            // No `GetTimezone` command yet — read /etc/bananas/system.toml
            // through ReadServiceConfig instead.
            let resp = bananas_helper::call(
                socket,
                &HelperCommand::ReadServiceConfig {
                    name: "system".into(),
                },
            )
            .await
            .context("calling helper")?;
            if !resp.ok {
                bail!(resp.error.unwrap_or_else(|| "unknown helper error".into()));
            }
            let parsed: toml::Table = resp.output.parse().unwrap_or_default();
            let tz = parsed
                .get("system")
                .and_then(|v| v.as_table())
                .and_then(|t| t.get("timezone"))
                .and_then(|v| v.as_str())
                .unwrap_or("(unset — defaults to UTC)");
            println!("{tz}");
            Ok(())
        }
    }
}

pub async fn reboot(socket: &Path, skip_confirm: bool) -> Result<()> {
    if !skip_confirm {
        eprint!("Reboot the system now? [y/N] ");
        std::io::stderr().flush().ok();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).ok();
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            eprintln!("Aborted.");
            return Ok(());
        }
    }
    let resp = bananas_helper::call(socket, &HelperCommand::RebootSystem)
        .await
        .context("calling helper")?;
    if resp.ok {
        println!("Reboot triggered.");
    } else {
        bail!(resp.error.unwrap_or_else(|| "helper refused".into()));
    }
    Ok(())
}

/// Refresh the opkg feed index and print upgradable bananas-* packages.
pub async fn update_check(socket: &Path) -> Result<()> {
    let update_resp = bananas_helper::call(socket, &HelperCommand::OpkgUpdate)
        .await
        .context("calling helper for opkg update")?;
    if !update_resp.ok {
        eprintln!(
            "warning: opkg update failed: {}",
            update_resp.error.unwrap_or_else(|| "unknown error".into())
        );
        // Continue anyway — list-upgradable still works against
        // cached metadata.
    }

    let upgradable = list_upgradable(socket).await?;
    let bananas: Vec<&UpgradablePackage> = upgradable
        .iter()
        .filter(|p| p.name.starts_with(PACKAGE_PREFIX))
        .collect();

    if bananas.is_empty() {
        println!("All bananas-* packages are up to date.");
        return Ok(());
    }
    println!("{} package(s) upgradable:", bananas.len());
    println!();
    println!("  PACKAGE                INSTALLED       AVAILABLE");
    println!("  ────────────────────── ─────────────── ───────────────");
    for p in &bananas {
        println!("  {:<22} {:<15} {}", p.name, p.installed, p.candidate);
    }
    Ok(())
}

/// Run `opkg upgrade` for one or more packages. Names without the
/// `bananas-` prefix get one auto-prepended for ergonomics.
pub async fn update_install(socket: &Path, packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        bail!("no packages specified");
    }
    let resolved: Vec<String> = packages
        .iter()
        .map(|p| {
            if p.starts_with(PACKAGE_PREFIX) {
                p.clone()
            } else {
                format!("{PACKAGE_PREFIX}{p}")
            }
        })
        .collect();

    println!("Upgrading: {}", resolved.join(", "));
    println!();
    let resp = bananas_helper::call(
        socket,
        &HelperCommand::OpkgUpgrade {
            packages: resolved.clone(),
        },
    )
    .await
    .context("calling helper")?;
    if !resp.output.is_empty() {
        print!("{}", resp.output);
        if !resp.output.ends_with('\n') {
            println!();
        }
    }
    if !resp.ok {
        bail!(
            resp.error
                .unwrap_or_else(|| "helper refused install".into())
        );
    }
    println!();
    println!("Done.");
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
struct UpgradablePackage {
    name: String,
    installed: String,
    candidate: String,
}

#[derive(Debug, Clone, Deserialize)]
struct InstalledPackageRow {
    name: String,
    version: String,
}

async fn list_upgradable(socket: &Path) -> Result<Vec<UpgradablePackage>> {
    let resp = bananas_helper::call(socket, &HelperCommand::OpkgListUpgradable)
        .await
        .context("calling helper for opkg list-upgradable")?;
    if !resp.ok {
        bail!(
            resp.error
                .unwrap_or_else(|| "opkg list-upgradable failed".into())
        );
    }
    Ok(serde_json::from_str(&resp.output).context("parsing list-upgradable JSON")?)
}

// ─── helpers ─────────────────────────────────────────────────────────

pub fn current_self_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Map from short component slug ("server", "stats", …) to installed
/// version, sourced from opkg list-installed. Slugs match the legacy
/// terminology so the TUI Status tab + the `status` subcommand keep
/// rendering the same shape; "server" and "helper" both resolve to
/// the bananas-server package.
pub async fn read_versions(socket: &Path) -> Result<std::collections::HashMap<String, String>> {
    let resp = bananas_helper::call(socket, &HelperCommand::OpkgListInstalled)
        .await
        .context("calling helper for opkg list-installed")?;
    if !resp.ok {
        return Ok(std::collections::HashMap::new());
    }
    let rows: Vec<InstalledPackageRow> = serde_json::from_str(&resp.output).unwrap_or_default();
    let mut by_pkg: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for r in rows {
        by_pkg.insert(r.name, r.version);
    }
    let mut out = std::collections::HashMap::new();
    for (slug, pkg) in [
        ("server", "bananas-server"),
        ("helper", "bananas-server"), // helper rides in the server IPK
        ("stats", "bananas-stats"),
        ("dashboard", "bananas-dashboard"),
        ("config", "bananas-config"),
        ("webadmin", "bananas-webadmin"),
    ] {
        if let Some(v) = by_pkg.get(pkg) {
            out.insert(slug.to_string(), v.clone());
        }
    }
    Ok(out)
}

pub async fn unit_states() -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for unit in UNITS {
        let state = match tokio::process::Command::new("systemctl")
            .arg("is-active")
            .arg(unit)
            .output()
            .await
        {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Err(_) => "unknown".to_string(),
        };
        out.insert(unit.to_string(), state);
    }
    out
}
