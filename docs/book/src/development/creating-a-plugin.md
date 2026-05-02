# Creating a plugin (IPK)

A walkthrough — building a hypothetical `bananas-foo` plugin from scratch. Reference the existing `bananas-cloud` recipe (`layers/meta-bananas/recipes-bsp/bananas-cloud/`) when you want to see a real one.

A plugin is the sum of three things:

1. A **Rust crate** at `crates/foo/` that implements the daemon (and optionally an embedded SPA).
2. A **Yocto recipe** at `layers/meta-bananas/recipes-bsp/bananas-foo/bananas-foo.bb` that packages the binary.
3. An **extension manifest** TOML at `/etc/bananas/extensions.d/foo.toml` (shipped by the recipe) that tells `bananas-router` and `bananas-webadmin` how to wire it in.

That's the entire surface. If your plugin doesn't have a UI it can skip the SPA piece; if it doesn't need to call the engine it just listens on its socket and serves `/api/foo/*` requests.

## 1. Add the crate

```bash
cd crates
cargo new --lib foo                   # or --bin if you don't need the lib split
```

Edit `crates/foo/Cargo.toml`:

```toml
[package]
name        = "bananas-foo"
version     = { workspace = true }
edition     = { workspace = true }
license     = "MIT"

[[bin]]
name = "bananas-foo"
path = "src/main.rs"

# Only needed if you ship a wasm SPA — see "Optional: embedded SPA" below.
[[bin]]
name              = "bananas-foo-ui"
path              = "src/ui/main.rs"
required-features = ["wasm-ui"]

[features]
default  = []
wasm-ui  = []

[dependencies]
# pull whatever runtime you need (axum, tokio, serde…); see crates/cloud/Cargo.toml
# for the canonical dep set used by sister plugins
```

Add the crate to the workspace:

```toml
# Cargo.toml (root)
[workspace]
members = [
  # … existing members …
  "crates/foo",
]
```

The daemon entry point (`crates/foo/src/main.rs`) at minimum:

- Listens on the Unix socket given by env var `BANANAS_FOO_SOCKET` (default: `/run/bananas/foo.sock`).
- Implements your `/api/foo/*` HTTP handlers (axum + hyper-util's `UnixListener`).
- For privileged work, opens `/run/bananas/engine.sock` and sends newline-delimited JSON `{"op": "...", "args": {…}}`.
- For session validation, reads the shared `BANANAS_SESSION_KEY` (`/var/lib/bananas/session.key`) and verifies the cookie set by `bananas-webadmin`.

`crates/cloud/src/main.rs` is the closest reference — copy its top-level structure and adapt the route handlers.

## 2. Optional: embedded SPA

If your plugin has its own tab in the web admin, ship a Dioxus 0.7 wasm SPA. Two-binary pattern, same as `bananas-cloud`:

- `src/main.rs` — the daemon (native armv7 / aarch64).
- `src/ui/main.rs` — the wasm SPA, gated behind `required-features = ["wasm-ui"]` so the workspace's `cargo check` skips it.
- `build.rs` — runs `dx build --bin bananas-foo-ui --features wasm-ui --platform web`, copies the dist tree into `$OUT_DIR/ui/`, pre-compresses with brotli + gzip.
- `src/embedded.rs` — a custom axum handler that uses `include_dir!("$OUT_DIR/ui")` to serve the SPA from bytes baked into the daemon binary.

The build.rs strips `cargo-zigbuild`'s armv7-targeted env vars before invoking dx, so the recursive cargo-for-wasm32 build inside dx isn't poisoned by the outer host's cross-compile flags. Copy that block from `crates/cloud/build.rs` verbatim.

The webadmin's MFE loader reaches your SPA at `/assets/foo/index.html` — the router routes `/assets/<id>/*` to the webadmin, which sub-proxies to your daemon's socket.

## 3. Write the Yocto recipe

Create `layers/meta-bananas/recipes-bsp/bananas-foo/bananas-foo.bb`:

```bitbake
SUMMARY = "BanaNAS foo plugin (one-line description)"
DESCRIPTION = "Longer paragraph describing what the plugin does, what \
it depends on, and what triggers an install. Mention any heavy \
dependencies that justify why it is opt-in."

LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

require recipes-bsp/bananas-version.inc

inherit systemd

# Pull files from the recipe's files/ dir AND from the prebuilt
# binary staged into serve/bin/ by `pixi run build-webadmin-arm`.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:${COREBASE}/../serve/bin:"
SRC_URI = "file://bananas-foo.service \
           file://bananas-foo \
           file://foo.toml"

S = "${WORKDIR}"

# Restrict to boards this plugin actually works on. Widen later if it
# becomes board-agnostic.
COMPATIBLE_MACHINE = "(bananapro)"

# The binary is prebuilt by cargo-zigbuild — bitbake mustn't re-strip
# or re-process it.
INHIBIT_PACKAGE_STRIP = "1"
INSANE_SKIP:${PN} += "arch already-stripped"

SYSTEMD_SERVICE:${PN} = "bananas-foo.service"
SYSTEMD_AUTO_ENABLE = "enable"

# Hard runtime deps:
#   bananas-router — reads our manifest at startup to route /api/foo/*
#                    to our socket.
#   bananas-webadmin — owns the public TCP port and sub-proxies our
#                      assets at /assets/foo/*.
#   bananas-engine — only if your plugin makes engine RPC calls.
RDEPENDS:${PN} += "bananas-router bananas-webadmin"

# The Rust binary was already built by `pixi run build-webadmin-arm` and
# staged at serve/bin/bananas-foo. Skip bitbake's compile/configure.
do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${WORKDIR}/bananas-foo ${D}${bindir}/bananas-foo

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/bananas-foo.service ${D}${systemd_system_unitdir}/

    # The extension manifest. bananas-router reads it for API routing;
    # bananas-webadmin reads it to populate its plugin asset map.
    install -d ${D}${sysconfdir}/bananas/extensions.d
    install -m 0644 ${WORKDIR}/foo.toml ${D}${sysconfdir}/bananas/extensions.d/foo.toml
}

# Restart both manifest-reading daemons after install/remove so the
# new tab appears (or disappears) without a reboot. ~50 ms downtime
# per daemon.
pkg_postinst:${PN}() {
    if [ -z "$D" ]; then
        systemctl restart bananas-router.service 2>/dev/null || true
        systemctl restart bananas-webadmin.service 2>/dev/null || true
    fi
}

pkg_postrm:${PN}() {
    if [ -z "$D" ]; then
        systemctl restart bananas-router.service 2>/dev/null || true
        systemctl restart bananas-webadmin.service 2>/dev/null || true
    fi
}

FILES:${PN} += "${bindir}/bananas-foo \
                ${systemd_system_unitdir}/bananas-foo.service \
                ${sysconfdir}/bananas/extensions.d/foo.toml"
```

## 4. Ship the support files

Three files in `layers/meta-bananas/recipes-bsp/bananas-foo/files/`:

### `bananas-foo.service`

```ini
[Unit]
Description=BanaNAS foo plugin daemon
After=local-fs.target bananas-engine.service
Requires=bananas-engine.service

[Service]
Type=simple
User=bananas
Group=bananas
ExecStart=/usr/bin/bananas-foo

# Shared with the rest of the bananas-* daemons. Preserve so the engine
# / webadmin sockets aren't ripped out from under them when this unit
# cycles.
RuntimeDirectory=bananas
RuntimeDirectoryPreserve=yes

# Same StateDirectory as bananas-webadmin so the shared session.key is
# reachable.
StateDirectory=bananas
StateDirectoryMode=0700

Environment=BANANAS_ENGINE_SOCKET=/run/bananas/engine.sock
Environment=BANANAS_FOO_SOCKET=/run/bananas/foo.sock
Environment=BANANAS_SESSION_KEY=/var/lib/bananas/session.key
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
```

### `foo.toml` — the extension manifest

```toml
# bananas-foo — extension manifest read by bananas-router on startup.
# One API prefix per plugin; the asset prefix is implied by id (webadmin
# sub-proxies /assets/foo/* to this daemon's socket without consulting
# the manifest).
id          = "foo"
label       = "Foo"
socket      = "/run/bananas/foo.sock"
api_prefix  = "/api/foo"
order       = 60                # ordering hint for the SPA's tab list (lower = earlier)
icon        = "puzzle"          # icon name from the SPA's icon set
```

`order` values currently used by core/plugins:

| Plugin | order |
|---|---|
| storage | 10 |
| users | 20 |
| exports | 30 |
| stats | 40 |
| cloud | 50 |
| dashboard | 70 |

Pick a value that slots the new tab where you want it.

### Optional: a default per-service config TOML

If your daemon reads `/etc/bananas/foo.toml` for its own config, ship a sane default in the same `files/` dir and `install` it the same way the cloud recipe ships `cloud.toml`. The engine is the one that rewrites these files at runtime when the operator changes settings via the UI.

## 5. Wire it into the bake (optional)

If your plugin should ship as a default-installed essential (rather than opkg-installable on demand), add it to `IMAGE_INSTALL` in `layers/meta-bananas/recipes-core/images/bananas-image.bb`. Otherwise skip — the recipe will land in `build/tmp/deploy/ipk/<arch>/` and the release pipeline will publish it to the gh-pages opkg feed where operators can `opkg install bananas-foo` on demand.

The whitelist in `.github/workflows/release.yml` (the `Stage feed for gh-pages` step) controls which IPKs get published. New plugins need to be added to the `PKGS=` line:

```bash
PKGS="bananas-webadmin bananas-stats bananas-dashboard bananas-config bananas-webadmin-ui bananas-feed-config bananas-modprobe bananas-rclone bananas-foo"
```

## 6. Build it

Cross-compile the Rust crate to armv7 and stage the binary:

```bash
pixi run build-webadmin-arm   # also builds the other daemons; quickest path
```

Now bake the recipe:

```bash
pixi shell
kas shell kas.yml -c 'bitbake bananas-foo'
```

The `.ipk` lands at `build/tmp/deploy/ipk/cortexa7t2hf-neon/bananas-foo_*.ipk`.

## 7. Install + test on the board

If you're on the dev loop (`pixi run iterate`), the next iteration bakes and serves a rootfs that already has the new IPK in `/var/lib/opkg/`. To install:

```bash
ssh root@bananapro.local 'opkg install /path/to/bananas-foo_X.Y.Z_armv7vehf-neon.ipk'
```

Or to test the full opkg install flow (after the IPK is published to the gh-pages feed):

```bash
ssh root@bananapro.local 'opkg update && opkg install bananas-foo'
```

The postinst restarts router + webadmin; the new tab should appear in the SPA within a couple seconds. Open the web admin in a browser and verify:

- The tab shows up in the nav bar.
- Clicking it loads your SPA without errors.
- `/api/foo/*` requests reach your daemon (check `journalctl -u bananas-foo -e`).
- Engine RPC (if used) works (`journalctl -u bananas-engine -e`).

## 8. Uninstall

```bash
opkg remove bananas-foo
```

Postrm restarts the router + webadmin, the tab disappears, and the manifest is gone from `/etc/bananas/extensions.d/`. The daemon is stopped via the `systemctl disable` that systemd's RPM scriptlet runs on package removal.

Operator data your plugin wrote (e.g. `/etc/bananas/foo.toml`, `/var/lib/bananas/foo/`) survives the uninstall — `opkg purge` is the way to wipe it, and that's the operator's call, not yours.

## Checklist for review

Before opening a PR for a new plugin, confirm:

- [ ] Recipe builds clean: `bitbake bananas-foo` exits 0, `.ipk` lands in `build/tmp/deploy/ipk/`.
- [ ] `pkg_postinst` and `pkg_postrm` both restart router + webadmin.
- [ ] Manifest `id` matches the package name (`bananas-foo` → `id = "foo"`).
- [ ] `RDEPENDS:${PN}` lists every other `bananas-*` package required at runtime.
- [ ] `COMPATIBLE_MACHINE` restricts to boards the plugin actually works on.
- [ ] Service file references shared `RuntimeDirectory=bananas` + `StateDirectory=bananas` (don't take ownership of `/run/bananas/` exclusively — other daemons live there).
- [ ] Daemon never writes `/etc/anything` directly. Privileged writes go through the engine.
- [ ] Daemon validates session cookies via `BANANAS_SESSION_KEY` for any state-changing endpoint.
- [ ] Added to `release.yml`'s `PKGS=` whitelist if it should publish to the opkg feed.
- [ ] `cargo fmt` clean (pre-commit will block you otherwise).
- [ ] Commit message follows conventional commits (see [Conventions](./conventions.md)).
