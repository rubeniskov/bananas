# Architecture

The bits a plugin author needs to understand: who runs as what, who owns what state, who talks to whom.

## Process map

```
                    ┌──────────────────────────────────┐
LAN  ─── tcp:80 ───▶│ bananas-router (bananas user)    │
                    │   /api/<id>/*  → plugin sockets  │
                    │   /assets/<id>/* → webadmin sub  │
                    │   everything else → webadmin     │
                    └────────────┬─────────────────────┘
                                 │ loopback
                    ┌────────────▼─────────────────────┐
                    │ bananas-webadmin (bananas user)  │
                    │   embedded SPA + session cookies │
                    │   + sub-proxy for plugin assets  │
                    └────────────┬─────────────────────┘
                                 │ /run/bananas/engine.sock (newline JSON)
                    ┌────────────▼─────────────────────┐
                    │ bananas-engine (root)            │
                    │   /etc/exports, /etc/fstab,      │
                    │   /etc/passwd, smartctl, opkg,   │
                    │   reboot, exec rclone, …         │
                    └──────────────────────────────────┘

  Plugin daemon (bananas-cloud, bananas-dashboard, …):
    runs as `bananas` user, listens on /run/bananas/<id>.sock,
    calls /run/bananas/engine.sock for any privileged op.
```

## The privilege model

There is exactly one root daemon: **`bananas-engine`**. Every other BanaNAS process runs as the unprivileged `bananas` user.

Plugins **do not get root**. If a plugin needs to write `/etc/foo` or exec a privileged command (mount, useradd, smartctl, opkg upgrade, reboot), it sends a JSON request to `/run/bananas/engine.sock`. The engine validates it against an allowlist and runs it.

Why: the public web admin (`bananas-webadmin`) is the largest attack surface. Keeping it unprivileged means a webadmin RCE doesn't get the attacker root. The engine's RPC surface is small and easy to audit; the webadmin's HTTP+wasm surface is not.

For plugins that need to drop further (e.g. cloud sync runs `rclone` as the `bananas` user but the engine is the one that exec's it), the engine handles the privilege drop before exec — the plugin doesn't have to.

## State ownership

The engine owns every persistent piece of system state:

| State | File | Owner |
|---|---|---|
| NFS exports | `/etc/exports` | `bananas-engine` |
| Mount points | `/etc/fstab` | `bananas-engine` |
| Users / groups | `/etc/passwd`, `/etc/shadow`, `/etc/group` | `bananas-engine` |
| Per-service config | `/etc/bananas/*.toml` | `bananas-engine` (plugins read it) |
| Stats DB | `/var/lib/bananas/stats.db` | `bananas-stats` (its own data, not engine-owned) |
| Session key | `/var/lib/bananas/session.key` | `bananas-webadmin` (used by every plugin to validate cookies) |

A plugin may have **its own data** (logs, caches, a SQLite DB) under `/var/lib/bananas/<id>/`, but **system-level state goes through the engine**. A plugin must never write `/etc/anything` directly.

## The router: URL prefix mounts

`bananas-router` is the only process listening on the public TCP port (`:80`). It routes by URL prefix:

| Prefix | Goes to |
|---|---|
| `/api/<id>/*` (where `<id>` is a plugin id) | the plugin's Unix socket `/run/bananas/<id>.sock` |
| `/assets/<id>/*` | `bananas-webadmin`, which sub-proxies the request to the plugin's socket so the plugin can serve its own SPA assets |
| `/api/*` (no plugin id match) | `bananas-webadmin` (the core admin API: storage, users, exports, stats, …) |
| everything else | `bananas-webadmin` (the SPA shell, static assets, login page) |

The router rebuilds its routing table at startup by reading `/etc/bananas/extensions.d/`. It does not consult the engine for this; it's a pure file-based discovery.

## The webadmin: SPA shell + sub-proxy

`bananas-webadmin` does two jobs:

1. **Serves the shell SPA** at `/` — the BanaNAS top-bar, login form, the always-on tabs (Stats, Exports, Storage, Users), the `/api/*` core endpoints. The SPA is a Dioxus 0.7 wasm app embedded in the daemon binary at compile time via `include_dir!`.
2. **Sub-proxies plugin assets.** When a plugin is installed, the shell's MFE loader fetches `/assets/<id>/index.html`, parses out the plugin's content-hashed JS shim, and injects a `<script type="module">` tag at runtime. The new tab appears in the SPA without a page reload (after a router/webadmin restart picks up the manifest change).

## The extension manifest

This is the **only piece of metadata** that wires a plugin into the rest of the system. One file per plugin, dropped at install time:

```toml
# /etc/bananas/extensions.d/<id>.toml
id          = "cloud"               # short kebab-case identifier; matches the bananas-<id> package name
label       = "Cloud"               # human-readable tab label
socket      = "/run/bananas/cloud.sock"   # where the plugin daemon listens
api_prefix  = "/api/cloud"          # router routes this prefix to `socket`
order       = 50                    # ordering hint for the SPA's tab list (lower = earlier)
icon        = "cloud"               # icon name from the SPA's icon set
```

Both `bananas-router` and `bananas-webadmin` re-read the directory at startup. After dropping a new manifest, a plugin's `pkg_postinst` restarts both daemons — the new tab appears within ~50 ms.

There is no API for "registering" a plugin at runtime. Drop-the-file-and-restart is the entire mechanism. This makes plugins inspectable (`ls /etc/bananas/extensions.d/`) and trivially uninstallable (`opkg remove bananas-<id>` deletes the file and restarts the daemons).

## The cross-compile pipeline

Plugins are Rust crates in the workspace at `crates/<name>/`. The release pipeline cross-compiles them to armv7 (Banana Pro) and aarch64 (RPi-unified) using `cargo-zigbuild`, then the Yocto recipe takes the prebuilt binary and packages it as an IPK.

Why prebuilt + Yocto-packaged instead of "let bitbake compile the Rust crate from source": cargo-zigbuild on a beefy x86_64 CI host is much faster than rustc-on-bitbake's host-emulated armv7 dance, and it dodges the `meta-lts-mixins/scarthgap/rust` toolchain version drift. Trade-off: the recipe's `do_compile` is a no-op (`do_compile[noexec] = "1"`) and the "build" happens in `pixi run build-webadmin-arm` (or the per-arch sibling) ahead of the bake.

This is internal pipeline detail — when you write a plugin, you add an `IMAGE_INSTALL +=` line and the cross-compile + bake just works.
