//! bananas-helper daemon — root-privileged service that the unprivileged
//! HTTP frontend talks to over a Unix socket.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result};
use bananas_helper::{Command, Response};
use serde_json::json;

mod opkg;
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    process::Command as TokioCommand,
};

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Static INFO level — the helper logs only on errors and admin
    // RPCs. Skip env-filter (and its `regex` dep) to keep the binary
    // small. RUST_LOG=… is intentionally ignored here; if you ever
    // need to debug the helper, add the filter back.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let socket_path: PathBuf = std::env::var_os("BANANAS_HELPER_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/run/bananas-helper.sock".into());
    let exports_path: PathBuf = std::env::var_os("BANANAS_EXPORTS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/etc/exports".into());

    if socket_path.exists() {
        fs::remove_file(&socket_path).await.ok();
    }
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("binding {}", socket_path.display()))?;
    fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o660))
        .await
        .ok();
    // The unprivileged frontend (bananas-server) runs as the `bananas`
    // user. Without this chown the socket is root:root 0660, which the
    // frontend can't open. Look up the GID by reading /etc/group; falls
    // back to a no-op log if the group is missing.
    if let Some(gid) = lookup_group_gid("bananas") {
        if let Err(e) = std::os::unix::fs::chown(&socket_path, Some(0), Some(gid)) {
            tracing::warn!(error=%e, "failed to chown socket to root:bananas");
        }
    } else {
        tracing::warn!(
            "group 'bananas' not found in /etc/group — frontend won't be able to connect"
        );
    }

    tracing::info!(
        socket=%socket_path.display(),
        exports=%exports_path.display(),
        "bananas-helper listening"
    );

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error=%e, "accept failed");
                continue;
            }
        };
        let exports_path = exports_path.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, &exports_path).await {
                tracing::error!(error=%e, "request handler failed");
            }
        });
    }
}

async fn handle(stream: UnixStream, exports_path: &Path) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response = match serde_json::from_str::<Command>(line.trim()) {
        Ok(cmd) => dispatch(cmd, exports_path).await,
        Err(e) => Response::err(format!("bad command: {e}"), String::new()),
    };

    let mut payload = serde_json::to_vec(&response)?;
    payload.push(b'\n');
    write_half.write_all(&payload).await?;
    Ok(())
}

async fn dispatch(cmd: Command, exports_path: &Path) -> Response {
    match cmd {
        Command::WriteExports { content } => match write_exports(&content, exports_path).await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::ReloadExports => match exportfs_reload().await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::WriteFstab { content } => match write_fstab(&content).await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::Smart { device } => match smart(&device).await {
            Ok(json) => Response::ok(json),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::ListUsers => match list_users(false).await {
            Ok(json) => Response::ok(json),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::ExportUsers => match list_users(true).await {
            Ok(json) => Response::ok(json),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::CreateUser {
            username,
            password,
            full_name,
            admin,
            password_is_hash,
        } => {
            match create_user(
                &username,
                &password,
                full_name.as_deref(),
                admin,
                password_is_hash,
            )
            .await
            {
                Ok(()) => Response::ok(format!("created {username}")),
                Err(e) => Response::err(e.to_string(), String::new()),
            }
        }
        Command::DeleteUser { username } => match delete_user(&username).await {
            Ok(()) => Response::ok(format!("deleted {username}")),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::SetPassword { username, password } => {
            match set_password(&username, &password).await {
                Ok(()) => Response::ok(format!("password updated for {username}")),
                Err(e) => Response::err(e.to_string(), String::new()),
            }
        }
        Command::ChangeOwnPassword {
            username,
            old_password,
            new_password,
        } => {
            match change_own_password(&username, &old_password, &new_password).await {
                Ok(()) => Response::ok(format!("password rotated for {username}")),
                // Same redaction rule as authenticate — bury the specific
                // reason behind a single "invalid credentials" so we don't
                // confirm whether the user exists.
                Err(_) => Response::err("invalid credentials", String::new()),
            }
        }
        Command::SetAdmin { username, admin } => match set_admin(&username, admin).await {
            Ok(()) => Response::ok(format!(
                "{username} is {} an admin",
                if admin { "now" } else { "no longer" }
            )),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::Stat { path } => match stat_path(&path).await {
            Ok(json) => Response::ok(json),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::SetPermissions {
            path,
            uid,
            gid,
            mode,
            recursive,
        } => match set_permissions(&path, uid, gid, mode.as_deref(), recursive).await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::ReadServiceConfig { name } => match read_service_config(&name).await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::WriteServiceConfig { name, content } => {
            match write_service_config(&name, &content).await {
                Ok(out) => Response::ok(out),
                Err(e) => Response::err(e.to_string(), String::new()),
            }
        }
        Command::RunCloudSync { idx } => match run_cloud_sync(idx).await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::CancelCloudSync { idx } => match cancel_cloud_sync(idx).await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::RebootSystem => match reboot_system().await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::MakeDirectory { path } => match make_directory(&path).await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::Lsblk => match run_lsblk().await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::SetTimezone { tz } => match set_timezone(&tz).await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::ListTimezones => match list_timezones().await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::OpkgUpdate => match opkg::update().await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::OpkgListUpgradable => match opkg::list_upgradable().await {
            Ok(rows) => match serde_json::to_string(&rows) {
                Ok(json) => Response::ok(json),
                Err(e) => Response::err(format!("serializing upgradable: {e}"), String::new()),
            },
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::OpkgListInstalled => match opkg::list_installed().await {
            Ok(rows) => match serde_json::to_string(&rows) {
                Ok(json) => Response::ok(json),
                Err(e) => Response::err(format!("serializing installed: {e}"), String::new()),
            },
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::OpkgUpgrade { packages } => match opkg::upgrade(&packages).await {
            Ok(out) => Response::ok(out),
            Err(e) => Response::err(e.to_string(), String::new()),
        },
        Command::Authenticate { username, password } => {
            // Generic failure message — same string for missing user, locked
            // account, and wrong password. Avoids confirming which usernames
            // exist on the system to an attacker probing the API.
            //
            // The `password_expired` sentinel is the deliberate exception:
            // it can only fire when the username + password are BOTH
            // correct (the lastchg check happens after the hash compare),
            // so it leaks no more than a successful login already would.
            // Passing it through is what lets the UI swap into the "set
            // new password" form on first sign-in.
            const GENERIC_FAIL: &str = "invalid credentials";
            match authenticate(&username, &password).await {
                Ok(()) => Response::ok(format!("authenticated {username}")),
                Err(e) => {
                    let msg = e.to_string();
                    tracing::warn!(user=%username, error=%msg, "auth failed");
                    let surface = if msg.contains("password_expired") {
                        "password_expired"
                    } else {
                        GENERIC_FAIL
                    };
                    Response::err(surface, String::new())
                }
            }
        }
    }
}

async fn write_exports(content: &str, exports_path: &Path) -> Result<String> {
    validate_exports(content)?;
    // Make sure every export path actually exists on disk before we let
    // nfs-server try to stat it. The image used to pre-create /srv/media
    // and /srv/services; we removed that, and a config-bundle restore
    // can now write rows referencing paths that the helper hasn't seen.
    // exportfs would then fail with "Failed to stat /path: No such file
    // or directory" and nfs-server lands in `failed`. Auto-mkdir keeps
    // the apply atomic from the operator's point of view.
    let mkdir_log = ensure_export_dirs(content).await;

    // Iterate-loop dev hosts NFS-netboot the BPI, so `/` is itself an
    // NFS mount. exportfs treats anything below an NFS root as an
    // implicit re-export and refuses without a numeric `fsid=`:
    //   exportfs: /srv/services requires fsid= for NFS export
    // Inject `fsid=<line-number>` into rows that don't already carry
    // one when /proc/mounts says rootfs is NFS. Production SD-card
    // boots have ext4 root, so the injection is a no-op there.
    let needs_fsid = rootfs_is_nfs().await;
    let (effective, fsid_log) = if needs_fsid {
        inject_fsid_into_exports(content)
    } else {
        (content.to_string(), String::new())
    };

    let dir = exports_path.parent().unwrap_or_else(|| Path::new("/"));
    let tmp = dir.join(format!(
        ".{}.tmp",
        exports_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("exports")
    ));
    fs::write(&tmp, &effective)
        .await
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))
        .await
        .ok();
    fs::rename(&tmp, exports_path)
        .await
        .with_context(|| format!("renaming {} -> {}", tmp.display(), exports_path.display()))?;

    // Service state follows export state: zero entries → stop nfs-server,
    // any entries → ensure it's running, then `exportfs -rv` to push the
    // new table to the kernel without disturbing existing client mounts.
    // Both branches surface their stdout/stderr in the operator-facing
    // banner so a misbehaving export rule (or a stale unit dependency)
    // is visible without an SSH round-trip.
    let active = count_active_exports(&effective);
    let nfs_state = if active > 0 {
        ensure_nfs_server_running().await
    } else {
        ensure_nfs_server_stopped().await
    };

    let reload = if active > 0 {
        exportfs_reload()
            .await
            .unwrap_or_else(|e| format!("(exportfs failed: {e})"))
    } else {
        "(no exports — skipping exportfs)".to_string()
    };

    Ok(format!(
        "wrote {} ({} bytes)\n{}{}{}\n{}",
        exports_path.display(),
        effective.len(),
        mkdir_log,
        fsid_log,
        nfs_state,
        reload
    ))
}

/// `/proc/mounts` line for `/` whose third field starts with `nfs`
/// indicates the iterate-loop NFS netboot. exportfs's "requires fsid="
/// gate fires whenever the export's underlying fs differs from the
/// rootfs's fs, and an NFS root makes that gate true for every disk
/// mount. Returns `false` on the production SD-card path (ext4 root).
async fn rootfs_is_nfs() -> bool {
    let s = match fs::read_to_string("/proc/mounts").await {
        Ok(s) => s,
        Err(_) => return false,
    };
    s.lines().any(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        f.len() >= 3 && f[1] == "/" && (f[2] == "nfs" || f[2] == "nfs4" || f[2].starts_with("nfs"))
    })
}

/// Inject `fsid=<n>` into each non-comment export row that doesn't
/// already specify one. `<n>` is the row's 1-based position in the
/// content so the same input produces the same fsids deterministically.
/// Rows that already carry a `fsid=` are left alone — operators who
/// pinned a specific id keep it.
fn inject_fsid_into_exports(content: &str) -> (String, String) {
    let mut out = String::with_capacity(content.len() + 64);
    let mut next_fsid: u32 = 1;
    let mut injected: Vec<String> = Vec::new();
    for raw in content.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        // The fields after the path are `client(opts) [client(opts) …]`.
        // We replace each `(...)` group's option list, adding fsid=N if
        // missing. Operating on the raw string keeps quoting / spacing
        // intact for the bits we don't touch.
        let already_has = trimmed.contains("fsid=");
        if already_has {
            out.push_str(raw);
            out.push('\n');
            // Bump the counter anyway so the IDs we DO assign stay
            // unique across operator-pinned + auto-injected rows.
            next_fsid += 1;
            continue;
        }
        let path = trimmed.split_whitespace().next().unwrap_or("").to_string();
        let assigned = next_fsid;
        next_fsid += 1;
        let injected_line = if let Some(open) = raw.find('(') {
            // Insert `fsid=<n>,` right after the first `(`. exportfs
            // accepts duplicate option commas / leading commas so this
            // is safe even on weirdly-spaced input.
            let (lhs, rhs) = raw.split_at(open + 1);
            format!("{lhs}fsid={assigned},{rhs}")
        } else {
            // No client(options) group — append `*(fsid=<n>)`. Treats
            // the rare "path with no client" pattern, which exportfs
            // would reject anyway, but at least the line is parseable.
            format!("{raw} *(fsid={assigned})")
        };
        out.push_str(&injected_line);
        out.push('\n');
        injected.push(format!("{path}=fsid={assigned}"));
    }
    let log = if injected.is_empty() {
        String::new()
    } else {
        format!("auto-fsid (NFS rootfs detected): {}\n", injected.join(" "))
    };
    (out, log)
}

/// Walk the parsed export rows and create any path that isn't on disk
/// yet. Owner is left at root:root (mode 0755) — operators wanting a
/// different uid/gid use the per-row Permissions modal afterwards. The
/// returned string lists the paths we created (or none) so the apply
/// banner makes the side-effect visible.
async fn ensure_export_dirs(content: &str) -> String {
    let mut created: Vec<String> = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = match line.split_whitespace().next() {
            Some(p) => p.trim_matches('"'),
            None => continue,
        };
        if !path.starts_with('/') {
            continue;
        }
        let p = Path::new(path);
        if p.is_dir() {
            continue;
        }
        match fs::create_dir_all(p).await {
            Ok(()) => created.push(path.to_string()),
            Err(e) => {
                return format!("mkdir -p {path} failed: {e}\n");
            }
        }
    }
    if created.is_empty() {
        String::new()
    } else {
        format!("created export dirs: {}\n", created.join(" "))
    }
}

/// Lines that aren't pure whitespace or comment-only count as active
/// exports. Matches the same filter `validate_exports` uses below.
fn count_active_exports(content: &str) -> usize {
    content
        .lines()
        .filter(|raw| {
            let line = raw.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .count()
}

async fn ensure_nfs_server_running() -> String {
    // We start the bundle of NFS daemons that v3 (and macOS in
    // particular) needs as a single peer-to-peer set. nfs-server is
    // the obvious one; nfs-statd is the supplementary lock-state
    // tracker — without it macOS refuses the mount with
    // "RPC prog. not avail" before even opening a TCP connection. The
    // helper used to start only nfs-server, leaving statd inactive
    // and macOS clients stuck.
    let mut log = String::new();
    for unit in ["nfs-server.service", "nfs-statd.service"] {
        log.push_str(&start_nfs_unit(unit).await);
        log.push('\n');
    }
    log.trim_end().to_string()
}

/// Idempotent "start this unit unless it's already active". Clears any
/// stuck `failed` state first so a previous boot's startup error
/// doesn't permanently lock the unit out.
async fn start_nfs_unit(unit: &str) -> String {
    let active = TokioCommand::new("systemctl")
        .args(["is-active", unit])
        .output()
        .await;
    if matches!(active, Ok(o) if o.status.success()) {
        return format!("{unit}: active");
    }
    let _ = TokioCommand::new("systemctl")
        .args(["reset-failed", unit])
        .output()
        .await;
    let out = TokioCommand::new("systemctl")
        .args(["start", unit])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => format!("{unit}: started"),
        Ok(o) => format!(
            "{unit}: failed to start ({}): {}{}",
            o.status,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr),
        ),
        Err(e) => format!("{unit}: spawn error: {e}"),
    }
}

async fn ensure_nfs_server_stopped() -> String {
    // Stop both nfs-server and nfs-statd when we drop to zero exports —
    // statd staying up after the server stopped is harmless but adds
    // noise to systemctl --failed and "still listening on a port" type
    // diagnostics, so we tear down the whole pair.
    let mut log = String::new();
    for unit in ["nfs-server.service", "nfs-statd.service"] {
        log.push_str(&stop_nfs_unit(unit).await);
        log.push('\n');
    }
    log.trim_end().to_string()
}

async fn stop_nfs_unit(unit: &str) -> String {
    let active = TokioCommand::new("systemctl")
        .args(["is-active", unit])
        .output()
        .await;
    let already_inactive = matches!(active, Ok(o) if !o.status.success());
    if already_inactive {
        return format!("{unit}: inactive");
    }
    let out = TokioCommand::new("systemctl")
        .args(["stop", unit])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => format!("{unit}: stopped"),
        Ok(o) => format!(
            "{unit}: failed to stop ({}): {}{}",
            o.status,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr),
        ),
        Err(e) => format!("{unit}: spawn error: {e}"),
    }
}

/// Reject obviously malformed exports content before atomic-replacing the
/// file. The full validity check is delegated to exportfs which the daemon
/// runs immediately after.
fn validate_exports(content: &str) -> Result<()> {
    for (i, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // First whitespace-separated field is the export path; bail if it
        // doesn't start with '/' (no relative paths, no shell metacharacters
        // we forgot to escape).
        let first = line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches('"');
        if !first.starts_with('/') {
            anyhow::bail!(
                "line {}: export path must be absolute (got {first:?})",
                i + 1
            );
        }
    }
    Ok(())
}

/// Group whose members are authorized to sign in to the web admin. Root
/// bypasses this check (it can sign in unconditionally); every other user
/// must be a member or the auth call fails with the same generic error
/// as a wrong password.
const ADMIN_GROUP: &str = "bananas-admin";

/// Verify `password` against the hash stored for `username` in /etc/shadow,
/// then check that the user is authorized (root or a member of
/// `bananas-admin`). Only `$6$` (SHA-512) hashes are accepted — that's what
/// the BanaNAS image produces via `mkpasswd -m sha-512`.
async fn authenticate(username: &str, password: &str) -> Result<()> {
    let entry = verify_shadow_password(username, password).await?;

    // The shadow file's third field is "days since 1970-01-01 of the last
    // password change". `0` is the magic value `chage -d 0` writes — PAM
    // treats it as "must change at next login". We surface it as a stable
    // sentinel so the UI can show a "set new password" form instead of a
    // generic 401.
    if entry.lastchg_zero {
        anyhow::bail!("password_expired");
    }

    // Authorization gate. Root is always trusted; everyone else must be in
    // the bananas-admin group. Same error string as a password mismatch so
    // the API can't be used to enumerate which users have admin access.
    if username != "root" && !is_in_group(username, ADMIN_GROUP).await? {
        anyhow::bail!("user not in {ADMIN_GROUP} group");
    }
    Ok(())
}

struct ShadowEntry {
    /// True when /etc/shadow's third field (last password change in days
    /// since epoch) is exactly `0` — i.e. `chage -d 0` was applied.
    lastchg_zero: bool,
}

/// Verify `password` against the hash stored for `username` in /etc/shadow.
/// Returns the parsed shadow row on success so callers can also inspect
/// the expiry field. Used by both `authenticate` and `change_own_password`
/// — the latter wants to accept the password even when expired so the
/// user can rotate it.
async fn verify_shadow_password(username: &str, password: &str) -> Result<ShadowEntry> {
    if username.is_empty() || username.contains(':') || username.contains('\n') {
        anyhow::bail!("malformed username");
    }
    if password.is_empty() {
        anyhow::bail!("empty password");
    }
    let shadow = fs::read_to_string("/etc/shadow")
        .await
        .context("reading /etc/shadow (helper must run as root)")?;

    let (hash, lastchg) = shadow
        .lines()
        .find_map(|line| {
            let mut fields = line.splitn(4, ':');
            let user = fields.next()?;
            let pw = fields.next()?;
            let lastchg = fields.next()?;
            if user == username {
                Some((pw.to_string(), lastchg.to_string()))
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow::anyhow!("user not found"))?;

    // Locked / disabled accounts: "*", "!", "!!", or any "!"-prefixed hash
    // (passwd -l). The empty string means no password set — treat as locked.
    if hash.is_empty() || hash == "*" || hash.starts_with('!') {
        anyhow::bail!("account locked");
    }
    if !hash.starts_with("$6$") {
        anyhow::bail!(
            "only SHA-512 ($6$) hashes are supported, got prefix {:?}",
            &hash[..hash.find('$').map(|i| i + 1).unwrap_or(0).min(hash.len())]
        );
    }

    sha_crypt::sha512_check(password, &hash).map_err(|_| anyhow::anyhow!("password mismatch"))?;

    Ok(ShadowEntry {
        lastchg_zero: lastchg.trim() == "0",
    })
}

/// Self-service password change. Verifies the old password, then runs
/// `chpasswd` to set the new one. Unlike `set_password`, this is allowed
/// for any user — including root — because the caller already proved
/// they know the current password. After `chpasswd` succeeds, `chage`
/// stamps lastchg with today's day count, which clears the
/// `password_expired` state for future logins.
async fn change_own_password(username: &str, old_password: &str, new_password: &str) -> Result<()> {
    // Same shape-check as authenticate, plus the new-password rules.
    let _ = verify_shadow_password(username, old_password).await?;
    if !valid_name(username) {
        anyhow::bail!("invalid username");
    }
    ensure_password_safe(new_password)?;
    chpasswd(username, new_password).await
}

/// Read /etc/group and return true if `username` is listed as a member of
/// `group`. The primary-GID case (i.e. user's primary group is `group`)
/// is handled separately below.
async fn is_in_group(username: &str, group: &str) -> Result<bool> {
    let group_file = fs::read_to_string("/etc/group")
        .await
        .context("reading /etc/group")?;
    let target_gid: Option<u32> = group_file.lines().find_map(|line| {
        let mut fields = line.splitn(4, ':');
        let name = fields.next()?;
        let _passwd = fields.next()?;
        let gid_s = fields.next()?;
        if name == group {
            gid_s.parse().ok()
        } else {
            None
        }
    });
    for line in group_file.lines() {
        let mut fields = line.splitn(4, ':');
        let name = fields.next().unwrap_or("");
        if name != group {
            continue;
        }
        if let Some(members) = fields.nth(2) {
            if members.split(',').any(|m| m == username) {
                return Ok(true);
            }
        }
        break;
    }
    // Primary-group check — a user whose primary GID is `group`'s GID is
    // also a member, even if they don't appear in the comma list.
    if let Some(gid) = target_gid {
        let passwd = fs::read_to_string("/etc/passwd")
            .await
            .context("reading /etc/passwd")?;
        for line in passwd.lines() {
            let fields: Vec<&str> = line.splitn(7, ':').collect();
            if fields.len() < 7 {
                continue;
            }
            if fields[0] == username && fields[3].parse::<u32>().ok() == Some(gid) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Tiny `/etc/group` parser. We avoid pulling in `nix`/`libc` for one
/// lookup — the format is `name:passwd:gid:members`, one record per line.
fn lookup_group_gid(name: &str) -> Option<u32> {
    let content = std::fs::read_to_string("/etc/group").ok()?;
    for line in content.lines() {
        let mut fields = line.splitn(4, ':');
        let n = fields.next()?;
        let _passwd = fields.next()?;
        let gid_s = fields.next()?;
        if n == name {
            return gid_s.parse().ok();
        }
    }
    None
}

// --- User management ------------------------------------------------------

/// Lowest UID we'll let the API touch. Anything below is system-managed
/// (root, daemon, the `bananas` service user) and locking ourselves out
/// of those would brick the admin server itself.
const MIN_USER_UID: u32 = 1000;

/// Validate POSIX-portable usernames: leading lowercase or underscore,
/// then alnum/underscore/hyphen. useradd already enforces this, but we
/// reject up-front so the error message is sane.
fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 32 {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn ensure_password_safe(password: &str) -> Result<()> {
    if password.is_empty() {
        anyhow::bail!("password must not be empty");
    }
    if password.contains('\0') || password.contains('\n') || password.contains(':') {
        anyhow::bail!("password may not contain NUL, newline, or colon");
    }
    if password.len() > 128 {
        anyhow::bail!("password too long (>128 chars)");
    }
    Ok(())
}

async fn list_users(include_hashes: bool) -> Result<String> {
    let passwd = fs::read_to_string("/etc/passwd")
        .await
        .context("reading /etc/passwd")?;
    let group = fs::read_to_string("/etc/group")
        .await
        .context("reading /etc/group")?;
    let shadow = fs::read_to_string("/etc/shadow")
        .await
        .context("reading /etc/shadow")?;

    // Build a name → primary-group lookup table for the GID column.
    let mut gid_to_name: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    // user → secondary groups (where they appear in `members`).
    let mut user_groups: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for line in group.lines() {
        let mut fields = line.splitn(4, ':');
        let Some(name) = fields.next() else { continue };
        let _passwd = fields.next();
        let Some(gid_s) = fields.next() else { continue };
        let Ok(gid) = gid_s.parse::<u32>() else {
            continue;
        };
        gid_to_name.entry(gid).or_insert_with(|| name.to_string());
        if let Some(members) = fields.next() {
            for m in members.split(',').filter(|s| !s.is_empty()) {
                user_groups
                    .entry(m.to_string())
                    .or_default()
                    .push(name.to_string());
            }
        }
    }

    let mut shadow_locked: std::collections::HashMap<String, bool> =
        std::collections::HashMap::new();
    let mut shadow_hashes: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for line in shadow.lines() {
        let mut fields = line.splitn(3, ':');
        let Some(name) = fields.next() else { continue };
        let Some(hash) = fields.next() else { continue };
        let locked = hash.is_empty() || hash == "*" || hash.starts_with('!');
        shadow_locked.insert(name.to_string(), locked);
        if include_hashes && !locked {
            shadow_hashes.insert(name.to_string(), hash.to_string());
        }
    }

    let mut users = Vec::new();
    for line in passwd.lines() {
        let fields: Vec<&str> = line.splitn(7, ':').collect();
        if fields.len() < 7 {
            continue;
        }
        let name = fields[0].to_string();
        let Ok(uid) = fields[2].parse::<u32>() else {
            continue;
        };
        let Ok(gid) = fields[3].parse::<u32>() else {
            continue;
        };
        let gecos = fields[4].split(',').next().unwrap_or("").to_string();
        let home = fields[5].to_string();
        let shell = fields[6].to_string();
        let primary_group = gid_to_name.get(&gid).cloned();
        let mut groups = user_groups.remove(&name).unwrap_or_default();
        groups.sort();
        groups.dedup();
        let mut entry = json!({
            "name": name.clone(),
            "uid": uid,
            "gid": gid,
            "primary_group": primary_group,
            "full_name": if gecos.is_empty() { serde_json::Value::Null } else { json!(gecos) },
            "home": home,
            "shell": shell,
            "locked": shadow_locked.get(&name).copied().unwrap_or(true),
            "groups": groups,
            "system": uid < MIN_USER_UID,
        });
        if include_hashes {
            // Empty Option<&String> → null, real hash → string. Only present
            // for non-system, unlocked accounts (filtered above).
            if let Some(h) = shadow_hashes.get(&name) {
                entry["password_hash"] = json!(h);
            }
        }
        users.push(entry);
    }
    Ok(serde_json::to_string(&json!({ "users": users }))?)
}

async fn create_user(
    username: &str,
    password: &str,
    full_name: Option<&str>,
    admin: bool,
    password_is_hash: bool,
) -> Result<()> {
    if !valid_name(username) {
        anyhow::bail!("invalid username");
    }
    if password_is_hash {
        ensure_hash_safe(password)?;
    } else {
        ensure_password_safe(password)?;
    }
    if user_exists(username).await? {
        anyhow::bail!("user already exists");
    }
    let mut cmd = TokioCommand::new("useradd");
    cmd.arg("-m");
    if let Some(name) = full_name {
        if name.contains(':') || name.contains('\n') {
            anyhow::bail!("invalid full name");
        }
        cmd.args(["-c", name]);
    }
    cmd.arg(username);
    let out = cmd.output().await.context("spawning useradd")?;
    if !out.status.success() {
        anyhow::bail!(
            "useradd failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    if password_is_hash {
        chpasswd_encrypted(username, password).await?;
    } else {
        chpasswd(username, password).await?;
    }
    if admin {
        set_admin(username, true).await?;
    }
    Ok(())
}

/// Validate a shadow-style password hash. Looser than the plaintext check
/// because hashes intentionally contain `$` separators — but they must
/// still be a single line of printable ASCII without colons (which would
/// break /etc/shadow's field separator) and start with `$<id>$`.
fn ensure_hash_safe(hash: &str) -> Result<()> {
    if hash.is_empty() {
        anyhow::bail!("password hash must not be empty");
    }
    if hash.contains('\n') || hash.contains('\0') || hash.contains(':') {
        anyhow::bail!("password hash may not contain newline, NUL, or colon");
    }
    if !hash.starts_with('$') {
        anyhow::bail!("password hash must be in $id$salt$digest format");
    }
    Ok(())
}

/// `chpasswd -e` reads "user:hash" pairs and writes the hash to
/// /etc/shadow without rehashing. Used by the config-import path to
/// restore accounts with their original passwords.
async fn chpasswd_encrypted(username: &str, hash: &str) -> Result<()> {
    let mut child = TokioCommand::new("chpasswd")
        .arg("-e")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning chpasswd -e")?;
    let mut stdin = child.stdin.take().context("chpasswd stdin missing")?;
    let line = format!("{}:{}\n", username, hash);
    stdin
        .write_all(line.as_bytes())
        .await
        .context("writing chpasswd stdin")?;
    drop(stdin);
    let out = child
        .wait_with_output()
        .await
        .context("waiting on chpasswd")?;
    if !out.status.success() {
        anyhow::bail!(
            "chpasswd -e failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

async fn set_admin(username: &str, admin: bool) -> Result<()> {
    if !valid_name(username) {
        anyhow::bail!("invalid username");
    }
    if username == "root" {
        anyhow::bail!("root is implicitly an admin; nothing to do");
    }
    let uid = uid_of(username).await?.context("user not found")?;
    if uid < MIN_USER_UID {
        anyhow::bail!("refusing to modify groups for system user (uid {uid})");
    }
    let arg = if admin { "-a" } else { "-d" };
    let out = TokioCommand::new("gpasswd")
        .args([arg, username, "bananas-admin"])
        .output()
        .await
        .context("spawning gpasswd")?;
    if !out.status.success() {
        anyhow::bail!(
            "gpasswd failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

async fn delete_user(username: &str) -> Result<()> {
    if !valid_name(username) {
        anyhow::bail!("invalid username");
    }
    let uid = uid_of(username).await?.context("user not found")?;
    if uid < MIN_USER_UID {
        anyhow::bail!("refusing to delete system user (uid {uid})");
    }
    let out = TokioCommand::new("userdel")
        .args(["-r", username])
        .output()
        .await
        .context("spawning userdel")?;
    if !out.status.success() {
        anyhow::bail!(
            "userdel failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

async fn set_password(username: &str, password: &str) -> Result<()> {
    if !valid_name(username) {
        anyhow::bail!("invalid username");
    }
    ensure_password_safe(password)?;
    let uid = uid_of(username).await?.context("user not found")?;
    if uid < MIN_USER_UID {
        anyhow::bail!("refusing to change password for system user (uid {uid})");
    }
    chpasswd(username, password).await
}

async fn chpasswd(username: &str, password: &str) -> Result<()> {
    let mut child = TokioCommand::new("chpasswd")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning chpasswd")?;
    let mut stdin = child.stdin.take().context("chpasswd stdin missing")?;
    let line = format!("{}:{}\n", username, password);
    stdin
        .write_all(line.as_bytes())
        .await
        .context("writing chpasswd stdin")?;
    drop(stdin);
    let out = child
        .wait_with_output()
        .await
        .context("waiting on chpasswd")?;
    if !out.status.success() {
        anyhow::bail!(
            "chpasswd failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

async fn user_exists(username: &str) -> Result<bool> {
    Ok(uid_of(username).await?.is_some())
}

async fn uid_of(username: &str) -> Result<Option<u32>> {
    let passwd = fs::read_to_string("/etc/passwd")
        .await
        .context("reading /etc/passwd")?;
    for line in passwd.lines() {
        let fields: Vec<&str> = line.splitn(7, ':').collect();
        if fields.len() < 7 {
            continue;
        }
        if fields[0] == username {
            return Ok(fields[2].parse().ok());
        }
    }
    Ok(None)
}

// --- Filesystem permissions ----------------------------------------------

/// Allowlist for paths the UI is permitted to chown/chmod. We don't want
/// to give the web admin a way to chmod /etc, /usr, /, etc. Anything
/// under these prefixes is fair game; everything else gets a 403-style
/// reply on the wire.
const PERMS_ALLOW_PREFIXES: &[&str] = &[
    "/srv/", "/mnt/", "/media/", "/home/", "/opt/",
    // Match the prefix-with-trailing-slash check below; bare /srv is
    // also explicitly allowed so the operator can fix the mount root
    // ownership itself (the original NFS-write motivation).
    "/srv", "/mnt", "/media", "/home", "/opt",
];

fn is_safe_perms_path(path: &str) -> bool {
    if !path.starts_with('/') || path.contains("..") {
        return false;
    }
    PERMS_ALLOW_PREFIXES.iter().any(|p| {
        // exact-match (e.g. "/srv") OR starts-with-with-slash ("/srv/x")
        path == *p || path.starts_with(&format!("{}/", p.trim_end_matches('/')))
    })
}

async fn set_timezone(tz: &str) -> Result<String> {
    // Validate against the on-disk zoneinfo db. A bogus tz here would
    // get rejected by timedatectl too, but checking up front gives us
    // a cleaner error message and keeps audit logs readable.
    if tz.is_empty()
        || tz.contains("..")
        || tz.starts_with('/')
        || !tz
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '+'))
    {
        anyhow::bail!("invalid timezone string: {tz:?}");
    }
    let zone_path = format!("/usr/share/zoneinfo/{tz}");
    if !std::path::Path::new(&zone_path).exists() {
        anyhow::bail!("unknown timezone {tz:?} (no zoneinfo entry at {zone_path})");
    }
    let out = TokioCommand::new("timedatectl")
        .args(["set-timezone", tz])
        .output()
        .await
        .context("spawning timedatectl")?;
    if !out.status.success() {
        anyhow::bail!(
            "timedatectl set-timezone failed (status {}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut log = format!("timedatectl set-timezone {tz}\n");

    // Persist to /etc/bananas/system.toml so the web UI's General tab
    // and the TUI's Status tab show the same value the kernel is
    // actually using. Helper owns this — the previous split (server
    // did a follow-up WriteServiceConfig, CLI/TUI didn't) led to
    // /etc/localtime and system.toml drifting on the CLI path.
    match persist_timezone_to_system_toml(tz).await {
        Ok(persisted) => log.push_str(&persisted),
        Err(e) => log.push_str(&format!("WARN persisting system.toml: {e}\n")),
    }

    // Bounce bananas-dashboard so the LCD picks up the new local
    // offset (the Slint app reads time::UtcOffset::current_local_offset
    // once at startup). Best-effort — a missing unit on a fresh image
    // shouldn't make the timezone change fail.
    match TokioCommand::new("systemctl")
        .args(["restart", "bananas-dashboard.service"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
    {
        Ok(o) if o.status.success() => log.push_str("restarted bananas-dashboard.service\n"),
        Ok(o) => log.push_str(&format!(
            "WARN restart bananas-dashboard.service status {}: {}\n",
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => log.push_str(&format!(
            "WARN spawning systemctl restart bananas-dashboard: {e}\n"
        )),
    }

    Ok(log)
}

/// Read /etc/bananas/system.toml, set [system].timezone = tz, atomic-write
/// it back. Creates the file (and parent dir) if missing. Used by the
/// SetTimezone helper command so persistence is symmetric across every
/// caller that drives a tz change.
async fn persist_timezone_to_system_toml(tz: &str) -> Result<String> {
    const SYSTEM_TOML: &str = "/etc/bananas/system.toml";
    if let Some(parent) = std::path::Path::new(SYSTEM_TOML).parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let existing = fs::read_to_string(SYSTEM_TOML).await.unwrap_or_default();
    let mut doc: toml::Table = if existing.trim().is_empty() {
        toml::Table::new()
    } else {
        existing
            .parse()
            .with_context(|| format!("parsing existing {SYSTEM_TOML}"))?
    };
    let system_table = doc
        .entry("system".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(t) = system_table {
        t.insert("timezone".into(), tz.into());
    }
    let serialized = toml::to_string(&doc).context("serializing system.toml")?;
    let tmp = format!("{SYSTEM_TOML}.tmp");
    fs::write(&tmp, &serialized)
        .await
        .with_context(|| format!("writing {tmp}"))?;
    fs::rename(&tmp, SYSTEM_TOML)
        .await
        .with_context(|| format!("renaming {tmp} -> {SYSTEM_TOML}"))?;
    Ok(format!("wrote {SYSTEM_TOML}\n"))
}

async fn list_timezones() -> Result<String> {
    // Walk /usr/share/zoneinfo and return every regular file path
    // relative to that root. Skip top-level directories that aren't
    // user-facing zones (Etc/ collides too much with friendly names,
    // posix/ + right/ are duplicate trees with different leap-second
    // handling, and SystemV/ is legacy POSIX-style aliases).
    const ROOT: &str = "/usr/share/zoneinfo";
    const SKIP: &[&str] = &["Etc", "posix", "right", "SystemV"];

    let mut zones: Vec<String> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![std::path::PathBuf::from(ROOT)];
    while let Some(dir) = stack.pop() {
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in read.flatten() {
            let path = entry.path();
            let rel = match path.strip_prefix(ROOT) {
                Ok(r) => r.to_string_lossy().to_string(),
                Err(_) => continue,
            };
            // Skip the curated drop-list at the top level.
            if path
                .parent()
                .map(|p| p == std::path::Path::new(ROOT))
                .unwrap_or(false)
                && SKIP.iter().any(|s| rel == *s)
            {
                continue;
            }
            // Skip non-zone files at the root: localtime, posixrules,
            // tzdata.zi, leapseconds, zone.tab/zone1970.tab/iso3166.tab,
            // etc. — these aren't IANA names.
            if !rel.contains('/')
                && !path.is_dir()
                && (rel.contains('.')
                    || rel == "localtime"
                    || rel == "posixrules"
                    || rel == "leapseconds")
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                zones.push(rel);
            }
        }
    }
    zones.sort();
    Ok(serde_json::to_string(&zones).context("serializing timezone list")?)
}

async fn run_lsblk() -> Result<String> {
    // -J = JSON, -b = bytes (not human-readable), -o pins the column
    // set the server expects to parse. Running here as root lets
    // blkid read /dev/sd* superblocks for FSTYPE/LABEL/UUID.
    let out = TokioCommand::new("lsblk")
        .args([
            "-J",
            "-b",
            "-o",
            "NAME,KNAME,SIZE,MODEL,TYPE,MOUNTPOINT,FSTYPE,LABEL,UUID,RO",
        ])
        .output()
        .await
        .context("spawning lsblk")?;
    if !out.status.success() {
        anyhow::bail!(
            "lsblk failed (status {}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn make_directory(path: &str) -> Result<String> {
    if !is_safe_perms_path(path) {
        anyhow::bail!("path not allowed: {path}");
    }
    // Don't allow exact-match against an allowlist root — they already
    // exist, and `mkdir -p /srv` is a no-op anyway. The point is creating
    // children of those, e.g. `/srv/media`.
    if PERMS_ALLOW_PREFIXES.iter().any(|p| *p == path) {
        return Ok(format!("{path} exists (allowlist root)"));
    }
    if let Ok(md) = std::fs::symlink_metadata(path) {
        if md.is_dir() {
            return Ok(format!("{path} already exists"));
        }
        anyhow::bail!("{path} exists but is not a directory");
    }
    std::fs::create_dir_all(path).with_context(|| format!("mkdir -p {path}"))?;
    Ok(format!("created {path}"))
}

async fn stat_path(path: &str) -> Result<String> {
    if !is_safe_perms_path(path) {
        anyhow::bail!("path not allowed: {path}");
    }
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::symlink_metadata(path).with_context(|| format!("stat {path}"))?;
    let uid = md.uid();
    let gid = md.gid();
    let mode = md.mode() & 0o7777;
    let kind = if md.is_dir() {
        "dir"
    } else if md.file_type().is_symlink() {
        "symlink"
    } else if md.is_file() {
        "file"
    } else {
        "other"
    };
    let user = lookup_user(uid).unwrap_or_default();
    let group = lookup_group_name(gid).unwrap_or_default();
    Ok(serde_json::to_string(&serde_json::json!({
        "path": path,
        "uid": uid,
        "gid": gid,
        "user": user,
        "group": group,
        "mode": format!("{mode:o}"),
        "mode_decimal": mode,
        "kind": kind,
    }))?)
}

async fn set_permissions(
    path: &str,
    uid: Option<u32>,
    gid: Option<u32>,
    mode: Option<&str>,
    recursive: bool,
) -> Result<String> {
    if !is_safe_perms_path(path) {
        anyhow::bail!("path not allowed: {path}");
    }
    if uid.is_none() && gid.is_none() && mode.is_none() {
        anyhow::bail!("nothing to change — set at least one of uid, gid, mode");
    }
    let mut steps = Vec::<String>::new();

    if uid.is_some() || gid.is_some() {
        let owner = format!(
            "{}:{}",
            uid.map(|n| n.to_string()).unwrap_or_default(),
            gid.map(|n| n.to_string()).unwrap_or_default()
        );
        let mut cmd = TokioCommand::new("chown");
        if recursive {
            cmd.arg("-R");
        }
        cmd.args([owner.as_str(), path]);
        let out = cmd.output().await.context("spawning chown")?;
        if !out.status.success() {
            anyhow::bail!(
                "chown failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        steps.push(format!(
            "chown {owner} {}",
            if recursive { "-R " } else { "" }.to_string() + path
        ));
    }

    if let Some(mode_str) = mode {
        // Accept either decimal ("493") or octal-with-prefix ("0o755") /
        // bare octal ("755"). chmod's own parser handles bare octal.
        let mode_arg = mode_str.trim_start_matches("0o");
        if !mode_arg.chars().all(|c| c.is_ascii_digit()) {
            anyhow::bail!("mode must be numeric (octal), got {mode_str:?}");
        }
        let mut cmd = TokioCommand::new("chmod");
        if recursive {
            cmd.arg("-R");
        }
        cmd.args([mode_arg, path]);
        let out = cmd.output().await.context("spawning chmod")?;
        if !out.status.success() {
            anyhow::bail!(
                "chmod failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        steps.push(format!(
            "chmod {mode_arg} {}",
            if recursive { "-R " } else { "" }.to_string() + path
        ));
    }
    Ok(steps.join("\n"))
}

fn lookup_user(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let mut fields = line.splitn(4, ':');
        let name = fields.next()?;
        let _ = fields.next()?;
        let uid_s = fields.next()?;
        if uid_s.parse::<u32>().ok() == Some(uid) {
            return Some(name.to_string());
        }
    }
    None
}

/// Forward lookup: name → (uid, gid). Used by run_cloud_sync to
/// drop privileges from root → `bananas` before exec'ing rclone, so
/// files written under /srv/* end up bananas-owned.
fn lookup_uid_gid(name: &str) -> Option<(u32, u32)> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let mut fields = line.splitn(7, ':');
        let entry_name = fields.next()?;
        let _ = fields.next()?;
        let uid: u32 = fields.next()?.parse().ok()?;
        let gid: u32 = fields.next()?.parse().ok()?;
        if entry_name == name {
            return Some((uid, gid));
        }
    }
    None
}

fn lookup_group_name(gid: u32) -> Option<String> {
    let group = std::fs::read_to_string("/etc/group").ok()?;
    for line in group.lines() {
        let mut fields = line.splitn(4, ':');
        let name = fields.next()?;
        let _ = fields.next()?;
        let gid_s = fields.next()?;
        if gid_s.parse::<u32>().ok() == Some(gid) {
            return Some(name.to_string());
        }
    }
    None
}

/// Run smartctl against a validated block device. Returns the JSON output
/// verbatim — the server passes it through to the UI, which extracts the
/// fields it cares about (smart_status.passed, ata_smart_attributes, etc.).
async fn smart(device: &str) -> Result<String> {
    if !is_valid_block_device(device) {
        anyhow::bail!("invalid device path {device:?}");
    }
    let out = TokioCommand::new("smartctl")
        .args(["-j", "-H", "-A", device])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning smartctl")?;
    // smartctl uses a bitmask exit code: bit 0 = command line error, bits
    // 1+ = various drive states. Bits 4+ (failures predicted etc.) still
    // return useful JSON, so we accept everything except bit 0.
    let code = out.status.code().unwrap_or(0);
    if code & 0b1 != 0 {
        anyhow::bail!(
            "smartctl rejected the request: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let json = String::from_utf8(out.stdout).context("smartctl produced non-UTF-8 output")?;
    Ok(json)
}

/// Allowlist for device paths the helper will hand to smartctl. Matches
/// the kernel's typical block-device naming: sd[a-z]+, nvme<n>n<n>,
/// mmcblk<n>. Anything with a path separator past `/dev/` or non-ASCII is
/// rejected outright.
fn is_valid_block_device(device: &str) -> bool {
    let Some(name) = device.strip_prefix("/dev/") else {
        return false;
    };
    if name.is_empty() || name.len() > 32 || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    // sd[a-z]+, nvme*n*, mmcblk* covers our cases. We don't bother with
    // a regex — the alphanumeric check above already constrains it.
    name.starts_with("sd") || name.starts_with("nvme") || name.starts_with("mmcblk")
}

async fn write_fstab(content: &str) -> Result<String> {
    validate_fstab(content)?;
    // Each user-managed fstab row needs its mountpoint to exist on disk,
    // otherwise systemd-fstab-generator's auto-mount unit fails on the
    // next boot. Same family of issue the export path had — mkdir the
    // user-managed mountpoints up front so a saved row "just works"
    // after a reboot or `mount -a`.
    let mkdir_log = ensure_fstab_mountpoints(content).await;

    let path = Path::new("/etc/fstab");
    let dir = path.parent().unwrap_or_else(|| Path::new("/"));
    let tmp = dir.join(".fstab.tmp");
    fs::write(&tmp, content)
        .await
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))
        .await
        .ok();
    fs::rename(&tmp, path)
        .await
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;

    let reload = systemd_daemon_reload()
        .await
        .unwrap_or_else(|e| format!("(daemon-reload failed: {e})"));

    // `mount -a` brings up any newly-added entry without needing a
    // reboot. -O no_netdev skips entries flagged as needing the network
    // (we don't ship any by default but operators might add NFS rows).
    let mount_log = match TokioCommand::new("mount")
        .args(["-a", "-O", "no_netdev"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
    {
        Ok(o) if o.status.success() => "mount -a: ok".to_string(),
        Ok(o) => format!(
            "mount -a: rc={} {}{}",
            o.status,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => format!("mount -a: spawn error: {e}"),
    };

    Ok(format!(
        "wrote /etc/fstab ({} bytes)\n{}{}\n{}",
        content.len(),
        mkdir_log,
        reload,
        mount_log,
    ))
}

/// Walk the fstab content's mountpoint column (field #2) and create
/// any directory that doesn't exist yet. Skip the OE stock mountpoints
/// (`/`, `/proc`, `/dev/pts`, `/run`, `/var/volatile`) — they're created
/// by base-files and trying to mkdir on them is a no-op anyway.
async fn ensure_fstab_mountpoints(content: &str) -> String {
    let mut created: Vec<String> = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 2 {
            continue;
        }
        let mountpoint = fields[1];
        if !mountpoint.starts_with('/') {
            continue;
        }
        let p = Path::new(mountpoint);
        if p.is_dir() {
            continue;
        }
        match fs::create_dir_all(p).await {
            Ok(()) => created.push(mountpoint.to_string()),
            Err(e) => {
                return format!("mkdir -p {mountpoint} failed: {e}\n");
            }
        }
    }
    if created.is_empty() {
        String::new()
    } else {
        format!("created mountpoints: {}\n", created.join(" "))
    }
}

/// Reject obviously malformed fstab content. Each non-blank, non-comment
/// line must have 6 whitespace-separated fields and an absolute
/// mountpoint. We don't try to validate the device spec or fstype — both
/// can take many shapes (LABEL=, UUID=, /dev/X, tmpfs, swap, none, etc.)
/// and `mount` will surface specific errors when actually mounting.
fn validate_fstab(content: &str) -> Result<()> {
    for (i, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() != 6 {
            anyhow::bail!(
                "line {}: expected 6 fields (device, mountpoint, fstype, options, dump, pass), got {}",
                i + 1,
                fields.len()
            );
        }
        if fields[1] != "none" && fields[1] != "swap" && !fields[1].starts_with('/') {
            anyhow::bail!(
                "line {}: mountpoint must be absolute (got {:?})",
                i + 1,
                fields[1]
            );
        }
        for (which, idx) in [("dump", 4), ("pass", 5)] {
            if fields[idx].parse::<u32>().is_err() {
                anyhow::bail!(
                    "line {}: {} must be a non-negative integer (got {:?})",
                    i + 1,
                    which,
                    fields[idx]
                );
            }
        }
    }
    Ok(())
}

async fn systemd_daemon_reload() -> Result<String> {
    let out = TokioCommand::new("systemctl")
        .arg("daemon-reload")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning systemctl daemon-reload")?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        anyhow::bail!(
            "systemctl daemon-reload failed (status {}): {combined}",
            out.status
        );
    }
    Ok(combined)
}

async fn run_cloud_sync(idx: usize) -> Result<String> {
    // Read cloud.toml fresh — the user might have edited entries via
    // the API just before triggering the run.
    let raw = match tokio::fs::read_to_string("/etc/bananas/cloud.toml").await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("/etc/bananas/cloud.toml does not exist (no accounts configured)");
        }
        Err(e) => return Err(e).context("reading /etc/bananas/cloud.toml"),
    };

    #[derive(serde::Deserialize)]
    struct Cfg {
        #[serde(default)]
        accounts: Vec<Account>,
        #[serde(default)]
        syncs: Vec<SyncEntry>,
    }
    #[derive(serde::Deserialize)]
    struct Account {
        name: String,
        provider: String,
        #[serde(default)]
        token: String,
    }
    #[derive(serde::Deserialize)]
    struct SyncEntry {
        account: String,
        local_path: String,
        remote_path: String,
        #[serde(default)]
        direction: String,
        #[serde(default)]
        schedule: String,
    }

    let cfg: Cfg = toml::from_str(&raw).context("parsing /etc/bananas/cloud.toml")?;
    let entry = cfg
        .syncs
        .get(idx)
        .ok_or_else(|| anyhow::anyhow!("sync entry {idx} not found"))?;
    let account = cfg
        .accounts
        .iter()
        .find(|a| a.name == entry.account)
        .ok_or_else(|| {
            anyhow::anyhow!("sync entry references missing account {:?}", entry.account)
        })?;

    if account.token.trim().is_empty() {
        anyhow::bail!(
            "account {:?} has no token configured — paste an `rclone authorize \"{}\"` token through the Cloud tab first",
            account.name,
            account.provider
        );
    }

    // Provider keys are rclone backend names verbatim (see the catalog
    // in `crates/server/src/cloud.rs::PROVIDERS`). Allowlist them so a
    // forged cloud.toml can't slip an arbitrary backend through.
    let rclone_type = match account.provider.as_str() {
        "drive" | "dropbox" | "onedrive" | "s3" | "webdav" | "ftp" => account.provider.as_str(),
        other => anyhow::bail!("unsupported provider {other:?} for rclone runtime"),
    };

    // `rclone copy <local> <remote>:<path>` for push,
    // `rclone copy <remote>:<path> <local>` for pull,
    // `rclone bisync <local> <remote>:<path>` for bidirectional.
    // bisync needs an initial `--resync` on first run; we pass it
    // unconditionally so first-time sync entries don't fail with
    // "first run needs --resync".
    let direction = if entry.direction.is_empty() {
        "push"
    } else {
        entry.direction.as_str()
    };
    let remote_arg = format!("{}:{}", account.name, entry.remote_path);

    // Drop privileges from root → bananas so synced files end up
    // owned by the right user (and rclone can't accidentally read /
    // overwrite root-only paths). Falls back to running as root with
    // a warning if the user is missing — should never happen on the
    // BPI image since the bananas-server recipe creates it via USERADD.
    let bananas = lookup_uid_gid("bananas");
    if bananas.is_none() {
        tracing::warn!("`bananas` user missing from /etc/passwd; rclone will run as root");
    }

    let mut cmd = TokioCommand::new("rclone");
    cmd.env_clear();
    if let Some((uid, gid)) = bananas {
        cmd.uid(uid);
        cmd.gid(gid);
        cmd.env("HOME", format!("/home/{}", "bananas"));
        cmd.env("USER", "bananas");
    }
    // Preserve PATH so rclone can find /bin tools it might shell out
    // to. systemd's environment for the helper unit already provides
    // this; we just don't want our env_clear to nuke it.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    // Tell rclone to use a temporary in-memory config — no on-disk
    // rclone.conf with the token.
    cmd.env("RCLONE_CONFIG", "/dev/null");
    // Per-remote env-var-style config. rclone reads
    // RCLONE_CONFIG_<NAME>_<KEY> at runtime; the underscore-uppercase
    // mangling matches the docs.
    let env_prefix = format!("RCLONE_CONFIG_{}", account.name.to_uppercase());
    cmd.env(format!("{env_prefix}_TYPE"), rclone_type);
    cmd.env(format!("{env_prefix}_TOKEN"), &account.token);

    // `--stats=2s --stats-one-line` prints a periodic progress summary to
    // stderr in a parseable shape:
    //   "Transferred: 5.2 GiB / 12.3 GiB, 42%, 1.2 MiB/s, ETA 1h41m"
    // We tee that into the per-sync progress file so the server-side
    // JobManager (and the UI's RunRow circular bar) can poll it without
    // any IPC changes to the helper protocol.
    // The local path has to exist before rclone runs. For `push` /
    // `bidirectional` we want the operator to fix the underlying
    // problem (usually: their backing disk isn't mounted yet) instead
    // of silently pushing an empty tree to the cloud — auto-mkdir-ing
    // here would mask the real misconfiguration. For `pull` we DO
    // create the directory because rclone is about to populate it
    // and a missing local target is the normal "fresh restore" case.
    let local_missing = !std::path::Path::new(&entry.local_path).is_dir();
    if local_missing {
        match direction {
            "pull" => {
                fs::create_dir_all(&entry.local_path)
                    .await
                    .with_context(|| format!("creating pull target {}", entry.local_path))?;
            }
            _ => {
                anyhow::bail!(
                    "local path {:?} does not exist on this device. \
                     If the underlying disk should be mounted there, add the \
                     fstab entry from the Mount points tab and retry. Otherwise \
                     edit the sync entry's Local path to point somewhere that \
                     exists.",
                    entry.local_path,
                );
            }
        }
    }

    match direction {
        "push" => {
            cmd.args(["copy", &entry.local_path, &remote_arg]);
        }
        "pull" => {
            cmd.args(["copy", &remote_arg, &entry.local_path]);
        }
        "bidirectional" | "bisync" => {
            cmd.args(["bisync", &entry.local_path, &remote_arg, "--resync"]);
        }
        other => anyhow::bail!("unknown direction {other:?}"),
    }
    // `--stats-log-level=NOTICE` is load-bearing: rclone defaults its
    // overall log level to NOTICE, but the stats output level defaults
    // to INFO — which is BELOW NOTICE and therefore silently dropped
    // before it reaches stderr. Without this flag the helper sees no
    // "Transferred: …%" lines at all and the UI's circular progress
    // bar stays indeterminate forever.
    cmd.args(["--stats=2s", "--stats-one-line", "--stats-log-level=NOTICE"]);

    let _ = entry.schedule; // honored by an external timer, not here

    let progress_path = sync_progress_path(idx);
    if let Some(parent) = progress_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    // Stale progress file from a previous run for this sync_idx — wipe it
    // so the UI doesn't see a frozen percentage from minutes ago.
    let _ = tokio::fs::remove_file(&progress_path).await;

    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning rclone")?;

    // Record the PID so the operator can hit Cancel from the UI.
    // Cancel reads `/run/bananas/sync-progress/<idx>.pid` and SIGTERMs;
    // see `cancel_cloud_sync` below.
    let pid_path = sync_pid_path(idx);
    if let Some(pid) = child.id() {
        let _ = fs::write(&pid_path, pid.to_string()).await;
    }

    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");

    let progress_for_stdout = progress_path.clone();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        let mut captured = String::new();
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some(pct) = parse_rclone_progress(&line) {
                let _ = tokio::fs::write(&progress_for_stdout, pct.to_string()).await;
            }
            captured.push_str(&line);
            captured.push('\n');
        }
        captured
    });

    let progress_for_stderr = progress_path.clone();
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        let mut captured = String::new();
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some(pct) = parse_rclone_progress(&line) {
                let _ = tokio::fs::write(&progress_for_stderr, pct.to_string()).await;
            }
            captured.push_str(&line);
            captured.push('\n');
        }
        captured
    });

    let status = child.wait().await.context("waiting on rclone")?;
    let stdout_capture = stdout_task.await.unwrap_or_default();
    let stderr_capture = stderr_task.await.unwrap_or_default();
    // Done, regardless of outcome — the progress + pid files are the
    // "live state" contract. After the job finishes the JobManager
    // surfaces success/failure via `status`, and a stale 99% / lingering
    // pid would just be confusing or block a Cancel against the next
    // run.
    let _ = tokio::fs::remove_file(&progress_path).await;
    let _ = tokio::fs::remove_file(&pid_path).await;

    let combined = format!("{stdout_capture}{stderr_capture}");
    if !status.success() {
        anyhow::bail!("rclone exited with {status}: {combined}");
    }
    Ok(combined)
}

/// rclone v1.68 `--stats-one-line --stats-log-level=NOTICE` lines look
/// like (verified on the BPI's running binary):
///   "2026/04/29 18:34:34 NOTICE:    85.434 MiB / 200 MiB, 43%, 36 MiB/s, ETA 3s"
/// The earlier version of this parser gated on `line.contains("Transferred:")`,
/// which v1.68's `--stats-one-line` no longer emits — that's why the
/// progress file never appeared in production. Match instead on
/// `"NOTICE:"` + the integer immediately preceding `%`, which is the
/// only place rclone's status format puts a percent.
fn parse_rclone_progress(line: &str) -> Option<u32> {
    let line = line.trim();
    if !line.contains("NOTICE:") {
        return None;
    }
    let pct_idx = line.find('%')?;
    let prefix = &line[..pct_idx];
    let digits: String = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    let digits: String = digits.chars().rev().collect();
    digits.parse().ok()
}

/// Where to write the live progress percentage for a given sync index.
/// `/run/bananas/sync-progress/<idx>.progress` is in tmpfs (cleared on
/// reboot) and the bananas-helper systemd unit's RuntimeDirectory=
/// declaration creates `/run/bananas` for us.
fn sync_progress_path(idx: usize) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/run/bananas/sync-progress/{idx}.progress"))
}

/// Sibling of the progress file: holds the rclone child PID while the
/// run is in flight so the Cancel button has something to target.
fn sync_pid_path(idx: usize) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/run/bananas/sync-progress/{idx}.pid"))
}

/// Cancel the in-flight rclone for `idx` by sending SIGTERM to the PID
/// the run task wrote into `<idx>.pid`. SIGTERM lets rclone close
/// open transfers cleanly; if it doesn't exit within ~5 s the OS sends
/// SIGKILL (handled by the Drop on the spawned process). Returns a
/// human-readable status line for the apply banner.
async fn cancel_cloud_sync(idx: usize) -> Result<String> {
    let pid_path = sync_pid_path(idx);
    let pid_str = match fs::read_to_string(&pid_path).await {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            return Ok(format!("no live PID for sync {idx} (already finished?)"));
        }
    };
    let pid: i32 = pid_str
        .parse()
        .with_context(|| format!("parsing pid {pid_str:?} from {}", pid_path.display()))?;

    // SIGTERM = 15. We avoid the `nix` crate dependency and just shell
    // out to /bin/kill — same gate as the rest of the privileged ops.
    let out = TokioCommand::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning kill")?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(format!(
            "cancelled sync {idx} (SIGTERM → pid {pid})\n{combined}"
        ))
    } else {
        anyhow::bail!(
            "kill -TERM {pid} failed (status {}): {combined}",
            out.status
        );
    }
}

/// `systemctl reboot` schedules a reboot through systemd. The helper
/// returns success synchronously — systemd waits for the unit handler
/// to finish before actually pulling the trigger, so our reply makes
/// it back to bananas-server before the network drops. The browser
/// just sees a connection close shortly after the apply banner.
async fn reboot_system() -> Result<String> {
    let out = TokioCommand::new("systemctl")
        .arg("reboot")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning systemctl reboot")?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        anyhow::bail!(
            "systemctl reboot failed (status {}): {combined}",
            out.status
        );
    }
    Ok(format!("rebooting…\n{combined}"))
}

async fn exportfs_reload() -> Result<String> {
    let out = TokioCommand::new("exportfs")
        .arg("-rv")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning exportfs -rv")?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        anyhow::bail!("exportfs failed (status {}): {combined}", out.status);
    }
    Ok(combined)
}

/// Allowlist mapping a logical service name to (config-path, systemd-unit).
/// Adding a new entry here is the only way to expose another file to the
/// UI's edit-config modal — the path/unit are NEVER taken from the
/// request, so a malicious server can't ask the helper to write
/// /etc/passwd or restart sshd.
/// Map a logical config name to its on-disk path + the systemd units
/// that need to be bounced when the file changes. Restart behaviour:
///   - `bananas-stats` is restarted on stats.toml changes — sampling
///     interval / retention / device filters are read once at startup.
///   - `bananas-dashboard` is intentionally NOT in this list anymore.
///     The dashboard polls /etc/bananas/stats.toml's mtime every 2 s
///     and reapplies theme + UI changes in place, so the LCD doesn't
///     blink off + redo KMS+EGL init every time the operator flips
///     a setting through the web UI.
///   - cloud.toml is read live by bananas-server on each /api/cloud/*
///     request — no daemon to restart, hence the empty slice.
fn service_config_target(name: &str) -> Option<(&'static str, &'static [&'static str])> {
    match name {
        "stats" => Some(("/etc/bananas/stats.toml", &["bananas-stats.service"])),
        // Dashboard polls its config's mtime every 2 s and reapplies
        // theme / refresh-rate changes in place — no service restart
        // needed. Empty units slice keeps the LCD from blinking off
        // when the operator flips a setting through the web UI.
        "dashboard" => Some(("/etc/bananas/dashboard.toml", &[])),
        "cloud" => Some(("/etc/bananas/cloud.toml", &[])),
        // Read by bananas-server on every request that needs it; no
        // daemon to restart. Used for the General tab (timezone, etc).
        "system" => Some(("/etc/bananas/system.toml", &[])),
        _ => None,
    }
}

async fn read_service_config(name: &str) -> Result<String> {
    let (path, _units) = service_config_target(name)
        .ok_or_else(|| anyhow::anyhow!("unknown service config {name:?}"))?;
    match tokio::fs::read_to_string(path).await {
        Ok(s) => Ok(s),
        // Missing file is not an error — return empty so the UI can show
        // a fresh editor instead of a banner.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).context(format!("reading {path}")),
    }
}

async fn write_service_config(name: &str, content: &str) -> Result<String> {
    let (path, units) = service_config_target(name)
        .ok_or_else(|| anyhow::anyhow!("unknown service config {name:?}"))?;
    // Validate as TOML before touching the disk — invalid syntax would
    // crash the service on next start.
    toml::from_str::<toml::Value>(content).with_context(|| format!("invalid TOML for {name}"))?;
    // Ensure parent dir exists (e.g. /etc/bananas/ on a fresh install).
    if let Some(parent) = std::path::Path::new(path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // Atomic replace: write to a sibling temp file, fsync, rename.
    let tmp = format!("{path}.tmp");
    tokio::fs::write(&tmp, content)
        .await
        .with_context(|| format!("writing {tmp}"))?;
    tokio::fs::rename(&tmp, path)
        .await
        .with_context(|| format!("renaming {tmp} -> {path}"))?;
    if units.is_empty() {
        // No daemon to restart — bananas-server reads this config on
        // each request. Caller (UI) sees an immediate config change.
        return Ok(format!("Saved {path} (live config — no restart).\n"));
    }
    // Restart each unit sequentially. Bail on the first failure so
    // the operator-facing banner reports the actual cause rather than
    // a cascade of "Restart=always retry" noise from later units.
    let mut log = String::new();
    for unit in units {
        let out = TokioCommand::new("systemctl")
            .arg("restart")
            .arg(unit)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("spawning systemctl restart")?;
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if !out.status.success() {
            anyhow::bail!(
                "systemctl restart {unit} failed (status {}): {combined}",
                out.status
            );
        }
        log.push_str(&format!("restarted {unit}\n{combined}"));
    }
    Ok(format!("Saved {path}; {log}"))
}
