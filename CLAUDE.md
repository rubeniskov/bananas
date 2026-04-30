# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

BanaNAS is a Yocto-based image for the Banana Pro (BPI-M1+, Allwinner A20) targeting a headless NFS NAS profile. The build pipeline is `pixi` → `kas` → `bitbake`; the deliverable is an SD-card image produced by the `bananas-image` recipe.

## Commands

```bash
pixi run info       # sanity check (python --version, kas --version)
pixi run build      # full image build via kas (kas build kas.yml)
pixi run setup-fel  # one-shot: clones linux-sunxi/sunxi-tools into vendor/ and builds it against the pixi-provided libusb/libfdt/zlib
pixi run fel ...    # invokes vendor/sunxi-tools/sunxi-fel; auto-runs setup-fel on first use. Used for FEL-mode flashing of the BananaPro over USB-OTG (e.g. `pixi run fel ver`, `pixi run fel uboot build/.../u-boot-sunxi-with-spl.bin`)
pixi run serve      # stages kernel/dtb into ./serve/tftp/, extracts rootfs into /srv/nfs/bananas/, and starts the TFTP docker container — see Iteration loop below
pixi run reboot     # ssh root@$BPI_HOST reboot — BPI_HOST is required, e.g. `BPI_HOST=<bpi-host-or-ip> pixi run reboot`
pixi run iterate    # build + serve + reboot, the standard dev loop
```

## Iteration loop (TFTP + NFS netboot, all in Docker)

For fast development without re-flashing the SD, the board boots its kernel/DTB over TFTP and mounts its rootfs over NFS. Both servers run in containers managed by `compose.yml` against the **native** Docker daemon (`unix:///var/run/docker.sock`); Docker Desktop's VM context cannot serve UDP/69 or the NFS port set to the LAN. Run `docker context use default` once on a fresh machine — every iterate task just calls `docker` with no context override and relies on that being the active context.

The rootfs is extracted via a throwaway alpine container so the files inside `./serve/nfs/bananas/` retain their original (root-owned) ownership without needing host-side `sudo`.

**One-time host setup:**

```bash
# Disable the native NFS server (Docker NFS will use the kernel nfsd module — only one is allowed)
sudo systemctl disable --now nfs-server

# Ensure nfsd is loadable (it's a kernel module shared between native and container NFS)
lsmod | grep -q '^nfsd' || sudo modprobe nfsd
```

That's it for the host. No `/etc/exports` changes — the container manages exports internally.

**One-time U-Boot env** (paste into the U-Boot serial console once, then `saveenv`):

```
setenv serverip <host-server-ip>
setenv ipaddr <bpi-static-ip>
setenv netargs 'setenv bootargs console=ttyS0,115200 root=/dev/nfs nfsroot=<host-server-ip>:/nfsshare,vers=3 ip=dhcp panic=10'
setenv bootcmd_net 'run netargs; tftpboot 0x42000000 uImage; tftpboot 0x43000000 sun7i-a20-bananapro.dtb; bootm 0x42000000 - 0x43000000'
setenv bootcmd 'run bootcmd_net'
saveenv
```

The NFS export path `/nfsshare` is the path *inside* the container — not on the host. The host bind-mounts `./serve/nfs/bananas` → `/nfsshare`.

**Per-iteration:** `pixi run iterate` rebuilds, republishes (kernel/DTB → tftp share, rootfs → NFS share via container-side extract), and reboots the board — no buttons, no SD writes.

**`pixi run reboot` requires SSH key access** to the booted board. `bananas-image.bb` runs `install_root_authkey` in `ROOTFS_POSTPROCESS_COMMAND` to bake `layers/meta-bananas/recipes-core/images/files/authorized_keys` into `${ROOT_HOME}/.ssh/authorized_keys` (note: in OE the root home is `/home/root`, not `/root` — the install function reads the actual passwd entry to avoid drift). Drop your public key into that file before building. Root password stays unset; OpenSSH's default `PermitRootLogin prohibit-password` allows key auth and blocks password auth.

**Iterate-loop ordering**: the explicit sequence in `pixi run iterate` is `build-webadmin → build-server-arm → build (Yocto) → serve-tftp → reboot → wait-board-down → extract-rootfs → serve`. The `wait-board-down` task pings `$BPI_HOST` once a second until it stops answering (or 30 s timeout) — this guarantees `extract-rootfs` runs against an idle NFS share so the running BPI's libraries don't get swapped under it mid-shutdown. Earlier the order was `build → (serve-tftp + extract-rootfs) → reboot`, which deterministically wedged the board into "Running in chroot, ignoring request" mode every iteration; if you ever see that error, you're running an old `pixi.toml` and need to update.

## Web admin (crates/server + crates/webadmin)

The repo's a Cargo workspace at edition 2024. Three crates:

- **`crates/engine`** — root-privileged daemon (`bananas-engine`). Listens on `/run/bananas/engine.sock` (newline-delimited JSON). Performs every privileged op: rewrite `/etc/exports`, write `/etc/fstab`, manage users/groups, write per-service TOMLs, run `smartctl`/`lsblk`, opkg upgrades, system reboot.
- **`crates/server`** — unprivileged HTTP daemon (`bananas` user, port 8080). Pure JSON API at `/api/*` plus `tower-http::ServeDir` serving the wasm SPA from `BANANAS_WEBADMIN_DIR` (default `/usr/share/bananas/webadmin`). The fallback is `index.html` so the SPA owns its own routing.
- **`crates/webadmin`** — Dioxus 0.7 Web app (wasm32). Compiled with `dx bundle --release --platform web` via the `pixi run build-webadmin` task. Output lands under `target/dx/bananas-webadmin/release/web/public/` and is staged into `serve/webadmin/` for the Yocto recipe to install at `/usr/share/bananas/webadmin/`.

Cross-compile pipeline (`pixi run build-server-arm`) uses `cargo-zigbuild` for the armv7 binaries and runs **only** the `bananas-server` + `bananas-engine` packages (`-p`-pinned, since `bananas-webadmin` is wasm-only). The `pixi run build-webadmin` task is a separate step that the `iterate` loop chains in front. The `bananas-server.bb` recipe `bbfatal`s if `serve/webadmin/index.html` is missing, so forgetting `build-webadmin` fails loudly instead of shipping a 404-only image.

The `pixi` environment provides Python 3.11, `kas`, and the conda-side host tools (chrpath, cpio, patch, gcc, etc.). Bitbake additionally needs system-level packages that conda does not ship cleanly — install them via the host package manager as listed in `README.md` (`chrpath cpio diffstat hostname/inetutils rpcsvc-proto`).

When iterating on a single recipe, enter the bitbake environment manually rather than re-running the whole `kas build`:

```bash
pixi shell
kas shell kas.yml -c 'bitbake <recipe>'              # e.g. linux-sunxi, u-boot-sunxi, bananas-image
kas shell kas.yml -c 'bitbake -c <task> <recipe>'    # single task: cleansstate, compile, devshell, ...
```

`kas` materialises the upstream layers (`poky/`, `meta-openembedded/`, `meta-sunxi/`, `meta-arm/`) into the repo root and writes everything else under `build/` (`build/conf/`, `build/tmp/`, `build/downloads/`, `build/sstate-cache/`). Both the cloned layers and `build/` are gitignored — never commit them.

## Architecture

### Build composition (kas.yml)

`kas.yml` is the single source of truth for layer composition. It pins:

- **machine**: `bananapro` (defined in this repo at `layers/meta-bananas/conf/machine/bananapro.conf`). meta-sunxi only ships a `bananapi` machine which targets the original M1 with a different DTB (`sun7i-a20-bananapi.dtb`); our config requires the BPI-M1+ DTB (`sun7i-a20-bananapro.dtb`) and U-Boot config (`Bananapro_defconfig`) to drive the onboard SDIO WiFi, the LCD, and other Pro-specific peripherals.
- **distro**: `bananas` (defined in this repo at `layers/meta-bananas/conf/distro/bananas.conf`)
- **target**: `bananas-image`
- Upstream layers locked to the `scarthgap` Yocto release.
- `local_conf_header` block — anything appended here is injected into `build/conf/local.conf`. This is where `INHERIT += "rm_work"`, `PACKAGE_CLASSES = "package_ipk"`, `BB_NUMBER_THREADS`, systemd selection, and `DISTRO_FEATURES:remove = " x11 wayland vulkan"` live.

If a build setting needs to be global, edit `kas.yml`'s `local_conf_header` rather than touching `build/conf/local.conf` directly — the latter is regenerated by kas.

### meta-bananas layer

The only layer authored in this repo is `layers/meta-bananas/`. Its priority is `8` and it depends on `core openembedded-layer meta-python networking-layer filesystems-layer`.

Layout:

- `conf/distro/bananas.conf` — distro definition; inherits `poky.conf` and selects systemd + NFS/SSH/IPv6/zeroconf/opengl features.
- `recipes-core/images/bananas-image.bb` — the image recipe. `IMAGE_INSTALL` is the canonical list of what ships on the SD card (NFS/rpcbind, avahi, openssh, mdadm/hdparm/smartmontools, etc.). Samba is intentionally commented out.
- `recipes-devtools/` (elfutils, pseudo) and `recipes-kernel/dtc/` — host-toolchain `.bbappend` files that patch upstream native recipes so the build survives modern host toolchains (GCC 14+, glibc with `openat2`). Examples: `elfutils_0.191.bbappend`, `pseudo_git.bbappend`, `dtc_1.7.0.bbappend`. These all use `do_compile:prepend:class-native()` to rewrite sources before the host-side compile and target only `class-native` builds — do not promote them to target builds without justification.
- `recipes-kernel/linux/linux-mainline_%.bbappend` — appends a `SRC_URI:append:bananapro` listing two patches and a kernel config fragment (under `files/`) for our hardware:
  - `0001-bananapro-cpu-clock.patch` adds `clock-frequency = <912000000>` on cpu@0 / cpu@1 (silences `missing clock-frequency property` warning, populates `cpu_capacity`).
  - `0002-bananapro-lcd-panel.patch` enables the 5" 800×480 BL050-RGB-002 panel: sets `&de` (display-engine) `status = "okay"` (CRITICAL — sun7i-a20.dtsi ships it disabled, without this DRM master never instantiates), wires `lcd0_rgb888_pins` (PD0..PD27) into tcon0, creates `lcd_panel` (panel-dpi with explicit panel-timing + `bus-format = <0x100a>` for MEDIA_BUS_FMT_RGB888_1X24) and `lcd_backlight` (pwm-backlight on PB2/PWM0 + PH8 enable; panel power on PH12).
  - `drm-sun4i.cfg` turns on `CONFIG_DRM`, `CONFIG_DRM_SUN4I*`, `CONFIG_DRM_PANEL_SIMPLE`, `CONFIG_DRM_PANEL_DPI`, `CONFIG_BACKLIGHT_PWM`, `CONFIG_PWM_SUN4I` (the upstream sunxi defconfig is headless — none of these are on by default).

When a Yocto build fails inside a *-native* recipe with a `-Werror` or const-qualifier error from the host compiler, the fix pattern is a new bbappend in `recipes-devtools/` mirroring the existing ones, not a change to upstream.

### LCD pipeline notes (Banana Pro 5" RGB)

- The display pipeline is `display-frontend → display-backend → tcon0 → panel-simple → pwm-backlight`. **Every** node must probe successfully or `sun4i-drm` (the component master) never registers and `/sys/class/drm/` stays empty even though the sub-drivers all bind. The single biggest gotcha is `&de { status = "okay"; };` — without it, all sub-drivers bind individually but `/sys/class/drm/card0` never appears.
- The panel is reached via the `panel-dpi` compatible (parallel RGB888 through DPI), not `lvds`. Using `connector-type = "lvds"` (as in the wens/dt-overlays patch 0004 referenced from the legacy SD-card flow) is wrong for this hardware and silently leaves the panel off.
- `panel-simple` will warn `Specify missing bus_format` and `Expected bpc in {6,8} but got: 0` if `bus-format` isn't set on the panel node (panel-dpi doesn't read `data-mapping` for that). Specifying `bus-format = <0x100a>` (numeric, MEDIA_BUS_FMT_RGB888_1X24) silences both — it implies bpc=8.
- Panel timings live in `docs/LCD_5INCH_CONFIGURATION.md` (30 MHz pclk, 800x480, 88/40 hbp/hfp + 48 hsync, 32/13 vbp/vfp + 3 vsync); the LCD is now baked directly into `sun7i-a20-bananapro.dtb` rather than loaded as a runtime overlay (U-Boot 2024.01 in this image lacks `CONFIG_CMD_NFS`/`CONFIG_CMD_WGET` so the overlay-loading path isn't worth setting up).
- The doc also references `scripts/fex_to_uboot.py` and `overlays/bpi-m1p-lcd.dtbo` — treat those as planned/expected artifacts rather than guaranteed to exist; the inlined-DTS approach makes them unnecessary.

### SATA storage

The image now ships an **empty `/etc/fstab`** and an **empty `/etc/exports`** — operators add disk mounts and NFS shares through the web admin's *Mount points* and *Exports* tabs, which route the edit through `bananas-engine`. No more hardcoded `LABEL=media` / `LABEL=services` defaults; nothing in `bananas-image.bb` post-processes those files at bake time. `nfs-server.service` is still pre-enabled with the `NFSD_COUNT=8` drop-in so adding the first export from the UI just works without a manual `systemctl enable`.

Recommended mount options to use when adding entries via the UI: `defaults,noatime,nofail,x-systemd.device-timeout=10`. `nofail` avoids dropping to rescue mode if the disk is missing; `device-timeout=10` lets a slow drive enumerate without hanging boot indefinitely.

**32-bit ARM ext4 wall — DO NOT FORGET.** sun7i-a20 is 32-bit, so `pgoff_t = unsigned long = 32 bits = 16 TiB at 4 KiB pages`. Consequences:

- An ext4 partition larger than 16 TiB **cannot be mounted** on the BPI (`EXT4-fs: filesystem too large to mount safely on this system`). The 24 TB drive originally had a single 21.8 TiB partition; we shrunk it to two ≤16 TiB partitions on an x86_64 host.
- **Buffered I/O on the raw block device silently truncates past 16 TiB.** Reads return zero bytes with no error; writes are dropped. Running `e2fsck` on a >16 TiB filesystem from the BPI **destroyed the primary superblock** because metadata writes past the cap were lost. If a future drive is >16 TiB, do **all** filesystem maintenance off-board.
- O_DIRECT bypasses the page-cache cap (`dd iflag=direct` works at any offset), but `e2fsprogs` doesn't enable it by default. Don't rely on this.
- The keep-the-data shrink path is: `e2fsck -fy && resize2fs -p /dev/sda1 4294967295` (16 TiB minus one 4 KiB block — exactly under `pgoff_t_max`), then `sgdisk` to shrink partition 1 + create partition 2 in the freed tail.

`IMAGE_INSTALL` ships `e2fsprogs-resize2fs`, `e2fsprogs-tune2fs`, `e2fsprogs-mke2fs`, `e2fsprogs-e2fsck`, and `gptfdisk` so partition surgery on ≤16 TiB partitions can happen on the BPI itself.

## Conventions

- Yocto release line is `scarthgap`. If you bump it, update every `refspec` in `kas.yml` together and re-test the bbappends — version-pinned appends like `elfutils_0.191.bbappend` and `dtc_1.7.0.bbappend` will silently stop applying when the upstream recipe version changes.
- `pixi.lock` is marked `merge=binary linguist-generated=true -diff` in `.gitattributes`; regenerate it via `pixi` rather than hand-editing or attempting a 3-way merge.
