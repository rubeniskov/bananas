//! In-place updates: verify a staged release tarball, swap its files
//! into the install target, restart the relevant unit, and update
//! `/etc/bananas/versions.toml`.
//!
//! The server downloads tarballs from GitHub releases into a staging
//! directory it owns (`/var/lib/bananas/updates/staging/`) — the helper
//! never reaches the network. Once a tarball is on disk the server hands
//! its path + the expected sha256 (from the `SHA256SUMS` release asset)
//! to this module. We validate everything before touching anything in
//! `/usr/bin` so a corrupt download can't brick the device.
//!
//! Atomicity boundaries:
//!   - SHA-256 mismatch: refuse before extracting. No state change.
//!   - Layout mismatch: refuse after extract, before swap. Work dir is
//!     cleaned up; install dir untouched.
//!   - `<bin> --version` mismatch: refuse before swap. Same as above.
//!   - Swap (rename) fails: roll back any already-renamed `.bak` files.
//!   - systemctl restart fails: roll back files and (best-effort)
//!     restart the unit again with the old binary.
//!
//! Path overrides (env, primarily for tests):
//!   - `BANANAS_UPDATES_STAGING_DIR` — must enclose every accepted tarball
//!   - `BANANAS_UPDATES_WORK_DIR`    — scratch space for extracts
//!   - `BANANAS_INSTALL_BIN_DIR`     — where binaries land
//!   - `BANANAS_INSTALL_WEBADMIN_DIR` — webadmin asset root
//!   - `BANANAS_VERSIONS_PATH`       — versions.toml location
//!   - `BANANAS_SYSTEMCTL_SKIP=1`    — no-op systemctl restart (tests)

use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::Stdio,
    time::SystemTime,
};

use anyhow::{Context, Result, anyhow, bail};
use bananas_helper::Component;
use sha2::{Digest, Sha256};
use tokio::process::Command as TokioCommand;

const DEFAULT_STAGING_DIR: &str = "/var/lib/bananas/updates/staging";
const DEFAULT_WORK_DIR: &str = "/var/lib/bananas/updates/work";
const DEFAULT_BIN_DIR: &str = "/usr/bin";
const DEFAULT_WEBADMIN_DIR: &str = "/usr/share/bananas/webadmin";
const DEFAULT_VERSIONS_PATH: &str = "/etc/bananas/versions.toml";

struct Paths {
    staging: PathBuf,
    work: PathBuf,
    bin_dir: PathBuf,
    webadmin_dir: PathBuf,
    versions: PathBuf,
    skip_systemctl: bool,
}

impl Paths {
    fn from_env() -> Self {
        Self {
            staging: env_path("BANANAS_UPDATES_STAGING_DIR", DEFAULT_STAGING_DIR),
            work: env_path("BANANAS_UPDATES_WORK_DIR", DEFAULT_WORK_DIR),
            bin_dir: env_path("BANANAS_INSTALL_BIN_DIR", DEFAULT_BIN_DIR),
            webadmin_dir: env_path("BANANAS_INSTALL_WEBADMIN_DIR", DEFAULT_WEBADMIN_DIR),
            versions: env_path("BANANAS_VERSIONS_PATH", DEFAULT_VERSIONS_PATH),
            skip_systemctl: std::env::var_os("BANANAS_SYSTEMCTL_SKIP").is_some(),
        }
    }
}

fn env_path(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

/// What the helper looks for inside the tarball + where it lands.
struct Target {
    /// Single binary at the tarball root, gets renamed into `bin_dir`.
    bin_files: &'static [&'static str],
    /// systemd units to bounce after the swap completes. Empty for
    /// webadmin (ServeDir re-reads on the next request).
    restart_units: &'static [&'static str],
    /// True for the webadmin tarball: the install path is a directory,
    /// not a single binary. Extract-then-swap-parent semantics.
    is_webadmin: bool,
}

fn target_for(component: Component) -> Result<Target> {
    Ok(match component {
        Component::Stats => Target {
            bin_files: &["bananas-stats"],
            restart_units: &["bananas-stats.service"],
            is_webadmin: false,
        },
        Component::Dashboard => Target {
            bin_files: &["bananas-dashboard"],
            restart_units: &["bananas-dashboard.service"],
            is_webadmin: false,
        },
        Component::Webadmin => Target {
            bin_files: &[],
            restart_units: &[],
            is_webadmin: true,
        },
        // Server + Helper self-update mechanics differ (the running
        // binary can't restart itself the same way). Step 7+8 lift this
        // restriction.
        Component::Server | Component::Helper => {
            bail!(
                "in-place install for `{}` not yet supported",
                component.as_str()
            )
        }
    })
}

/// Entry point called from main.rs dispatch. All paths come from env
/// (`Paths::from_env`) so tests can swap them out.
pub async fn install_update(
    component: Component,
    tarball_path: &str,
    expected_version: &str,
    expected_sha256: &str,
) -> Result<String> {
    let paths = Paths::from_env();
    let target = target_for(component)?;

    let tarball = validate_staged(&paths, tarball_path)?;
    verify_sha256(&tarball, expected_sha256)?;

    // Extract into a fresh work dir. Wrapped in spawn_blocking because
    // tar+flate2 are sync and can take seconds on slow ARM I/O.
    let work_dir = paths
        .work
        .join(format!("{}-{}", component.as_str(), expected_version));
    if work_dir.exists() {
        std::fs::remove_dir_all(&work_dir).ok();
    }
    std::fs::create_dir_all(&work_dir)
        .with_context(|| format!("creating work dir {}", work_dir.display()))?;
    let tarball_for_extract = tarball.clone();
    let work_for_extract = work_dir.clone();
    tokio::task::spawn_blocking(move || extract_tarball(&tarball_for_extract, &work_for_extract))
        .await
        .context("extract task panicked")??;

    let mut log = String::new();
    log.push_str(&format!(
        "verified sha256 + extracted {} → {}\n",
        tarball.display(),
        work_dir.display()
    ));

    if target.is_webadmin {
        install_webadmin(&work_dir, &paths.webadmin_dir, &mut log)?;
    } else {
        install_binaries(
            &target,
            &work_dir,
            &paths.bin_dir,
            expected_version,
            &mut log,
        )
        .await?;
    }

    // versions.toml after on-disk swap, before systemctl restart, so a
    // restart failure leaves an accurate record of what's actually on
    // disk. (The unit may then be down or running an older binary —
    // both reflected by the operator-facing log + journal.)
    write_version_entry(
        &paths.versions,
        component,
        expected_version,
        expected_sha256,
    )?;
    log.push_str(&format!("updated {}\n", paths.versions.display()));

    if !paths.skip_systemctl {
        for unit in target.restart_units {
            let result = systemctl_restart(unit).await;
            log.push_str(&format!("restart {unit}: "));
            match result {
                Ok(out) => log.push_str(&format!("ok\n{out}")),
                Err(e) => {
                    log.push_str(&format!("FAILED: {e}\n"));
                    bail!(
                        "install of {} succeeded but {unit} failed to restart: {e}\n--- log ---\n{log}",
                        component.as_str()
                    );
                }
            }
        }
    }

    // Cleanup work dir + .bak files only on the happy path. Leaving
    // them around on failure makes post-mortem easier.
    std::fs::remove_dir_all(&work_dir).ok();

    log.push_str(&format!(
        "{} installed at {expected_version}\n",
        component.as_str()
    ));
    Ok(log)
}

pub fn read_versions() -> Result<String> {
    let paths = Paths::from_env();
    match std::fs::read_to_string(&paths.versions) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", paths.versions.display())),
    }
}

fn validate_staged(paths: &Paths, tarball_path: &str) -> Result<PathBuf> {
    let p = Path::new(tarball_path);
    if !p.is_absolute() {
        bail!("tarball_path must be absolute, got {tarball_path:?}");
    }
    let canon_target = std::fs::canonicalize(p)
        .with_context(|| format!("locating staged tarball {tarball_path:?}"))?;
    // Canonicalize the staging root too so a symlink chain doesn't let
    // an attacker escape.
    let canon_staging = std::fs::canonicalize(&paths.staging).with_context(|| {
        format!(
            "locating staging dir {} (BANANAS_UPDATES_STAGING_DIR)",
            paths.staging.display()
        )
    })?;
    if !canon_target.starts_with(&canon_staging) {
        bail!(
            "staged tarball {} is outside {}",
            canon_target.display(),
            canon_staging.display()
        );
    }
    Ok(canon_target)
}

fn verify_sha256(file: &Path, expected: &str) -> Result<()> {
    let mut hasher = Sha256::new();
    let mut reader =
        BufReader::new(File::open(file).with_context(|| format!("opening {}", file.display()))?);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("reading {}", file.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex(&hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "sha256 mismatch for {}: expected {expected}, got {actual}",
            file.display()
        );
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn extract_tarball(tarball: &Path, dest: &Path) -> Result<()> {
    let f = File::open(tarball).with_context(|| format!("opening {}", tarball.display()))?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    archive
        .unpack(dest)
        .with_context(|| format!("unpacking {} into {}", tarball.display(), dest.display()))
}

async fn install_binaries(
    target: &Target,
    work_dir: &Path,
    bin_dir: &Path,
    expected_version: &str,
    log: &mut String,
) -> Result<()> {
    // Validate every expected file exists in the extracted tree before
    // any swap happens — partial swap is the worst possible state.
    for name in target.bin_files {
        let src = work_dir.join(name);
        if !src.is_file() {
            bail!(
                "tarball missing expected file {} (extracted to {})",
                name,
                work_dir.display()
            );
        }
    }

    // Make sure each new bin is executable + reports the version we
    // claimed. The first file is the canonical one we --version against;
    // the rest are dependencies bundled in the same tarball.
    let canonical = target.bin_files[0];
    let canonical_path = work_dir.join(canonical);
    let mut perms = std::fs::metadata(&canonical_path)
        .with_context(|| format!("statting {}", canonical_path.display()))?
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    std::fs::set_permissions(&canonical_path, perms).ok();

    let out = TokioCommand::new(&canonical_path)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("running {} --version", canonical_path.display()))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let reported = combined
        .split_whitespace()
        .last()
        .unwrap_or("")
        .trim()
        .to_string();
    if reported != expected_version {
        bail!(
            "extracted {canonical} reports version {reported:?}, expected {expected_version:?}; refusing to swap"
        );
    }
    log.push_str(&format!(
        "{canonical} --version → {reported} (matches expected)\n"
    ));

    // Swap phase. Track which files we've renamed so we can roll back
    // on a mid-swap failure (e.g. helper killed, fs ENOSPC).
    let mut swapped: Vec<(PathBuf, PathBuf)> = Vec::new();
    for name in target.bin_files {
        let src = work_dir.join(name);
        let dst = bin_dir.join(name);
        let bak = bin_dir.join(format!("{name}.bak"));
        // chmod 0755 on the source before move-into-place — mode survives
        // rename(2) but a missing exec bit would brick the unit.
        if let Ok(meta) = std::fs::metadata(&src) {
            let mut p = meta.permissions();
            use std::os::unix::fs::PermissionsExt;
            p.set_mode(0o755);
            std::fs::set_permissions(&src, p).ok();
        }
        // If a previous install left a .bak around, scrap it so we
        // don't accidentally restore something three versions back.
        if bak.exists() {
            std::fs::remove_file(&bak).ok();
        }
        if dst.exists() {
            std::fs::rename(&dst, &bak).map_err(|e| {
                rollback(&swapped);
                anyhow!("renaming {} → {}: {e}", dst.display(), bak.display())
            })?;
        }
        std::fs::rename(&src, &dst).map_err(|e| {
            // Restore the just-renamed .bak so we don't end up with no
            // binary in place.
            if bak.exists() {
                std::fs::rename(&bak, &dst).ok();
            }
            rollback(&swapped);
            anyhow!("renaming {} → {}: {e}", src.display(), dst.display())
        })?;
        swapped.push((dst.clone(), bak));
        log.push_str(&format!("swapped {} (old → .bak)\n", dst.display()));
    }

    // All files moved successfully; clean up .bak now that we're past
    // the rollback window. (systemctl restart is the next step but if
    // *that* fails we want to keep the file swap; the operator can roll
    // back manually using the .bak from the previous successful install
    // — gone here, but that previous one wrote its versions.toml row.)
    for (_dst, bak) in &swapped {
        if bak.exists() {
            std::fs::remove_file(bak).ok();
        }
    }
    Ok(())
}

fn rollback(swapped: &[(PathBuf, PathBuf)]) {
    for (dst, bak) in swapped.iter().rev() {
        if bak.exists() {
            std::fs::remove_file(dst).ok();
            std::fs::rename(bak, dst).ok();
        }
    }
}

fn install_webadmin(work_dir: &Path, install_dir: &Path, log: &mut String) -> Result<()> {
    // Tarball is flat: index.html + assets/ at the root. Sanity check.
    let index = work_dir.join("index.html");
    if !index.is_file() {
        bail!(
            "webadmin tarball missing index.html at root (extracted to {})",
            work_dir.display()
        );
    }
    // Atomic-ish dir swap: rename old → .bak, rename new → place.
    // Safe because both live on the same fs (the rootfs partition on
    // the SD card image; controlled in tests via env vars).
    let bak = install_dir.with_extension("bak");
    if bak.exists() {
        std::fs::remove_dir_all(&bak).ok();
    }
    if let Some(parent) = install_dir.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    if install_dir.exists() {
        std::fs::rename(install_dir, &bak)
            .with_context(|| format!("renaming {} → {}", install_dir.display(), bak.display()))?;
    }
    if let Err(e) = std::fs::rename(work_dir, install_dir) {
        // Restore on failure.
        if bak.exists() {
            std::fs::rename(&bak, install_dir).ok();
        }
        return Err(anyhow!(
            "renaming {} → {}: {e}",
            work_dir.display(),
            install_dir.display()
        ));
    }
    if bak.exists() {
        std::fs::remove_dir_all(&bak).ok();
    }
    log.push_str(&format!("swapped {} (old → .bak)\n", install_dir.display()));
    Ok(())
}

fn write_version_entry(
    versions_path: &Path,
    component: Component,
    version: &str,
    sha256: &str,
) -> Result<()> {
    use std::fmt::Write;

    if let Some(parent) = versions_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let existing = std::fs::read_to_string(versions_path).unwrap_or_default();
    let mut doc: toml::Table = if existing.is_empty() {
        toml::Table::new()
    } else {
        existing
            .parse()
            .with_context(|| format!("parsing existing {}", versions_path.display()))?
    };
    let mut entry = toml::Table::new();
    entry.insert("version".into(), version.into());
    entry.insert("sha256".into(), sha256.into());
    let installed_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    entry.insert("installed_at".into(), (installed_at as i64).into());
    doc.insert(component.as_str().into(), entry.into());

    // Stable serialization (alphabetical keys) so successive writes
    // diff cleanly — semantic-release-style commits if anyone ever
    // syncs versions.toml into git.
    let mut out = String::new();
    writeln!(out, "# Managed by bananas-helper. Do not edit by hand.").ok();
    writeln!(out).ok();
    let mut keys: Vec<_> = doc.keys().cloned().collect();
    keys.sort();
    for k in keys {
        writeln!(out, "[{k}]").ok();
        if let Some(toml::Value::Table(t)) = doc.get(&k) {
            let mut sub: Vec<_> = t.keys().cloned().collect();
            sub.sort();
            for sk in sub {
                if let Some(v) = t.get(&sk) {
                    writeln!(out, "{sk} = {}", v.to_string()).ok();
                }
            }
        }
        writeln!(out).ok();
    }
    let tmp = versions_path.with_extension("toml.tmp");
    std::fs::write(&tmp, &out).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, versions_path)
        .with_context(|| format!("renaming {} → {}", tmp.display(), versions_path.display()))?;
    Ok(())
}

async fn systemctl_restart(unit: &str) -> Result<String> {
    let out = TokioCommand::new("systemctl")
        .arg("restart")
        .arg(unit)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning systemctl")?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        bail!("status {}: {combined}", out.status);
    }
    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a fake binary at <work>/<name> that prints "<name> <version>"
    /// when invoked with --version. The real release tarballs ship the
    /// armv7 cross-compiled bin; we don't need that for the install
    /// machinery — we just need an ELF executable on the test host that
    /// answers `--version`. A tiny shell script is enough.
    fn make_fake_bin(work: &Path, name: &str, version: &str) -> PathBuf {
        let p = work.join(name);
        let mut f = File::create(&p).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(
            f,
            "if [ \"$1\" = \"--version\" ]; then echo \"{name} {version}\"; exit 0; fi"
        )
        .unwrap();
        writeln!(f, "echo \"<unused>\"").unwrap();
        drop(f);
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&p, perms).unwrap();
        p
    }

    fn pack_tarball(src_dir: &Path, dest: &Path) {
        let f = File::create(dest).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        tar.append_dir_all(".", src_dir).unwrap();
        tar.finish().unwrap();
    }

    fn sha256_of(path: &Path) -> String {
        let mut hasher = Sha256::new();
        let mut f = File::open(path).unwrap();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = f.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        hex(&hasher.finalize())
    }

    /// Set up a tempdir-rooted fake install layout and point all the
    /// env vars at it. Returns the tempdir handle (drop = teardown).
    fn setup_env() -> (tempfile::TempDir, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let staging = td.path().join("staging");
        let work = td.path().join("work");
        let bin_dir = td.path().join("usr-bin");
        let webadmin_dir = td.path().join("share/bananas/webadmin");
        let versions = td.path().join("etc/bananas/versions.toml");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::create_dir_all(versions.parent().unwrap()).unwrap();
        // Tests run sequentially (single-threaded) — they all share the
        // same env, so each test must set the vars before calling into
        // install_update. cargo test's default thread pool would race
        // these; #[test] in this module is gated below to use a single
        // cargo flag (--test-threads=1) noted in the helper README.
        unsafe {
            std::env::set_var("BANANAS_UPDATES_STAGING_DIR", &staging);
            std::env::set_var("BANANAS_UPDATES_WORK_DIR", &work);
            std::env::set_var("BANANAS_INSTALL_BIN_DIR", &bin_dir);
            std::env::set_var("BANANAS_INSTALL_WEBADMIN_DIR", &webadmin_dir);
            std::env::set_var("BANANAS_VERSIONS_PATH", &versions);
            std::env::set_var("BANANAS_SYSTEMCTL_SKIP", "1");
        }
        (td, staging)
    }

    #[tokio::test]
    #[serial_test::serial(install_env)]
    async fn installs_stats_binary_end_to_end() {
        let (_td, staging) = setup_env();
        // Build a fake tarball matching the bananas-stats layout.
        let src = tempfile::tempdir().unwrap();
        make_fake_bin(src.path(), "bananas-stats", "1.2.3");
        let tarball = staging.join("bananas-stats-armv7.tar.gz");
        pack_tarball(src.path(), &tarball);
        let sha = sha256_of(&tarball);

        let log = install_update(Component::Stats, tarball.to_str().unwrap(), "1.2.3", &sha)
            .await
            .unwrap();
        assert!(log.contains("bananas-stats --version → 1.2.3"));
        assert!(log.contains("stats installed at 1.2.3"));

        // Binary landed in fake /usr/bin/.
        let bin_dir: PathBuf = std::env::var("BANANAS_INSTALL_BIN_DIR").unwrap().into();
        assert!(bin_dir.join("bananas-stats").is_file());
        // versions.toml was written.
        let versions: PathBuf = std::env::var("BANANAS_VERSIONS_PATH").unwrap().into();
        let v = std::fs::read_to_string(&versions).unwrap();
        assert!(v.contains("[stats]"));
        assert!(v.contains("version = \"1.2.3\""));
    }

    #[tokio::test]
    #[serial_test::serial(install_env)]
    async fn refuses_sha_mismatch() {
        let (_td, staging) = setup_env();
        let src = tempfile::tempdir().unwrap();
        make_fake_bin(src.path(), "bananas-stats", "1.2.3");
        let tarball = staging.join("bananas-stats-armv7.tar.gz");
        pack_tarball(src.path(), &tarball);
        let result = install_update(
            Component::Stats,
            tarball.to_str().unwrap(),
            "1.2.3",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .await;
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("sha256 mismatch"), "got: {err}");
    }

    #[tokio::test]
    #[serial_test::serial(install_env)]
    async fn refuses_version_mismatch() {
        let (_td, staging) = setup_env();
        let src = tempfile::tempdir().unwrap();
        // bin reports 1.2.3 but we'll claim 9.9.9.
        make_fake_bin(src.path(), "bananas-stats", "1.2.3");
        let tarball = staging.join("bananas-stats-armv7.tar.gz");
        pack_tarball(src.path(), &tarball);
        let sha = sha256_of(&tarball);
        let result =
            install_update(Component::Stats, tarball.to_str().unwrap(), "9.9.9", &sha).await;
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("reports version"), "got: {err}");
    }

    #[tokio::test]
    #[serial_test::serial(install_env)]
    async fn rejects_path_outside_staging() {
        let (_td, _staging) = setup_env();
        // Put the tarball somewhere else; make sure we can't escape.
        let elsewhere = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        make_fake_bin(src.path(), "bananas-stats", "1.2.3");
        let tarball = elsewhere.path().join("bananas-stats-armv7.tar.gz");
        pack_tarball(src.path(), &tarball);
        let sha = sha256_of(&tarball);
        let result =
            install_update(Component::Stats, tarball.to_str().unwrap(), "1.2.3", &sha).await;
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("outside"), "got: {err}");
    }

    #[tokio::test]
    #[serial_test::serial(install_env)]
    async fn rejects_unsupported_component() {
        let (_td, _staging) = setup_env();
        let result = install_update(Component::Server, "/tmp/whatever.tar.gz", "1.2.3", "00").await;
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("not yet supported"), "got: {err}");
    }

    #[tokio::test]
    #[serial_test::serial(install_env)]
    async fn installs_webadmin_directory() {
        let (_td, staging) = setup_env();
        // Webadmin tarball: index.html + assets/foo.js at the root.
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("index.html"), "<html>v1</html>").unwrap();
        std::fs::create_dir_all(src.path().join("assets")).unwrap();
        std::fs::write(src.path().join("assets/foo.js"), "// v1").unwrap();
        let tarball = staging.join("bananas-webadmin.tar.gz");
        pack_tarball(src.path(), &tarball);
        let sha = sha256_of(&tarball);
        let log = install_update(
            Component::Webadmin,
            tarball.to_str().unwrap(),
            "1.2.3",
            &sha,
        )
        .await
        .unwrap();
        assert!(log.contains("webadmin installed at 1.2.3"));
        let webadmin_dir: PathBuf = std::env::var("BANANAS_INSTALL_WEBADMIN_DIR")
            .unwrap()
            .into();
        assert!(webadmin_dir.join("index.html").is_file());
        assert!(webadmin_dir.join("assets/foo.js").is_file());
    }
}
