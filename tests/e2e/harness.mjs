// Boots bananas-router + bananas-webadmin + bananas-cloud as native
// (host-target) processes with all their state in a temp dir, so
// playwright can drive them at http://127.0.0.1:9080 without
// reflashing the BPI. ~30 sec/iter vs 2-3 min for the cross-build +
// opkg cycle.
//
// What this DOESN'T cover: bananas-engine (privileged-ops daemon).
// We don't run it; webadmin's helper-call handlers will return 502.
// Tests that need engine (writing /etc/exports, opkg upgrades, the
// reboot endpoint) belong in a heavier "Yocto container" harness —
// follow-up. The lightweight harness covers everything URL/SPA/MFE/
// proxy-related, which is what we keep regressing.
import { spawn } from 'node:child_process';
import fs from 'node:fs/promises';
import { openSync } from 'node:fs';
import path from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '..', '..');

export const TCP_ADDR = '127.0.0.1:9080';
export const ORIGIN = `http://${TCP_ADDR}`;

export class Harness {
  constructor() {
    this.tmp = null;
    this.children = [];
  }

  async setup() {
    this.tmp = await fs.mkdtemp(path.join('/tmp', 'bananas-e2e-'));
    const ext = path.join(this.tmp, 'extensions.d');
    await fs.mkdir(ext, { recursive: true });

    // Manifests — same shape that ships in the Yocto IPKs, just
    // with sockets pointing into our temp dir.
    await fs.writeFile(path.join(ext, 'webadmin.toml'),
      `id = "webadmin"
label = "BanaNAS"
socket = "${this.tmp}/webadmin.sock"
api_prefix = "/api"
`);
    await fs.writeFile(path.join(ext, 'cloud.toml'),
      `id = "cloud"
label = "Cloud sync"
socket = "${this.tmp}/cloud.sock"
api_prefix = "/api/cloud"
order = 50
icon = "cloud"
`);

    // Pre-create the session.key (32 random bytes) — the daemons
    // would generate one on demand, but having it before they boot
    // means we can mint a cookie before any daemon syscalls.
    const { randomBytes } = await import('node:crypto');
    await fs.writeFile(path.join(this.tmp, 'session.key'), randomBytes(32), { mode: 0o600 });

    // Empty exports + fstab files so webadmin's read paths don't
    // 500 on a missing file.
    await fs.writeFile(path.join(this.tmp, 'exports'), '');

    return this.tmp;
  }

  async build() {
    console.log('[harness] cargo build -p bananas-{router,webadmin,cloud}');
    await runToCompletion('cargo', [
      'build',
      '-p', 'bananas-router',
      '-p', 'bananas-webadmin',
      '-p', 'bananas-cloud',
    ], { cwd: REPO });
  }

  // Spawn one daemon and capture stderr to a log file so failures
  // are diagnosable post-mortem (the harness redirects stdout to
  // /dev/null so the playwright suite isn't drowned in tracing
  // output).
  spawnDaemon(name, env) {
    const bin = path.join(REPO, 'target', 'debug', name);
    const logPath = path.join(this.tmp, `${name}.log`);
    const log = openSync(logPath, 'w');
    const child = spawn(bin, [], {
      env: { ...process.env, ...env },
      stdio: ['ignore', log, log],
    });
    child.on('exit', code => {
      if (code !== 0 && code !== null) {
        console.log(`[harness] ${name} exited code=${code} (see ${logPath})`);
      }
    });
    this.children.push({ name, child, logPath });
  }

  async start() {
    const env = {
      BANANAS_EXTENSIONS_DIR: path.join(this.tmp, 'extensions.d'),
      BANANAS_ROUTER_SOCKET: path.join(this.tmp, 'router.sock'),
      BANANAS_WEBADMIN_SOCKET: path.join(this.tmp, 'webadmin.sock'),
      BANANAS_CLOUD_SOCKET: path.join(this.tmp, 'cloud.sock'),
      BANANAS_SESSION_KEY: path.join(this.tmp, 'session.key'),
      BANANAS_LISTEN_ADDR: TCP_ADDR,
      BANANAS_ENGINE_SOCKET: path.join(this.tmp, 'engine.sock'),
      BANANAS_EXPORTS_PATH: path.join(this.tmp, 'exports'),
      BANANAS_STATS_DB: path.join(this.tmp, 'stats.db'),
      BANANAS_STATS_LIVE_SOCKET: path.join(this.tmp, 'stats-live.sock'),
      BANANAS_OPERATIONS_JOURNAL: path.join(this.tmp, 'operations.json'),
      RUST_LOG: 'warn',
    };

    this.spawnDaemon('bananas-router', env);
    this.spawnDaemon('bananas-webadmin', env);
    this.spawnDaemon('bananas-cloud', env);

    // Wait for the public TCP listener — a /healthz on the public
    // app sub-proxies through router→webadmin sock→handler, which
    // exercises every hop. 5 s should be plenty for native debug
    // binaries to start.
    await waitForReady(`${ORIGIN}/api/healthz`, 5000);
  }

  async teardown() {
    for (const { name, child } of this.children) {
      try { child.kill('SIGTERM'); } catch {}
    }
    // Give them a beat to flush stderr before the temp dir vanishes.
    await sleep(150);
    if (this.tmp && process.env.BANANAS_E2E_KEEP !== '1') {
      await fs.rm(this.tmp, { recursive: true, force: true });
    }
  }

  sessionKeyPath() {
    return path.join(this.tmp, 'session.key');
  }
}

async function runToCompletion(cmd, args, opts) {
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { stdio: 'inherit', ...opts });
    child.on('exit', code => code === 0 ? resolve() : reject(new Error(`${cmd} exited ${code}`)));
    child.on('error', reject);
  });
}

async function waitForReady(url, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let lastErr;
  while (Date.now() < deadline) {
    try {
      const r = await fetch(url);
      // /healthz is auth-gated → 401. /healthz is in webadmin's
      // public exempt list, so it's actually 200. Either way, a
      // numeric HTTP status proves the listener is up.
      if (typeof r.status === 'number') return;
    } catch (e) {
      lastErr = e;
    }
    await sleep(100);
  }
  throw new Error(`waitForReady ${url} timed out: ${lastErr?.message ?? 'no response'}`);
}
