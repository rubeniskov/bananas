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

/// One of the five binary / asset components shipped per release. Used
/// by `InstallUpdate` to select the install target + the systemd unit
/// to bounce afterwards. Snake-case wire form matches the GitHub asset
/// names (`bananas-stats-armv7.tar.gz` → `stats`, etc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Server,
    Helper,
    Stats,
    Dashboard,
    Webadmin,
}

impl Component {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Helper => "helper",
            Self::Stats => "stats",
            Self::Dashboard => "dashboard",
            Self::Webadmin => "webadmin",
        }
    }
}

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
    /// Self-service password change: verifies `old_password` against
    /// /etc/shadow and only on success writes `new_password`. Allowed for
    /// root (no UID gate) — that's the whole point: the firstboot
    /// shadow-expiry hook leaves root with lastchg=0, and this command
    /// is the only way the UI can clear it. The login flow surfaces
    /// `error == "password_expired"` to trigger this on the client.
    ChangeOwnPassword {
        username: String,
        old_password: String,
        new_password: String,
    },
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
    /// Stop a running cloud sync. The helper records each rclone child's
    /// PID at `/run/bananas/sync-progress/<idx>.pid` while the run is
    /// in flight; this reads it back and SIGTERMs the process. The
    /// original `RunCloudSync` call returns shortly after with a
    /// non-zero exit, which the JobManager picks up and surfaces as a
    /// failed run.
    CancelCloudSync { idx: usize },
    /// Soft-reboot the BPI via `systemctl reboot`. The helper response
    /// fires before systemd actually reboots, so the operator's web
    /// UI gets a confirmation banner; the connection then drops as
    /// the daemon goes down.
    RebootSystem,
    /// `mkdir -p <path>` for a fresh mountpoint. Allowlisted to the
    /// same prefixes as Stat/SetPermissions (/srv, /mnt, /media, /home,
    /// /opt) so the UI can pre-create a directory before adding an
    /// fstab entry that mounts onto it. Idempotent: no-op when the
    /// path already exists.
    MakeDirectory { path: String },
    /// `lsblk -J -b -o NAME,KNAME,SIZE,MODEL,TYPE,MOUNTPOINT,FSTYPE,LABEL,UUID,RO`
    /// returned verbatim in `output`. The unprivileged bananas-server
    /// user can run lsblk too, but blkid (which lsblk calls
    /// internally for FSTYPE/LABEL/UUID) needs raw-read on /dev/sd*
    /// — and /dev/sd* are mode 0660 root:disk. Routing the call
    /// through the root helper keeps bananas-server out of the disk
    /// group while still surfacing complete partition metadata to
    /// the storage tab.
    Lsblk,
    /// `timedatectl set-timezone <tz>`. Validates `tz` against the
    /// IANA tzdata zoneinfo database (must be a real file under
    /// /usr/share/zoneinfo/) before invoking timedatectl, so a bogus
    /// string can't be passed through. Used by the Settings → General
    /// tab and the first-boot geoip auto-detection path.
    SetTimezone { tz: String },
    /// Walk /usr/share/zoneinfo and return every IANA tzdata name as a
    /// JSON array. Backs the Settings → General timezone picker so the
    /// operator gets autocomplete over the OS's actual zone list rather
    /// than a hand-maintained subset. Skips the magic top-level files
    /// (Etc/, posix/, right/) that aren't user-facing zones.
    ListTimezones,
    /// Verify, extract, and atomically swap a per-component release
    /// tarball into place, then bounce the relevant systemd unit. The
    /// helper requires `tarball_path` to live under the staging dir
    /// (`/var/lib/bananas/updates/staging/`), computes the file's
    /// SHA-256 itself, and bails on any mismatch with `expected_sha256`
    /// before touching `/usr/bin` or `/usr/share/bananas`. For binary
    /// components the extracted file's `--version` is also checked
    /// against `expected_version`. On failure after the swap begins,
    /// the previous binary is rolled back from `.bak`.
    ///
    /// Step 3 wires this for `Stats`, `Dashboard`, and `Webadmin`. The
    /// `Server` and `Helper` variants reject with "unsupported" until
    /// the self-update primitive lands (see PrepareHelperUpdate).
    InstallUpdate {
        component: Component,
        tarball_path: String,
        expected_version: String,
        expected_sha256: String,
    },
    /// Read /etc/bananas/versions.toml and return the raw TOML in
    /// `output`. Empty string when the file doesn't exist (fresh
    /// install — every component's "installed version" is unknown
    /// until the first InstallUpdate). Server uses this as the cache
    /// for /api/version; falls back to running `<bin> --version`
    /// when a row is missing.
    ReadVersions,
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
