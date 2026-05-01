//! Per-test harness. Boots `bananas-engine` + the 7 plugin daemons +
//! `bananas-router` + `bananas-webadmin` against an isolated tmpdir
//! and a random TCP port. `Harness::login()` performs a real
//! `/api/login` POST against the same origin a browser sees.
//!
//! Every harness instance is hermetic — its own port, its own tmpdir,
//! its own session.key, its own /etc/shadow stub. Tests can run in
//! parallel under `cargo test --jobs N` without coordinating.

use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use tempfile::TempDir;
use tokio::{
    process::{Child, Command},
    time::sleep,
};

/// Default test password baked into the harness's `/etc/shadow`
/// stub. Tests log in with `Harness::login("root", TEST_PASSWORD)`.
/// Container-mode harness (PR-6) provisions the same password
/// inside the rootfs so the same call works against either target.
pub const TEST_PASSWORD: &str = "bananas-test";

/// All plugin daemons we spawn. Tuple of (binary name, manifest id,
/// label, api prefix, order, icon). The manifest contents need to
/// match what the plugin .ipks ship in /etc/bananas/extensions.d/
/// for routing to work.
const PLUGINS: &[(&str, &str, &str, &str, u32, &str)] = &[
    ("bananas-cloud", "cloud", "Cloud", "/api/cloud", 50, "cloud"),
    (
        "bananas-exports",
        "exports",
        "Exports",
        "/api/exports",
        20,
        "share-2",
    ),
    (
        "bananas-storage",
        "storage",
        "Storage",
        "/api/storage",
        30,
        "hard-drive",
    ),
    ("bananas-users", "users", "Users", "/api/users", 40, "users"),
    (
        "bananas-stats-web",
        "stats",
        "Stats",
        "/api/stats",
        10,
        "chart-bar",
    ),
    (
        "bananas-dashboard-web",
        "dashboard",
        "Dashboard",
        "/api/dashboard",
        60,
        "layout-grid",
    ),
];

/// A running test environment. Drop it to tear everything down.
pub struct Harness {
    /// `Some(TempDir)` when the harness owns the cleanup. `None`
    /// when `BANANAS_E2E_KEEP=1` was set — TempDir::keep() is
    /// called so the dir survives the test run for post-mortem
    /// inspection. The held `Option` keeps the destructor inert
    /// in the keep-mode case.
    #[allow(dead_code)]
    tmp: Option<TempDir>,
    /// Path to whichever tmp the harness used — set in both modes
    /// so `Harness::tmp()` keeps working.
    tmp_path: std::path::PathBuf,
    port: u16,
    children: Vec<(String, Child)>,
    http: reqwest::Client,
}

/// Type alias used by tests that pass the harness around without
/// owning it (`HarnessHandle = &Harness`). Keeps call signatures
/// short.
pub type HarnessHandle<'a> = &'a Harness;

impl Harness {
    /// Bring up engine + every plugin daemon. Blocks until the
    /// public TCP listener answers a `/api/healthz` probe (or
    /// times out at 5 s).
    pub async fn new() -> Result<Self> {
        let tmp = tempfile::tempdir().context("tempdir")?;
        let port = pick_free_port()?;

        let target = workspace_target_dir();
        let bin = |name: &str| target.join(name);

        // Pre-write everything the daemons read at startup, BEFORE
        // we spawn any of them.
        write_extensions_dir(tmp.path())?;
        write_session_key(tmp.path())?;
        write_shadow_with_test_root(tmp.path())?;
        std::fs::write(tmp.path().join("exports"), "").context("write empty exports")?;
        std::fs::write(tmp.path().join("fstab"), default_fstab_fixture())
            .context("write default fstab fixture")?;

        let env_base = base_env(tmp.path(), port);

        let mut children: Vec<(String, Child)> = Vec::new();

        // Spawn engine first — plugin daemons hard-Require=engine
        // in production, so we mirror that ordering here.
        let engine = spawn(&bin("bananas-engine"), &env_base)?;
        children.push(("bananas-engine".into(), engine));

        // Router + webadmin are the API gateway; spawn them after
        // engine so socket ordering matches production.
        let router = spawn(&bin("bananas-router"), &env_base)?;
        children.push(("bananas-router".into(), router));
        let webadmin = spawn(&bin("bananas-webadmin"), &env_base)?;
        children.push(("bananas-webadmin".into(), webadmin));

        // Plugin daemons in any order — each binds its own socket.
        for (binary, _id, _label, _prefix, _order, _icon) in PLUGINS {
            let child = spawn(&bin(binary), &env_base)?;
            children.push((binary.to_string(), child));
        }

        // Cookie store stays on the client so tests don't have to
        // re-attach the session header on every request after login.
        let http = reqwest::Client::builder()
            .cookie_store(true)
            .timeout(Duration::from_secs(10))
            .build()
            .context("reqwest client")?;

        let tmp_path = tmp.path().to_path_buf();
        let keep = std::env::var_os("BANANAS_E2E_KEEP").is_some();
        let tmp_owned = if keep {
            // Surface the kept path on stderr so a CI/debug log
            // can grep it out.
            eprintln!("[harness] keeping tmp dir: {}", tmp_path.display());
            let _ = tmp.keep();
            None
        } else {
            Some(tmp)
        };
        let h = Self {
            tmp: tmp_owned,
            tmp_path,
            port,
            children,
            http,
        };
        h.wait_for_ready().await?;
        Ok(h)
    }

    /// `http://127.0.0.1:<port>` — same shape the browser sees.
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Path to the harness's tmpdir. Tests can read daemon log
    /// files (`<bin>.log`) here when debugging a flaky run.
    pub fn tmp(&self) -> &Path {
        &self.tmp_path
    }

    /// Shared `reqwest::Client` with cookie storage enabled. After
    /// `login()`, follow-up requests on this client are automatically
    /// authenticated.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// POST `/api/login` with `username` + `password`. On success
    /// the response's `Set-Cookie: bananas_session=…` is captured by
    /// the cookie jar, and the cookie value is returned so callers
    /// can also attach it manually (e.g. to a separate browser
    /// context).
    pub async fn login(&self, username: &str, password: &str) -> Result<String> {
        #[derive(serde::Serialize)]
        struct Req<'a> {
            username: &'a str,
            password: &'a str,
        }
        let url = format!("{}/api/login", self.origin());
        let resp = self
            .http
            .post(&url)
            .json(&Req { username, password })
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("login failed: status={status} body={body}");
        }
        // Pluck the cookie out of the Set-Cookie header (the
        // response from /api/login). Some tests want the raw value
        // to attach to a non-reqwest client.
        let set_cookie = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .find_map(|v| {
                let s = v.to_str().ok()?;
                let kv = s.split(';').next()?.trim();
                let v = kv.strip_prefix("bananas_session=")?;
                Some(v.to_string())
            })
            .ok_or_else(|| anyhow!("/api/login returned no bananas_session cookie"))?;
        Ok(set_cookie)
    }

    async fn wait_for_ready(&self) -> Result<()> {
        let url = format!("{}/api/healthz", self.origin());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut last_err: Option<anyhow::Error> = None;
        while std::time::Instant::now() < deadline {
            match self.http.get(&url).send().await {
                Ok(_) => return Ok(()),
                Err(e) => last_err = Some(e.into()),
            }
            sleep(Duration::from_millis(80)).await;
        }
        Err(last_err.unwrap_or_else(|| anyhow!("/api/healthz never answered")))
            .with_context(|| format!("waiting on {url}"))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        for (_, child) in self.children.iter_mut() {
            let _ = child.start_kill();
        }
    }
}

fn spawn(bin: &Path, env: &[(String, std::ffi::OsString)]) -> Result<Child> {
    if !bin.exists() {
        anyhow::bail!(
            "binary missing: {}. Run `cargo build --workspace --bins` first.",
            bin.display()
        );
    }
    let mut cmd = Command::new(bin);
    for (k, v) in env {
        cmd.env(k, v);
    }
    // Tee to a per-binary log file under the harness tmp so tests
    // can fish out daemon stderr when something goes wrong. The
    // tmp lives only for the harness's lifetime so this doesn't
    // leak.
    let log_dir = std::path::PathBuf::from(
        env.iter()
            .find(|(k, _)| k == "BANANAS_OPERATIONS_JOURNAL")
            .map(|(_, v)| {
                std::path::Path::new(v)
                    .parent()
                    .unwrap_or(std::path::Path::new("/tmp"))
            })
            .unwrap_or(std::path::Path::new("/tmp"))
            .as_os_str(),
    );
    let log_path = log_dir.join(format!(
        "{}.log",
        bin.file_name().and_then(|s| s.to_str()).unwrap_or("daemon")
    ));
    let log_file = std::fs::File::create(&log_path).ok();
    if let Some(f) = log_file {
        let f2 = f.try_clone().expect("dup log fd");
        cmd.stdout(Stdio::from(f)).stderr(Stdio::from(f2));
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
    cmd.spawn()
        .with_context(|| format!("spawn {}", bin.display()))
}

/// Build the env block every daemon shares. Sockets all live in
/// `<tmp>/`; the public listener owns `port`.
fn base_env(tmp: &Path, port: u16) -> Vec<(String, std::ffi::OsString)> {
    let join = |name: &str| tmp.join(name).into_os_string();
    let e: Vec<(String, std::ffi::OsString)> = vec![
        (
            "BANANAS_EXTENSIONS_DIR".into(),
            tmp.join("extensions.d").into_os_string(),
        ),
        ("BANANAS_ROUTER_SOCKET".into(), join("router.sock")),
        ("BANANAS_WEBADMIN_SOCKET".into(), join("webadmin.sock")),
        ("BANANAS_ENGINE_SOCKET".into(), join("engine.sock")),
        ("BANANAS_CLOUD_SOCKET".into(), join("cloud.sock")),
        ("BANANAS_EXPORTS_SOCKET".into(), join("exports.sock")),
        ("BANANAS_STORAGE_SOCKET".into(), join("storage.sock")),
        ("BANANAS_USERS_SOCKET".into(), join("users.sock")),
        ("BANANAS_STATS_WEB_SOCKET".into(), join("stats-web.sock")),
        (
            "BANANAS_DASHBOARD_WEB_SOCKET".into(),
            join("dashboard-web.sock"),
        ),
        ("BANANAS_SESSION_KEY".into(), join("session.key")),
        (
            "BANANAS_LISTEN_ADDR".into(),
            format!("127.0.0.1:{port}").into(),
        ),
        ("BANANAS_EXPORTS_PATH".into(), join("exports")),
        ("BANANAS_SHADOW_PATH".into(), join("shadow")),
        ("BANANAS_FSTAB_PATH".into(), join("fstab")),
        ("BANANAS_STATS_DB".into(), join("stats.db")),
        ("BANANAS_STATS_LIVE_SOCKET".into(), join("stats-live.sock")),
        ("BANANAS_OPERATIONS_JOURNAL".into(), join("operations.json")),
        ("RUST_LOG".into(), "warn".into()),
    ];
    e
}

/// Write the manifest TOMLs the router reads at startup.
fn write_extensions_dir(tmp: &Path) -> Result<()> {
    let dir = tmp.join("extensions.d");
    std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;

    // webadmin always carries the catch-all — same as production.
    let webadmin = format!(
        r#"id = "webadmin"
label = "BanaNAS"
socket = "{sock}"
api_prefix = "/api"
"#,
        sock = tmp.join("webadmin.sock").display()
    );
    std::fs::write(dir.join("webadmin.toml"), webadmin)?;

    for (_, id, label, prefix, order, icon) in PLUGINS {
        let sock = tmp.join(format!("{}.sock", manifest_socket_name(id)));
        let toml = format!(
            r#"id = "{id}"
label = "{label}"
socket = "{sock}"
api_prefix = "{prefix}"
order = {order}
icon = "{icon}"
"#,
            sock = sock.display()
        );
        std::fs::write(dir.join(format!("{id}.toml")), toml)?;
    }
    Ok(())
}

/// Default fstab fixture written into every Harness's tmpdir.
/// Mirrors a real BPI's /etc/fstab — the protected list expands
/// to include every kernel virtual fs + cgroup mount the BPI
/// shows when the operator toggles "Show protected mounts" so
/// table-layout regressions surface on a representative row
/// count, not just the 3 entries a minimal fixture would.
fn default_fstab_fixture() -> &'static str {
    "# Auto-written by bananas-devtool::Harness for tests.\n\
     proc            /proc           proc       defaults                              0 0\n\
     sysfs           /sys            sysfs      defaults                              0 0\n\
     devtmpfs        /dev            devtmpfs   defaults                              0 0\n\
     devpts          /dev/pts        devpts     defaults,gid=5,mode=620               0 0\n\
     tmpfs           /dev/shm        tmpfs      defaults                              0 0\n\
     tmpfs           /run            tmpfs      defaults,mode=755                     0 0\n\
     tmpfs           /tmp            tmpfs      defaults                              0 0\n\
     tmpfs           /var/volatile   tmpfs      defaults                              0 0\n\
     cgroup2         /sys/fs/cgroup  cgroup2    defaults                              0 0\n\
     debugfs         /sys/kernel/debug debugfs  defaults                              0 0\n\
     LABEL=ROOT      /               ext4       defaults,noatime                      0 1\n\
     LABEL=BOOT      /boot           vfat       defaults,noatime                      0 2\n\
     LABEL=media     /srv/media      ext4       defaults,noatime,nofail               0 2\n\
     LABEL=services  /srv/services   ext4       defaults,noatime,nofail               0 2\n\
     UUID=11111111-1111-1111-1111-111111111111  /mnt/extra  ext4  defaults,noatime,nofail  0 2\n"
}

/// Plugins whose binary name has a `-web` suffix listen on a socket
/// named after the binary (e.g. stats-web.sock); plain plugins use
/// just the manifest id.
fn manifest_socket_name(id: &str) -> String {
    match id {
        "stats" => "stats-web".into(),
        "dashboard" => "dashboard-web".into(),
        other => other.into(),
    }
}

fn write_session_key(tmp: &Path) -> Result<()> {
    use rand::TryRngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|e| anyhow!("OsRng: {e}"))?;
    let path = tmp.join("session.key");
    std::fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn write_shadow_with_test_root(tmp: &Path) -> Result<()> {
    let params = sha_crypt::Sha512Params::new(sha_crypt::ROUNDS_DEFAULT)
        .map_err(|e| anyhow!("Sha512Params: {e:?}"))?;
    let hash = sha_crypt::sha512_simple(TEST_PASSWORD, &params)
        .map_err(|e| anyhow!("sha512_simple: {e:?}"))?;
    let line = format!("root:{hash}:20000:0:99999:7:::\n");
    std::fs::write(tmp.join("shadow"), line).context("write shadow")?;
    Ok(())
}

fn pick_free_port() -> Result<u16> {
    // Bind to :0, read the kernel-assigned port, drop the listener.
    // The next process to bind that port wins. Race-y in theory but
    // the daemon binds within ms, so collisions are vanishingly rare
    // even at high test parallelism.
    let listener = TcpListener::bind("127.0.0.1:0").context("bind 127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Walk up from this crate's manifest dir until we find the
/// workspace `Cargo.toml`, then return its `target/<profile>` dir.
/// Honours `CARGO_TARGET_DIR` if set.
fn workspace_target_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(d).join(profile());
    }
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let cargo = p.join("Cargo.toml");
        if cargo.exists() {
            let s = std::fs::read_to_string(&cargo).unwrap_or_default();
            if s.contains("[workspace]") {
                return p.join("target").join(profile());
            }
        }
        if !p.pop() {
            panic!("walked past filesystem root looking for workspace Cargo.toml");
        }
    }
}

fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}
