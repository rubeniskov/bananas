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
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    process::Command as TokioCommand,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bananas_helper=info".into()),
        )
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

    let dir = exports_path.parent().unwrap_or_else(|| Path::new("/"));
    let tmp = dir.join(format!(
        ".{}.tmp",
        exports_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("exports")
    ));
    fs::write(&tmp, content)
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
    let active = count_active_exports(content);
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
        "wrote {} ({} bytes)\n{}{}\n{}",
        exports_path.display(),
        content.len(),
        mkdir_log,
        nfs_state,
        reload
    ))
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
    // `systemctl restart` is intentionally avoided here — it would
    // briefly drop active clients even if the file's only change is a
    // new row. `is-active` + start-if-not-running is enough; the actual
    // export-table push happens in `exportfs -rv` afterwards.
    let active = TokioCommand::new("systemctl")
        .args(["is-active", "nfs-server.service"])
        .output()
        .await;
    let already_active = matches!(active, Ok(o) if o.status.success());
    if already_active {
        return "nfs-server.service: active".to_string();
    }
    // Clear any stuck `failed` state (StartLimitBurst etc.) before we
    // try to start. Best-effort — if the unit is already inactive
    // rather than failed, this is a harmless no-op.
    let _ = TokioCommand::new("systemctl")
        .args(["reset-failed", "nfs-server.service"])
        .output()
        .await;
    let out = TokioCommand::new("systemctl")
        .args(["start", "nfs-server.service"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => "nfs-server.service: started".to_string(),
        Ok(o) => format!(
            "nfs-server.service: failed to start ({}): {}{}",
            o.status,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr),
        ),
        Err(e) => format!("nfs-server.service: spawn error: {e}"),
    }
}

async fn ensure_nfs_server_stopped() -> String {
    let active = TokioCommand::new("systemctl")
        .args(["is-active", "nfs-server.service"])
        .output()
        .await;
    let already_inactive = matches!(active, Ok(o) if !o.status.success());
    if already_inactive {
        return "nfs-server.service: inactive (no exports)".to_string();
    }
    let out = TokioCommand::new("systemctl")
        .args(["stop", "nfs-server.service"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => "nfs-server.service: stopped (no exports)".to_string(),
        Ok(o) => format!(
            "nfs-server.service: failed to stop ({}): {}{}",
            o.status,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr),
        ),
        Err(e) => format!("nfs-server.service: spawn error: {e}"),
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
                fs::create_dir_all(&entry.local_path).await.with_context(|| {
                    format!("creating pull target {}", entry.local_path)
                })?;
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
    // Done, regardless of outcome — the file is the "live progress"
    // contract. After the job finishes the JobManager surfaces
    // success/failure via `status`, and a stale 99% would just be
    // confusing.
    let _ = tokio::fs::remove_file(&progress_path).await;

    let combined = format!("{stdout_capture}{stderr_capture}");
    if !status.success() {
        anyhow::bail!("rclone exited with {status}: {combined}");
    }
    Ok(combined)
}

/// rclone's `--stats-one-line` lines look like:
///   "Transferred: 5.2 GiB / 12.3 GiB, 42%, 1.2 MiB/s, ETA 1h41m"
/// We pluck the `42%` out. Best-effort — if the line shape changes in a
/// future rclone release, we just lose progress (the run still works).
fn parse_rclone_progress(line: &str) -> Option<u32> {
    let line = line.trim();
    if !line.contains("Transferred:") {
        return None;
    }
    let pct_idx = line.find('%')?;
    let prefix = &line[..pct_idx];
    let digits: String = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect();
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
fn service_config_target(name: &str) -> Option<(&'static str, Option<&'static str>)> {
    match name {
        "stats" => Some(("/etc/bananas/stats.toml", Some("bananas-stats.service"))),
        // The cloud config is read by bananas-server itself on every
        // /api/cloud/* call — no daemon to restart. `None` skips the
        // post-write systemctl invocation; this also avoids the
        // recursive "server tells helper to restart server" trap.
        "cloud" => Some(("/etc/bananas/cloud.toml", None)),
        _ => None,
    }
}

async fn read_service_config(name: &str) -> Result<String> {
    let (path, _unit) = service_config_target(name)
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
    let (path, unit) = service_config_target(name)
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
    let Some(unit) = unit else {
        // No daemon to restart — bananas-server reads this config on
        // each request. Caller (UI) sees an immediate config change.
        return Ok(format!("Saved {path} (live config — no restart).\n"));
    };
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
    Ok(format!("Saved {path}; restarted {unit}.\n{combined}"))
}
