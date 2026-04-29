//! Wire protocol + client for bananas-helper.
//!
//! Newline-delimited JSON over a Unix domain socket. One request per
//! connection; the server writes a single Response line and closes.
//! Keeps things simple to audit — no streaming, no multiplexing.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

/// Commands the unprivileged HTTP frontend can ask the root helper to
/// perform. New variants must be added carefully — each one is a privilege
/// escalation path. Validate args inside the helper, never trust the
/// caller's framing alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Atomically replace /etc/exports with `content`, then run
    /// `exportfs -rv` to reload. The helper validates that `content`
    /// parses as a sane exports file before writing.
    WriteExports { content: String },
    /// Run `exportfs -rv` without changing /etc/exports.
    ReloadExports,
    /// Atomically replace /etc/fstab with `content`, then
    /// `systemctl daemon-reload` so systemd notices new/removed mounts.
    /// Existing mounts are NOT unmounted — that's a separate explicit
    /// action — so removing a row from the file leaves the running
    /// mount in place until reboot.
    WriteFstab { content: String },
    /// Verify a system password by reading /etc/shadow and matching the
    /// stored hash. Replies with ok=true on success and a single
    /// generic ok=false on any failure (bad user, locked account, bad
    /// password) so the unprivileged frontend can't enumerate users.
    Authenticate { username: String, password: String },
    /// Run `smartctl -j -H -A <device>` and return the JSON in `output`.
    /// Device is validated against an allowlist (sda*, sdb*, …, nvme*,
    /// mmcblk*) so an attacker can't shell-inject arbitrary paths.
    /// `ok=true` if smartctl exited with a clean status; `ok=false` for
    /// drives that are missing/unreadable. The JSON in `output` is the
    /// authoritative truth — caller parses `smart_status.passed`.
    Smart { device: String },
    /// Read /etc/passwd + /etc/group + /etc/shadow and return a JSON
    /// payload describing every account on the system. The server filters
    /// system users (UID < 1000) for normal display.
    ListUsers,
    /// Same as ListUsers but additionally includes each account's
    /// /etc/shadow `password_hash` field. Used by the config-export path
    /// to round-trip accounts; never exposed via /api/users.
    ExportUsers,
    /// `useradd -m -c "<full_name>" <username>` followed by chpasswd to
    /// set the initial password. If `admin` is true, the new user is
    /// also added to `bananas-admin` so they can sign in to the UI.
    /// Username + group names are validated against POSIX portable
    /// filename charset so they can't contain shell metacharacters.
    ///
    /// `password_is_hash` lets the config-import path supply a pre-computed
    /// shadow hash (`$6$…`) instead of a plaintext password — the helper
    /// pipes via `chpasswd -e` so the hash is written to /etc/shadow as-is.
    CreateUser {
        username: String,
        password: String,
        full_name: Option<String>,
        #[serde(default)]
        admin: bool,
        #[serde(default)]
        password_is_hash: bool,
    },
    /// `userdel -r <username>` — removes the user AND their home dir.
    /// Refuses to delete root, the bananas service user, or any UID < 1000.
    DeleteUser { username: String },
    /// `chpasswd` over stdin. Refuses for system users (UID < 1000) so
    /// you can't lock yourself out of the helper by chpass-ing the
    /// `bananas` user.
    SetPassword { username: String, password: String },
    /// Add or remove `username` from the `bananas-admin` group. Used to
    /// toggle UI sign-in privilege for an existing account.
    SetAdmin { username: String, admin: bool },
    /// stat(2) the path. Response.output carries JSON with
    /// `{uid,gid,user,group,mode,kind}`. Allowlisted paths only — see
    /// `is_safe_perms_path` in the helper.
    Stat { path: String },
    /// chown / chmod a path. All three operands are optional; missing
    /// ones leave that aspect untouched. Allowlisted paths only.
    /// `mode` is the numeric mode (decimal or octal as a string).
    SetPermissions {
        path: String,
        #[serde(default)]
        uid: Option<u32>,
        #[serde(default)]
        gid: Option<u32>,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        recursive: bool,
    },
    /// Read the on-disk TOML for a known service config (allowlisted by
    /// `name` — see `service_config_path` in the helper). Reply.output
    /// is the raw TOML text. Used by the UI's stats-config modal.
    ReadServiceConfig { name: String },
    /// Atomically replace the service's config file with `content`,
    /// then `systemctl restart <unit>` so the new config takes effect.
    /// `name` is allowlisted, same as ReadServiceConfig.
    WriteServiceConfig { name: String, content: String },
    /// Run a single cloud sync entry by index. The helper reads
    /// /etc/bananas/cloud.toml, looks up the entry + its account, and
    /// invokes rclone with the right env (no on-disk rclone.conf — the
    /// account token never hits disk for the duration of the run).
    /// `output` carries combined stdout+stderr from rclone.
    RunCloudSync { idx: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    /// Combined stdout+stderr from any command run, for display in the UI.
    pub output: String,
    /// Set when ok=false. Display verbatim; the helper redacts any
    /// sensitive tail before sending.
    pub error: Option<String>,
}

impl Response {
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            ok: true,
            output: output.into(),
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: output.into(),
            error: Some(msg.into()),
        }
    }
}

/// Connect to the helper, send `cmd`, read one Response line, return it.
pub async fn call(socket: &Path, cmd: &Command) -> Result<Response> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to helper at {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut req = serde_json::to_vec(cmd).context("serializing command")?;
    req.push(b'\n');
    write_half
        .write_all(&req)
        .await
        .context("writing request")?;
    write_half.shutdown().await.ok();

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .context("reading response")?;
    serde_json::from_str(line.trim()).context("parsing response")
}
