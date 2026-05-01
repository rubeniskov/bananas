//! Yocto-shaped container harness — boots the daemon stack inside a
//! Docker container so engine privileged ops (writing /etc/exports,
//! useradd, opkg) can run as real root without giving the host's
//! test process root. Companion to the lightweight `Harness` (which
//! runs the same daemons in a tmpdir as the test user, skipping
//! privileged paths).
//!
//! Image source: `tests/container/Dockerfile` + the host's
//! `target/debug/bananas-*` binaries. Build the image once with
//! `pixi run build-test-container`; this module just spins up
//! disposable containers from it.
//!
//! Every `Container::launch()` allocates a random host port via
//! Docker's `-P` (publish all). Multiple containers run in parallel
//! safely.

use std::{
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use tokio::{process::Command, time::sleep};

/// Image tag the `pixi run build-test-container` task produces.
pub const IMAGE_TAG: &str = "bananas-test:latest";

/// Test password baked into the container image's `/etc/shadow`.
/// Same value as the lightweight harness's `TEST_PASSWORD` so a
/// single `Harness::login("root", TEST_PASSWORD)` works against
/// either target.
pub const CONTAINER_TEST_PASSWORD: &str = "bananas-test";

pub struct Container {
    id: String,
    host_port: u16,
    http: reqwest::Client,
}

impl Container {
    /// `docker run -d --rm -P bananas-test:latest`. Resolves the
    /// host-side port by running `docker port <id> 8080/tcp`, then
    /// polls `/api/healthz` until the daemon stack is ready (5 s
    /// budget — same as the lightweight harness).
    pub async fn launch() -> Result<Self> {
        let id = run_detached(&["-P", IMAGE_TAG]).await?;
        let host_port = container_port(&id, 8080)
            .await
            .with_context(|| format!("resolving published host port for container {id}"))?;

        let http = reqwest::Client::builder()
            .cookie_store(true)
            .timeout(Duration::from_secs(10))
            .build()
            .context("reqwest client")?;

        let me = Self {
            id,
            host_port,
            http,
        };
        me.wait_for_ready()
            .await
            .with_context(|| format!("container {} never answered /api/healthz", me.id))?;
        Ok(me)
    }

    /// `docker exec` into the running container. Returns
    /// `(stdout, stderr, exit_code)`. Useful for opkg / shadow / fs
    /// assertions that need to run inside the container's namespace
    /// (e.g. `cat /etc/exports` after `Command::WriteExports`).
    pub async fn exec(&self, argv: &[&str]) -> Result<ExecOutput> {
        let mut cmd = Command::new("docker");
        cmd.arg("exec").arg(&self.id);
        for a in argv {
            cmd.arg(a);
        }
        let out = cmd.output().await.context("docker exec")?;
        Ok(ExecOutput {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            status: out.status.code().unwrap_or(-1),
        })
    }

    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.host_port)
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// POST `/api/login` against the container's listener. Same
    /// shape as `Harness::login` so tests can swap targets without
    /// changing assertions.
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
            bail!("container login failed: status={status} body={body}");
        }
        resp.headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .find_map(|v| {
                let s = v.to_str().ok()?;
                let kv = s.split(';').next()?.trim();
                let v = kv.strip_prefix("bananas_session=")?;
                Some(v.to_string())
            })
            .ok_or_else(|| anyhow!("/api/login returned no bananas_session cookie"))
    }

    async fn wait_for_ready(&self) -> Result<()> {
        let url = format!("{}/api/healthz", self.origin());
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last_err: Option<anyhow::Error> = None;
        while Instant::now() < deadline {
            match self.http.get(&url).send().await {
                Ok(_) => return Ok(()),
                Err(e) => last_err = Some(e.into()),
            }
            sleep(Duration::from_millis(150)).await;
        }
        Err(last_err.unwrap_or_else(|| anyhow!("/api/healthz never answered")))
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        // `--rm` was set on launch, so docker reaps the container
        // automatically once it stops. SIGTERM via `docker stop -t 1`
        // is fast — entrypoint.sh traps and propagates.
        let _ = std::process::Command::new("docker")
            .args(["stop", "-t", "1", &self.id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[derive(Debug)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

impl ExecOutput {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

async fn run_detached(args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("docker");
    cmd.arg("run").arg("-d").arg("--rm");
    for a in args {
        cmd.arg(a);
    }
    let out = cmd.output().await.context("docker run")?;
    if !out.status.success() {
        bail!(
            "docker run failed: status={} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if id.is_empty() {
        bail!("docker run returned empty container id");
    }
    Ok(id)
}

async fn container_port(id: &str, container_port: u16) -> Result<u16> {
    // `docker port <id> 8080/tcp` prints lines like
    // `0.0.0.0:32769` (one per published address family). Take the
    // first one with a numeric port.
    let out = Command::new("docker")
        .args(["port", id, &format!("{container_port}/tcp")])
        .output()
        .await
        .context("docker port")?;
    if !out.status.success() {
        bail!(
            "docker port failed: status={} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if let Some(port_str) = line.rsplit(':').next()
            && let Ok(port) = port_str.trim().parse::<u16>()
        {
            return Ok(port);
        }
    }
    bail!("docker port {id} {container_port}/tcp produced no parseable host port:\n{stdout}")
}
