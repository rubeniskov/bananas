//! Subcommand handlers — the same operations the TUI exposes, but
//! wired to stdout/stderr so they can be scripted over SSH.

use std::{io::Write, path::Path};

use anyhow::{Context, Result, bail};
use bananas_helper::{Command as HelperCommand, Component};

use crate::updates;

const UNITS: &[&str] = &[
    "bananas-server.service",
    "bananas-helper.service",
    "bananas-stats.service",
    "bananas-dashboard.service",
];

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
        let installed = versions
            .get(*component)
            .map(|s| s.as_str())
            .unwrap_or("—");
        let unit_state = unit
            .and_then(|u| units.get(u).cloned())
            .unwrap_or_else(|| "n/a".to_string());
        println!(
            "  {component:<15} {installed:<14} {unit_state}"
        );
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

pub async fn update_check(socket: &Path) -> Result<()> {
    let report = updates::check(socket).await?;
    println!("Latest release: v{}", report.latest_version);
    println!();
    println!("  COMPONENT       INSTALLED      LATEST         STATUS");
    println!("  ─────────────── ────────────── ────────────── ──────────────");
    for c in updates::COMPONENTS {
        let s = report.components.get(*c);
        let installed = s.and_then(|r| r.installed.as_deref()).unwrap_or("—");
        let latest = &report.latest_version;
        let outdated = s.is_some_and(|r| r.outdated);
        let deferred = matches!(*c, "server" | "helper");
        let status = if deferred {
            "self-update deferred"
        } else if outdated {
            "update available"
        } else {
            "up to date"
        };
        println!("  {c:<15} {installed:<14} {latest:<14} {status}");
    }
    Ok(())
}

pub async fn update_install(socket: &Path, component_name: &str) -> Result<()> {
    let component = match component_name {
        "stats" => Component::Stats,
        "dashboard" => Component::Dashboard,
        "webadmin" => Component::Webadmin,
        "server" | "helper" => bail!(
            "self-update for `{component_name}` is not yet supported (deferred to v2)"
        ),
        other => bail!("unknown component {other:?}"),
    };
    let report = updates::check(socket).await?;
    let entry = report
        .components
        .get(component_name)
        .ok_or_else(|| anyhow::anyhow!("no GitHub asset for {component_name}"))?;
    let asset_url = entry
        .asset_url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing asset_url"))?;
    let sha256 = entry
        .sha256
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing sha256 — does this release publish SHA256SUMS?"))?;

    println!("Installing {component_name} v{}...", report.latest_version);
    let staged = updates::download(asset_url, component_name)
        .await
        .context("download")?;
    println!("  ✓ downloaded → {}", staged.display());
    println!("  ✓ verifying sha256...");
    updates::verify_sha(&staged, sha256).await.context("sha256")?;
    println!("  ✓ sha256 ok");
    println!("  → handing off to helper for atomic install...");
    let resp = bananas_helper::call(
        socket,
        &HelperCommand::InstallUpdate {
            component,
            tarball_path: staged.to_string_lossy().to_string(),
            expected_version: report.latest_version.clone(),
            expected_sha256: sha256.clone(),
        },
    )
    .await
    .context("calling helper")?;
    if !resp.ok {
        bail!(resp.error.unwrap_or_else(|| "helper refused install".into()));
    }
    println!("{}", resp.output.trim_end());
    println!();
    println!("Done.");
    Ok(())
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

pub async fn read_versions(socket: &Path) -> Result<std::collections::HashMap<String, String>> {
    let resp = bananas_helper::call(socket, &HelperCommand::ReadVersions)
        .await
        .context("calling helper")?;
    if !resp.ok {
        return Ok(std::collections::HashMap::new());
    }
    if resp.output.trim().is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let parsed: toml::Table = resp.output.parse().unwrap_or_default();
    let mut out = std::collections::HashMap::new();
    for (k, v) in &parsed {
        if let Some(version) = v
            .as_table()
            .and_then(|t| t.get("version"))
            .and_then(|v| v.as_str())
        {
            out.insert(k.clone(), version.to_string());
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
