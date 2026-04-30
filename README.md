<div align="center">
  <img src="assets/splashscreen.png" alt="BanaNAS" width="640" />

  <h1>BanaNAS</h1>

  <p>
    <a href="https://github.com/rubeniskov/bananas/actions/workflows/ci.yml"><img src="https://github.com/rubeniskov/bananas/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"/></a>
    <a href="https://github.com/rubeniskov/bananas/releases/latest"><img src="https://img.shields.io/github/v/release/rubeniskov/bananas?display_name=tag&sort=semver" alt="Latest release"/></a>
    <a href="LICENSE"><img src="https://img.shields.io/github/license/rubeniskov/bananas" alt="License: MIT"/></a>
  </p>

  <p>
    <strong>A Yocto-based, batteries-included NAS distribution for the Banana Pro (BPI-M1+, Allwinner A20).</strong><br/>
    Web admin · NFS exports · live stats dashboard on the LCD · cloud sync · all over a 5″ panel and a 5 W SoC.
  </p>

  <p>
    <a href="#-features">Features</a> ·
    <a href="#-quick-start">Quick start</a> ·
    <a href="#-architecture">Architecture</a> ·
    <a href="#-screenshots">Screenshots</a> ·
    <a href="docs/hardware-references">Hardware refs</a> ·
    <a href="CONTRIBUTING.md">Contributing</a>
  </p>
</div>

---

## 🍌 Why?

Off-the-shelf NAS boxes either cost a lot, lock you into a vendor cloud, or both. The Banana Pro has just enough horsepower for a quiet, always-on home file server (gigabit Ethernet, native SATA, USB hosts, a 5″ touch-friendly LCD panel) — but turning a bare board into a *usable* NAS means stitching together a bootloader, kernel, init system, NFS daemon, an admin UI, sync engine, splash screens, status displays, and the world's worst per-device cross-compile dance.

**BanaNAS bundles all of that into one Yocto image.** Build it from source with a single `pixi run iterate`, drop it on an SD card, plug in a SATA disk, and you get:

- A wired-only NFS server with a friendly web admin.
- A live stats dashboard on the on-board LCD.
- Backup-to-Google-Drive (and 5 other providers) without manual `rclone.conf` edits.
- A boot story (U-Boot logo → progress bar → live tiles) that doesn't look like a hobbyist project.
- A reproducible image you can rebuild months later because every dep is pinned in `kas.yml` + `pixi.lock`.

Reproducibility, low idle power, no cloud lock-in, and "your own data, your own metal" — that's the goal.

---

## ✨ Features

### NAS core

- **Yocto / scarthgap-based image** — pinned upstream layers, reproducible bake from `pixi run build`.
- **Mainline Linux 6.6** with CPU clock + Banana-Pro LCD-panel patches and a `panel-simple`-driven 800×480 RGB888 DPI panel wired through `tcon0`.
- **NFS v3/v4 server** with `all_squash`, sane `noexec`/`nosuid`/`x-systemd.device-timeout` defaults baked into `/etc/fstab`.
- **SATA + USB storage** with `e2fsprogs-resize2fs`, `tune2fs`, `mke2fs`, `e2fsck`, `gptfdisk` for on-device partition surgery (within the 16 TiB pgoff_t cap on this 32-bit ARM SoC — see the GPT memo in [`docs/`](docs)).
- **systemd** with key-only SSH, baked-in `authorized_keys`, optional root password (SHA-512 hash injected via `ROOT_PASSWORD_HASH`).

### Web admin (`crates/server` + `crates/webadmin`)

- Single-page Dioxus 0.7 wasm UI served alongside a JSON `/api/*` from a Rust HTTP server.
- Hash-based routing — every tab survives a full-page reload.
- Tabs: **Stats**, **Exports**, **Storage** (mount points), **Users**, **Cloud**.
- **Per-row Permissions modal** (chown / chmod / recursive) routed through a privileged helper, allowlisted to `/srv /mnt /media /home /opt`.
- **Save / Load Config** as a single TOML bundle — exports + fstab + users (with shadow hashes) + cloud accounts + sync entries all round-trip.
- **Protected fstab rows** (system mounts) render with a lock icon and refuse edits/deletes server-side; the "save config" path filters them out so backups stay clean.
- Live **WebSocket stats** at `/api/stats/live` — N web clients = ONE socket subscription to the stats daemon, regardless of N.

### LCD dashboard (`crates/dashboard`)

- Slint 1.16 app on the 5″ panel, software renderer (Mali-400 GPU acceleration scaffolding is present but gated behind a feature flag — flip it once a working lima userspace ships in our sysroot).
- Renders straight to `/dev/dri/card0` via `linuxkms-noseat` — no X server, no Wayland.
- Live tiles: **CPU**, **Memory**, **Network** sparklines (per iface), **Disk I/O** sparklines (per device), **Partitions** with usage bars, **Temperatures** chip strip (CPU on-die + per-disk SMART via the kernel `drivetemp` module).
- Subscribes to bananas-stats's Unix socket — no SQLite reads on the live path.

### Persistent stats (`crates/stats`)

- Sampler + writer over a WAL'd SQLite at `/var/lib/bananas/stats.db`.
- 1 Hz sampling, 5 s flush window, 24 h raw retention + 30 d 1-minute aggregates by default — all knobs editable through the web UI's Stats config form.
- Auto-sized DB budget (5 % of free space, capped at 100 MiB) with hourly retention sweeps.
- Live Unix-socket pub/sub at `/run/bananas-stats/live.sock` — both the dashboard and the web admin's WebSocket bus subscribe here, so live data never round-trips through SQLite.

### Cloud sync (`crates/server` + helper, vendored rclone)

- New **Cloud** tab with Accounts and Sync entries panels.
- Provider catalog: Google Drive, Dropbox, OneDrive, S3-compatible, WebDAV, FTP — extensible via a one-line const append.
- Account tokens (rclone-authorize JSON) stored in `/etc/bananas/cloud.toml`, redacted from the API GET response, included in TOML config bundles.
- Run-on-demand syncs invoke `rclone` with **env-var-only config** — the access token never hits disk during a sync (`RCLONE_CONFIG=/dev/null` + `RCLONE_CONFIG_<NAME>_TOKEN=…`).
- Privilege drop from root → `bananas` user before exec so synced files inherit the right ownership under `/srv/*`.

### Boot polish

- **U-Boot splash** on the LCD via `CONFIG_SPLASH_SCREEN` + a 24 bpp 800×480 BMP shipped on the FAT boot partition.
- **psplash** with a banana-yellow progress bar + the BanaNAS logo through the kernel→userspace handoff.
- Quiet kernel cmdline (`quiet loglevel=3 vt.global_cursor_default=0`) so fbcon doesn't draw over the splash.
- WiFi firmware blacklist (`brcmfmac` is unsupported on this image — kills the 13 s of boot-time spam).

### Build pipeline (`pixi.toml`)

- `pixi run iterate` — full loop: build UI (wasm via `dx build`), cross-compile Rust crates for armv7 (cargo-zigbuild for server/helper/stats, cross-rs for the Slint dashboard), bake Yocto image, stage TFTP, soft-reboot the BPI, extract rootfs over NFS, restart compose containers.
- Network-boot iteration over TFTP + NFS via `compose.yml` (no SD-card flashes per change).
- Failure-fast `pixi run build` — bake errors propagate cleanly instead of silently shipping the previous build.

---

## 📸 Screenshots

### Web admin

Web UI tabs across the three most-used surfaces — Exports, Storage, and Users.

| | |
| :-: | :-: |
| **NFS exports** | **Mount points** |
| ![Exports tab](docs/screenshots/web-exports.png) | ![Storage tab](docs/screenshots/web-mounts.png) |
| **Users** | |
| ![Users tab](docs/screenshots/web-users.png) | |

### LCD dashboard

Slint app rendered on the 5″ RGB panel — CPU + memory bars in the header, network and disk sparklines side-by-side, partition usage bars across the bottom.

![LCD dashboard](docs/screenshots/lcd-dashboard.png)

---

## 🚀 Quick start

> **Hardware:** Banana Pro (BPI-M1+), a microSD card (≥ 4 GB), optional 5″ RGB888 LCD, optional SATA disk(s).

### 1. Grab the SD image

Head to the [latest release](https://github.com/rubeniskov/bananas/releases/latest) and download `bananas-image-armv7.tar.gz`. That tarball wraps a single `.wic` file ready to be `dd`-ed straight onto a card — U-Boot SPL, kernel, dtb, and rootfs all baked in.

### 2. Flash it

Find your SD device (replace `/dev/sdX` below — `lsblk` will show it under the right size). One-liner that streams straight from the tarball into `dd`, no intermediate `.wic` file:

```bash
tar -xzOf bananas-image-armv7.tar.gz | sudo dd of=/dev/sdX bs=4M status=progress conv=fsync && sync
```

`-O` makes `tar` extract to stdout; `dd` reads it from stdin. Saves ~575 MB of disk on the host and is the same throughput as the two-step version.

> ⚠️ Double-check the device — `dd` will gladly overwrite your laptop's NVMe if you point it at the wrong path.

If you'd rather verify the inner `.wic` before flashing (or you want to keep a copy on disk):

```bash
tar -xzf bananas-image-armv7.tar.gz
sudo dd if=bananas-image-bananapro.wic of=/dev/sdX bs=4M status=progress conv=fsync
sync
```

### 3. Boot the BPI

Insert the card, plug in Ethernet, power on. The first boot:

- U-Boot shows the BanaNAS splash on the LCD if one is attached.
- A psplash progress bar covers the kernel → userspace handoff.
- The rootfs auto-grows to fill the rest of the SD card (one-time, NFS netboots are skipped automatically).
- mDNS publishes the box as `bananapro.local` via avahi.

Find the LAN IP via `ping bananapro.local` or your router's DHCP table.

### 4. Sign in to the web admin

Browse to **`http://bananapro.local:8080/`** (or the IP). First sign-in is **`root` / `bananas`**, the placeholder credential the image ships with. The login form immediately bounces you into a "Set a new password to continue" screen — the placeholder stops working the moment you rotate it. After rotation:

1. **Mount points** tab → add fstab entries for any SATA / USB disk you have plugged in. The image ships with no defaults; the UI handles `mkdir`, fstab edit, and `systemctl daemon-reload`.
2. **Exports** tab → declare which paths to share over NFS and to what client / CIDR. Same deal — the image ships an empty `/etc/exports`, the UI rewrites it via the privileged helper.
3. **Users** tab → add normal admin users (member of `bananas-admin`); demote root to emergency-use.
4. **Cloud** tab (optional) → connect Google Drive / Dropbox / S3 / etc. for backup syncs.
5. **Save config** → drops a TOML bundle of the entire setup (exports + fstab + users + cloud) onto your laptop. Use **Load config** on a re-flashed card to restore in one click.

That's it for a normal install. Building from source / iterating without re-flashing is covered in [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md).

---

## 🏗 Architecture

```mermaid
flowchart LR
    Clients["NFS clients<br/>(LAN, gigabit)"]
    Browser["Browser / phone<br/>(http://&lt;bpi&gt;:8080)"]
    LCD["5″ RGB888 panel<br/>(/dev/dri/card0)"]
    Cloud["Cloud providers<br/>(Drive, Dropbox, S3, …)"]

    subgraph BPI["Banana Pro (BPI-M1+) — Mainline Linux 6.6 + sun4i-drm + drivetemp + lima · U-Boot 2024.01 + splash.bmp"]
        direction TB
        subgraph Userspace["systemd-managed services"]
            direction TB
            Server["bananas-server<br/>(Rust + axum, port 8080)<br/>/api/* + wasm SPA + WS bus"]
            Helper["bananas-engine<br/>(root, Unix socket RPC)"]
            Stats["bananas-stats<br/>(sampler + SQLite WAL<br/>+ live socket)"]
            Dashboard["bananas-dashboard<br/>(Slint app, software<br/>renderer on KMS)"]
            Rclone["rclone<br/>(on-demand cloud sync)"]
            Nfsd["nfsd · sshd · …"]
        end
    end

    Clients -->|NFSv3/v4| Nfsd
    Browser -->|HTTP + WS| Server
    Server -->|JSON over Unix socket| Helper
    Server -->|subscribe| Stats
    Dashboard -->|subscribe| Stats
    Dashboard -->|frame buffer| LCD
    Helper -->|exec| Rclone
    Rclone -->|HTTPS| Cloud

    classDef ext fill:#fef3c7,stroke:#92400e,color:#1f2937
    classDef svc fill:#dbeafe,stroke:#1e3a8a,color:#1f2937
    classDef root fill:#fee2e2,stroke:#991b1b,color:#1f2937
    class Clients,Browser,LCD,Cloud ext
    class Server,Stats,Dashboard,Rclone,Nfsd svc
    class Helper root
```

### Crate map

| Crate | Target | Purpose |
|-------|--------|---------|
| `crates/engine` | armv7 host bin | Privileged ops (NFS exports rewrite, fstab edit, user mgmt, chown/chmod, smartctl, rclone). Listens on `/run/bananas/engine.sock`. |
| `crates/server` | armv7 host bin | HTTP / WebSocket server, port 8080. JSON `/api/*` + bundled wasm SPA. Routes everything sensitive through the helper. |
| `crates/webadmin` | wasm32 | Dioxus 0.7 SPA. Bundled into `/usr/share/bananas/webadmin/`. |
| `crates/stats` | armv7 host bin + lib | Sampler daemon. Writes SQLite, publishes live snapshots over `/run/bananas-stats/live.sock`. |
| `crates/dashboard` | armv7 host bin | Slint app. Subscribes to the stats live socket; renders on `/dev/fb0`. |

### Layer map

| Layer | Priority | What's there |
|-------|----------|--------------|
| `meta-bananas` (this repo) | 8 | Distro `bananas`, machine `bananapro`, all the BanaNAS-specific recipes (server, helper, stats, dashboard, modprobe blacklist, U-Boot splash, psplash override, mesa-gl x11-strip bbappend, …). |
| `meta-sunxi` | 10 | Allwinner BSP — kernel patches, U-Boot defconfig, machine fragments. |
| `meta-openembedded/{meta-oe,meta-python,meta-networking,meta-filesystems}` | 6 | Recipes for `ttf-dejavu`, runtime libs, fonts, … |
| `meta-arm` | 5 | ARM-specific bits. |
| `poky/{meta,meta-poky,meta-yocto-bsp}` | 5 | Yocto core. |

---

## 📚 References

Picked up over months of stitching this together — useful when something inevitably needs digging.

### Hardware (LeMaker BananaPro datasheets, archived in [`docs/hardware-references/`](docs/hardware-references))

- 7 / 10.1 inch LCD module instructions
- WiFi driver setup
- Audio, camera, CAN bus, GPIO library, GPU, Hardware spec, LCD module, Pin definition, Quick start, UART, WiFi configuration

### Toolchain & SoC

- [Linux-sunxi A20 page](https://linux-sunxi.org/A20)
- [Linux-sunxi mainline kernel howto](https://linux-sunxi.org/Mainline_Kernel_Howto)
- [Linux-sunxi possible setups for hacking on mainline](https://linux-sunxi.org/Possible_setups_for_hacking_on_mainline)
- [Linux-sunxi U-Boot manual build howto](https://linux-sunxi.org/Manual_build_howto#Build_U-Boot)
- [Linux-sunxi Device Tree page](https://linux-sunxi.org/Device_Tree)
- [Linux-sunxi DVFS](https://linux-sunxi.org/A20#DVFS)

### Distros & images for context

- [Arch Linux ARM — Cubietruck (closest A20 reference)](https://archlinuxarm.org/platforms/armv7/allwinner/cubietruck)
- [xnux.eu — install Arch Linux ARM on sunxi](https://xnux.eu/howtos/install-arch-linux-arm.html)

### LCD & DT overlays

- [Armbian — BananaPro LeMaker 5″ LCD legacy kernel thread](https://forum.armbian.com/topic/841-bananapro-lemaker-5in-lcd-legacy-kernel/)
- [Armbian — sun4i-drm and LCD panels thread](https://forum.armbian.com/topic/14560-sun4i-drm-and-lcd-panels/)
- [LeMaker fex_configuration repo](https://github.com/LeMaker/fex_configuration/blob/master/README.md)
- [TI — How to enable DT overlays in Linux](https://software-dl.ti.com/processor-sdk-linux/esd/AM62X/09_01_00_08/exports/docs/linux/How_to_Guides/Target/How_to_enable_DT_overlays_in_linux.html)
- [wens/dt-overlays](https://github.com/wens/dt-overlays/tree/master)

### LCD-specific configuration in this repo

- [`docs/LCD_5INCH_CONFIGURATION.md`](docs/LCD_5INCH_CONFIGURATION.md) — panel timings, DT changes, U-Boot config knobs.

---

## 🧑‍💻 Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the dev-environment setup, the iterate loop, code style, and the conventional-commit format that drives semantic-release.

## 📜 License

[MIT](LICENSE) — © 2026 rubeniskov.
