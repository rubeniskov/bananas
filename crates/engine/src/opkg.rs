//! opkg shell-out + parser. Wraps the package manager that ships with
//! the image with two distinct call shapes:
//!
//! * Synchronous (`update`, `list-upgradable`, `list-installed`) — the
//!   helper waits for the child, captures stdout+stderr, returns text.
//! * Asynchronous (`upgrade`, `upgrade_status`) — the helper spawns
//!   opkg as a transient systemd unit detached from its own cgroup,
//!   then returns immediately. Status / log delta is queried via a
//!   separate command.
//!
//! Why the asynchronous shape for `upgrade`: opkg's own postinst on
//! `bananas-server.ipk` runs `systemctl restart bananas-server
//! bananas-engine`. systemd's default `KillMode=control-group` would
//! tear down every PID in the helper's cgroup — including the opkg
//! child the helper just spawned — leaving the system half-upgraded.
//! Running opkg inside its own transient `.service` unit (via
//! `systemd-run`) means it lives in an independent cgroup and rides
//! out helper's restart unaffected. Output goes to a log file the
//! webadmin SSE handler tails through `OpkgUpgradeStatus` so the user
//! still gets progress even after the server restarts.
//!
//! Package names supplied to `upgrade` are validated against
//! `[a-z][a-z0-9-]*` before they ever reach argv. opkg itself would
//! reject malformed names, but pre-validating keeps the helper's
//! audit trail honest about what's being executed as root.

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::process::Command as TokioCommand;

#[derive(Debug, Serialize)]
pub struct UpgradablePackage {
    pub name: String,
    pub installed: String,
    pub candidate: String,
}

#[derive(Debug, Serialize)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
}

/// Snapshot of the in-flight (or just-finished) opkg upgrade. Returned
/// from `upgrade_status` and forwarded to the webadmin's SSE handler.
#[derive(Debug, Serialize)]
pub struct UpgradeStatus {
    /// One of: `idle` (nothing has ever run), `active` (running), `done`
    /// (finished with exit 0), `failed` (finished with non-zero).
    pub state: String,
    /// Log delta — bytes from `since` to current EOF. May be empty when
    /// the caller is already caught up.
    pub log: String,
    /// Current log size in bytes. Pass back as `since` next call.
    pub log_offset: u64,
    /// Set once the exit marker is present in the log.
    pub exit_code: Option<i32>,
}

/// Transient systemd unit name. Reused across runs (we `reset-failed`
/// before each launch). `--collect` GCs it once finished.
const OPKG_UNIT: &str = "bananas-opkg-upgrade.service";
/// Log file. Written by the transient unit's StandardOutput=append:
/// directive (which the unit runs as root, so /var/lib/bananas-engine
/// is fine). Helper's `upgrade_status` reads it back.
const OPKG_LOG_DEFAULT: &str = "/var/lib/bananas-engine/opkg.log";
/// Sentinel line appended after opkg exits so `upgrade_status` can
/// distinguish "still running" from "done with exit code N".
const OPKG_EXIT_MARKER_PREFIX: &str = "[bananas-opkg-exit=";

fn opkg_bin() -> String {
    std::env::var("BANANAS_OPKG_BIN").unwrap_or_else(|_| "/usr/bin/opkg".into())
}

fn systemd_run_bin() -> String {
    std::env::var("BANANAS_SYSTEMD_RUN_BIN").unwrap_or_else(|_| "/usr/bin/systemd-run".into())
}

fn systemctl_bin() -> String {
    std::env::var("BANANAS_SYSTEMCTL_BIN").unwrap_or_else(|_| "/usr/bin/systemctl".into())
}

fn opkg_log_path() -> PathBuf {
    std::env::var("BANANAS_OPKG_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(OPKG_LOG_DEFAULT))
}

/// `opkg update` — refresh feed indices from /etc/opkg/customfeeds.conf.
pub async fn update() -> Result<String> {
    run_sync(&["update"]).await
}

/// `opkg list-upgradable` parsed into structured rows.
///
/// opkg's output format is one row per package:
///   `<name> - <installed> - <candidate>`
/// (separated by " - "). Malformed lines are skipped silently — the
/// helper response stays usable even if opkg ever changes its format.
pub async fn list_upgradable() -> Result<Vec<UpgradablePackage>> {
    let raw = run_sync(&["list-upgradable"]).await?;
    Ok(parse_list_upgradable(&raw))
}

/// `opkg list-installed` parsed into `{name, version}` rows.
pub async fn list_installed() -> Result<Vec<InstalledPackage>> {
    let raw = run_sync(&["list-installed"]).await?;
    Ok(parse_list_installed(&raw))
}

/// Spawn `opkg upgrade <packages...>` in a transient systemd unit. The
/// helper returns immediately once the unit is queued — actual progress
/// is observed via `upgrade_status`.
///
/// Refuses to start if a previous run is still active (no exit marker).
pub async fn upgrade(packages: &[String]) -> Result<String> {
    if packages.is_empty() {
        bail!("opkg upgrade: no packages specified");
    }
    for p in packages {
        validate_package_name(p)?;
    }

    let prior = upgrade_status(0).await.ok();
    if matches!(prior.as_ref().map(|s| s.state.as_str()), Some("active")) {
        bail!("an opkg upgrade is already in flight");
    }

    // Failed unit names linger in systemd's bookkeeping; resetting lets
    // us reuse the same unit name on the next launch.
    let _ = TokioCommand::new(systemctl_bin())
        .args(["reset-failed", OPKG_UNIT])
        .output()
        .await;

    let log_path = opkg_log_path();
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).context("creating opkg log dir")?;
    }
    // Seed the log with a marker line BEFORE spawning systemd-run.
    // Without this there's a window between truncate and systemd-run's
    // child producing its first byte where `upgrade_status()` returns
    // state="idle" (no exit marker, zero bytes). The server-side
    // watcher polls every 500 ms and treats sustained idle as a
    // helper-side desync — it would abort the op with "helper reports
    // no upgrade in progress" before opkg even gets to its first
    // download. The seed line keeps the file non-empty from the
    // moment the helper acks, so state is `active` for the watcher's
    // very first poll.
    let initial = format!(
        "Starting opkg upgrade {}\n",
        packages
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    std::fs::write(&log_path, initial.as_bytes()).context("seeding opkg log")?;

    let opkg_bin = opkg_bin();
    let pkgs_quoted: Vec<String> = packages.iter().map(|p| shell_quote(p)).collect();
    // Print the exit marker after opkg exits so upgrade_status() can
    // tell finished from in-flight without polling systemd. Newline-
    // terminated; matched on a line-by-line basis.
    let wrapped = format!(
        "{} upgrade {}; rc=$?; printf '\\n[bananas-opkg-exit=%d]\\n' \"$rc\"",
        opkg_bin,
        pkgs_quoted.join(" ")
    );

    let log_arg = log_path.display().to_string();
    let status = TokioCommand::new(systemd_run_bin())
        .args([
            "--unit",
            OPKG_UNIT,
            "--collect",
            "--no-block",
            "--quiet",
            "--property=Type=oneshot",
            &format!("--property=StandardOutput=append:{log_arg}"),
            &format!("--property=StandardError=append:{log_arg}"),
            "--",
            "/bin/sh",
            "-c",
            &wrapped,
        ])
        .status()
        .await
        .context("spawning systemd-run")?;

    if !status.success() {
        bail!("systemd-run exited with status {status}");
    }

    Ok(format!(
        "started opkg upgrade in transient unit {OPKG_UNIT}"
    ))
}

/// Read the upgrade log from byte offset `since` onward and report
/// whether the run is still active, finished, or never started. Cheap
/// (one read of a small file); safe to poll at SSE pace from the
/// server side.
pub async fn upgrade_status(since: u64) -> Result<UpgradeStatus> {
    let log_path = opkg_log_path();
    let buf = match tokio::fs::read(&log_path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UpgradeStatus {
                state: "idle".to_string(),
                log: String::new(),
                log_offset: 0,
                exit_code: None,
            });
        }
        Err(e) => return Err(anyhow::Error::new(e).context("reading opkg log")),
    };

    let total_len = buf.len() as u64;
    let delta = if since >= total_len {
        &[][..]
    } else {
        &buf[since as usize..]
    };
    let log = String::from_utf8_lossy(delta).into_owned();
    let full = String::from_utf8_lossy(&buf);

    let mut exit_code: Option<i32> = None;
    for line in full.lines().rev() {
        if let Some(rest) = line.strip_prefix(OPKG_EXIT_MARKER_PREFIX) {
            if let Some(num_str) = rest.strip_suffix(']') {
                if let Ok(n) = num_str.parse::<i32>() {
                    exit_code = Some(n);
                    break;
                }
            }
        }
    }

    let state = match exit_code {
        Some(0) => "done",
        Some(_) => "failed",
        None if total_len > 0 => "active",
        None => "idle",
    };

    Ok(UpgradeStatus {
        state: state.to_string(),
        log,
        log_offset: total_len,
        exit_code,
    })
}

/// Spawn opkg with the given args, capture combined stdout+stderr,
/// fail on non-zero exit. Used by `update` / `list-*`. NOT used by
/// `upgrade` — that one goes through systemd-run.
async fn run_sync(args: &[&str]) -> Result<String> {
    let bin = opkg_bin();
    let out = TokioCommand::new(&bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("spawning {bin} {args:?}"))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        bail!(
            "{bin} {} failed (status {}): {}",
            args.join(" "),
            out.status,
            combined.trim()
        );
    }
    Ok(combined)
}

fn validate_package_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("empty package name");
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        bail!("package name {name:?} must start with a lowercase letter");
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            bail!("package name {name:?} contains invalid character {c:?}");
        }
    }
    Ok(())
}

fn shell_quote(s: &str) -> String {
    // validate_package_name has already rejected anything outside
    // [a-z][a-z0-9-]* so single-quoting is technically redundant. Kept
    // as belt-and-braces in case the validator ever loosens.
    let escaped = s.replace('\'', r"'\''");
    format!("'{escaped}'")
}

fn parse_list_upgradable(raw: &str) -> Vec<UpgradablePackage> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, " - ");
            let name = parts.next()?.trim();
            let installed = parts.next()?.trim();
            let candidate = parts.next()?.trim();
            if name.is_empty() || installed.is_empty() || candidate.is_empty() {
                return None;
            }
            Some(UpgradablePackage {
                name: name.to_string(),
                installed: installed.to_string(),
                candidate: candidate.to_string(),
            })
        })
        .collect()
}

fn parse_list_installed(raw: &str) -> Vec<InstalledPackage> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, " - ");
            let name = parts.next()?.trim();
            let version = parts.next()?.trim();
            if name.is_empty() || version.is_empty() {
                return None;
            }
            Some(InstalledPackage {
                name: name.to_string(),
                version: version.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a fake opkg shell script that prints `out` to stdout +
    /// `err` to stderr and exits with `exit_code`. Returns the path
    /// to the script in a tempdir (caller drops the dir to clean up).
    fn fake_opkg(out: &str, err: &str, exit_code: i32) -> (tempfile::TempDir, std::path::PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("opkg");
        let script = format!(
            "#!/bin/sh\nprintf '%s' '{}'\nprintf '%s' '{}' >&2\nexit {}\n",
            out.replace('\'', "'\\''"),
            err.replace('\'', "'\\''"),
            exit_code
        );
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(script.as_bytes()).unwrap();
        drop(f);
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        (td, path)
    }

    #[test]
    fn parses_list_upgradable() {
        let raw = "bananas-server - 1.0.0 - 1.1.0\n\
                   bananas-stats - 1.0.0 - 1.1.0\n\
                   \n\
                   garbage line\n";
        let rows = parse_list_upgradable(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "bananas-server");
        assert_eq!(rows[0].installed, "1.0.0");
        assert_eq!(rows[0].candidate, "1.1.0");
        assert_eq!(rows[1].name, "bananas-stats");
    }

    #[test]
    fn parses_list_installed() {
        let raw = "bananas-server - 1.1.0\n\
                   bananas-stats - 1.1.0\n\
                   bananas-webadmin - 1.1.0\n";
        let rows = parse_list_installed(raw);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].name, "bananas-webadmin");
        assert_eq!(rows[2].version, "1.1.0");
    }

    #[test]
    fn rejects_bogus_package_names() {
        assert!(validate_package_name("").is_err());
        assert!(validate_package_name("Bananas-Server").is_err());
        assert!(validate_package_name("../etc/passwd").is_err());
        assert!(validate_package_name("1stpackage").is_err());
        assert!(validate_package_name("foo bar").is_err());
        assert!(validate_package_name("foo;rm -rf").is_err());

        validate_package_name("bananas-server").unwrap();
        validate_package_name("a").unwrap();
        validate_package_name("a1").unwrap();
        validate_package_name("foo-bar-123").unwrap();
    }

    #[tokio::test]
    #[serial_test::serial(opkg_env)]
    async fn update_collects_stdout_and_stderr() {
        let (_td, path) = fake_opkg(
            "Downloading https://example.com/Packages.gz\n",
            "Inflating: 32 packages\n",
            0,
        );
        unsafe {
            std::env::set_var("BANANAS_OPKG_BIN", &path);
        }
        let out = update().await.unwrap();
        assert!(out.contains("Downloading"));
        assert!(out.contains("Inflating"));
    }

    #[tokio::test]
    #[serial_test::serial(opkg_env)]
    async fn list_upgradable_parses_stdout() {
        let (_td, path) = fake_opkg(
            "bananas-stats - 1.0.0 - 1.1.0\nbananas-config - 1.0.0 - 1.1.0\n",
            "",
            0,
        );
        unsafe {
            std::env::set_var("BANANAS_OPKG_BIN", &path);
        }
        let rows = list_upgradable().await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "bananas-stats");
        assert_eq!(rows[1].candidate, "1.1.0");
    }

    #[tokio::test]
    #[serial_test::serial(opkg_log)]
    async fn upgrade_status_idle_when_log_missing() {
        let td = tempfile::tempdir().unwrap();
        let log = td.path().join("nope.log");
        unsafe {
            std::env::set_var("BANANAS_OPKG_LOG", &log);
        }
        let s = upgrade_status(0).await.unwrap();
        assert_eq!(s.state, "idle");
        assert_eq!(s.log_offset, 0);
        assert!(s.exit_code.is_none());
    }

    #[tokio::test]
    #[serial_test::serial(opkg_log)]
    async fn upgrade_status_active_while_running() {
        let td = tempfile::tempdir().unwrap();
        let log = td.path().join("opkg.log");
        std::fs::write(&log, b"Downloading bananas-server.ipk\n").unwrap();
        unsafe {
            std::env::set_var("BANANAS_OPKG_LOG", &log);
        }
        let s = upgrade_status(0).await.unwrap();
        assert_eq!(s.state, "active");
        assert!(s.exit_code.is_none());
        assert!(s.log.contains("Downloading"));
        assert!(s.log_offset > 0);
    }

    #[tokio::test]
    #[serial_test::serial(opkg_log)]
    async fn upgrade_status_done_when_exit_marker_zero() {
        let td = tempfile::tempdir().unwrap();
        let log = td.path().join("opkg.log");
        std::fs::write(&log, b"Configuring bananas-stats.\n[bananas-opkg-exit=0]\n").unwrap();
        unsafe {
            std::env::set_var("BANANAS_OPKG_LOG", &log);
        }
        let s = upgrade_status(0).await.unwrap();
        assert_eq!(s.state, "done");
        assert_eq!(s.exit_code, Some(0));
    }

    #[tokio::test]
    #[serial_test::serial(opkg_log)]
    async fn upgrade_status_failed_when_exit_marker_nonzero() {
        let td = tempfile::tempdir().unwrap();
        let log = td.path().join("opkg.log");
        std::fs::write(&log, b"Cannot satisfy dependency.\n[bananas-opkg-exit=1]\n").unwrap();
        unsafe {
            std::env::set_var("BANANAS_OPKG_LOG", &log);
        }
        let s = upgrade_status(0).await.unwrap();
        assert_eq!(s.state, "failed");
        assert_eq!(s.exit_code, Some(1));
    }

    #[tokio::test]
    #[serial_test::serial(opkg_log)]
    async fn upgrade_status_returns_delta_from_since() {
        let td = tempfile::tempdir().unwrap();
        let log = td.path().join("opkg.log");
        let body = b"line one\nline two\nline three\n";
        std::fs::write(&log, body).unwrap();
        unsafe {
            std::env::set_var("BANANAS_OPKG_LOG", &log);
        }
        let s1 = upgrade_status(0).await.unwrap();
        assert_eq!(s1.log_offset, body.len() as u64);
        assert_eq!(s1.log, "line one\nline two\nline three\n");

        let s2 = upgrade_status(s1.log_offset).await.unwrap();
        assert_eq!(s2.log, "");
        assert_eq!(s2.log_offset, body.len() as u64);

        let mid = "line one\n".len() as u64;
        let s3 = upgrade_status(mid).await.unwrap();
        assert_eq!(s3.log, "line two\nline three\n");
    }
}
