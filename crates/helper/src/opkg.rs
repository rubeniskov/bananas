//! opkg shell-out + parser. Replaces the custom in-place update flow
//! with thin wrappers around the package manager that's already on
//! the image.
//!
//! All four commands (`update`, `list-upgradable`, `list-installed`,
//! `upgrade`) call out to `/usr/bin/opkg` (overridable via the
//! `BANANAS_OPKG_BIN` env var for tests) and capture combined
//! stdout+stderr. `list-*` parse opkg's `name - version[ - candidate]`
//! output into structured JSON; the others return raw text.
//!
//! Package names supplied to `upgrade` are validated against
//! `[a-z][a-z0-9-]*` before they ever reach argv. opkg itself would
//! reject malformed names, but pre-validating keeps the helper's
//! audit trail honest about what's being executed as root.
//!
//! Streaming progress (per-line SSE during a long upgrade) is *not*
//! implemented here — opkg runs are <60s for our package set, and
//! the existing helper IPC is single-shot. If progress UX becomes a
//! priority the protocol can grow a streaming variant; for v1, the
//! single-shot collect is the right shape.

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

fn opkg_bin() -> String {
    std::env::var("BANANAS_OPKG_BIN").unwrap_or_else(|_| "/usr/bin/opkg".into())
}

/// `opkg update` — refresh feed indices from /etc/opkg/customfeeds.conf.
pub async fn update() -> Result<String> {
    run(&["update"]).await
}

/// `opkg list-upgradable` parsed into structured rows.
///
/// opkg's output format is one row per package:
///   `<name> - <installed> - <candidate>`
/// (separated by " - "). Malformed lines are skipped silently — the
/// helper response stays usable even if opkg ever changes its format.
pub async fn list_upgradable() -> Result<Vec<UpgradablePackage>> {
    let raw = run(&["list-upgradable"]).await?;
    Ok(parse_list_upgradable(&raw))
}

/// `opkg list-installed` parsed into `{name, version}` rows.
pub async fn list_installed() -> Result<Vec<InstalledPackage>> {
    let raw = run(&["list-installed"]).await?;
    Ok(parse_list_installed(&raw))
}

/// `opkg upgrade <packages...>`. Validates each name before exec.
pub async fn upgrade(packages: &[String]) -> Result<String> {
    if packages.is_empty() {
        bail!("opkg upgrade: no packages specified");
    }
    for p in packages {
        validate_package_name(p)?;
    }
    let mut args: Vec<&str> = vec!["upgrade"];
    args.extend(packages.iter().map(|s| s.as_str()));
    run(&args).await
}

/// Spawn opkg with the given args, capture combined stdout+stderr,
/// fail on non-zero exit. Output goes back via Response::output.
async fn run(args: &[&str]) -> Result<String> {
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
        assert!(validate_package_name("Bananas-Server").is_err()); // uppercase
        assert!(validate_package_name("../etc/passwd").is_err());
        assert!(validate_package_name("1stpackage").is_err()); // starts with digit
        assert!(validate_package_name("foo bar").is_err()); // space
        assert!(validate_package_name("foo;rm -rf").is_err()); // shell chars

        // happy path
        validate_package_name("bananas-server").unwrap();
        validate_package_name("a").unwrap();
        validate_package_name("a1").unwrap();
        validate_package_name("foo-bar-123").unwrap();
    }

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
    async fn upgrade_propagates_failure() {
        let (_td, path) = fake_opkg("", "Cannot satisfy dependencies\n", 1);
        unsafe {
            std::env::set_var("BANANAS_OPKG_BIN", &path);
        }
        let result = upgrade(&["bananas-stats".to_string()]).await;
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("Cannot satisfy"), "got: {err}");
    }

    #[tokio::test]
    #[serial_test::serial(opkg_env)]
    async fn upgrade_rejects_invalid_package_name_before_exec() {
        // No fake opkg installed — validation must short-circuit
        // before any spawn attempt.
        unsafe {
            std::env::set_var("BANANAS_OPKG_BIN", "/nonexistent/opkg");
        }
        // Path-traversal: starts with `.`, fails the "must start with
        // a lowercase letter" check.
        let result = upgrade(&["../etc/passwd".to_string()]).await;
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("must start with a lowercase letter"),
            "got: {err}"
        );

        // Embedded shell metachar: passes the leading-letter check,
        // fails on the per-character validator.
        let result = upgrade(&["foo;rm-rf".to_string()]).await;
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("invalid character"), "got: {err}");
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
}
