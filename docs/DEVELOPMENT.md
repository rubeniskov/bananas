# Development guide

The README's [Quick start](../README.md#-quick-start) covers the path most operators take: grab the prebuilt SD image from a release tag, flash it, configure via the web UI. This document is for the *other* path — building the image from source and iterating against a Banana Pro on your desk without re-flashing the SD every time.

## Host requirements

`pixi` provides Python 3.11, `kas`, and the conda-side host tools. Bitbake additionally needs a couple of system packages that conda does not ship cleanly:

| OS | Command |
|----|---------|
| Ubuntu / Debian | `sudo apt-get install chrpath cpio diffstat hostname rpcsvc-proto` |
| Arch | `sudo pacman -S chrpath cpio diffstat inetutils rpcsvc-proto` |
| macOS | Yocto builds are not supported on macOS host — use a Linux VM / Docker. |
| Windows | Use WSL2 (Ubuntu) — `sudo apt-get install chrpath cpio diffstat hostname rpcsvc-proto`. |

You also need a Docker context pointed at the host's native daemon. Docker Desktop's VM context can't bind LAN UDP for TFTP / NFS, so the iterate loop's containers won't be reachable from the BPI:

```bash
docker context use default
```

## Clone + drop your SSH key

```bash
git clone https://github.com/rubeniskov/bananas.git && cd bananas
cp ~/.ssh/id_ed25519.pub layers/meta-bananas/recipes-core/images/files/authorized_keys
```

The image bakes `authorized_keys` into `${ROOT_HOME}/.ssh/` so `pixi run reboot` works over SSH key auth right after first boot. Without your pubkey in there, iterate's reboot step can't ssh in to soft-reboot the board.

## First build

```bash
pixi run info       # sanity check (pixi + kas versions, etc.)
pixi run build      # full Yocto bake (≈30 min cold cache)
```

The deliverables land at:

- `build/tmp/deploy/images/bananapro/bananas-image-bananapro.rootfs.tar.gz` — rootfs only, used by the iterate loop's NFS netboot path.
- `build/tmp/deploy/images/bananapro/bananas-image-bananapro.rootfs.wic` — full SD-card image (U-Boot SPL + boot partition + rootfs), `dd`-able.

The build is reproducible if you keep `Cargo.lock` and the layer pins in `kas.yml` unchanged.

## Network-boot setup (one-time)

The TFTP + NFS containers run via `compose.yml` against the host's native Docker daemon. On a new dev machine, disable the system NFS server first — the kernel `nfsd` module is shared and only one server may bind it:

```bash
sudo systemctl disable --now nfs-server
lsmod | grep -q '^nfsd' || sudo modprobe nfsd
```

Then teach the BPI's U-Boot to netboot. Paste these once over the serial console and `saveenv`:

```
setenv serverip <host-ip>
setenv ipaddr <bpi-static-ip>
setenv netargs 'setenv bootargs console=ttyS0,115200 root=/dev/nfs nfsroot=<host-ip>:/nfsshare,vers=3 ip=dhcp panic=10'
setenv bootcmd_net 'run netargs; tftpboot 0x42000000 uImage; tftpboot 0x43000000 sun7i-a20-bananapro.dtb; bootm 0x42000000 - 0x43000000'
setenv bootcmd 'run bootcmd_net'
saveenv
```

## The iterate loop

```bash
BPI_HOST=<bpi-host-or-ip> pixi run iterate
```

Picks up source changes, rebuilds the affected pieces (UI / server / helper / stats / dashboard), bakes the Yocto rootfs, soft-reboots the BPI over SSH, then streams the new rootfs back over NFS so the next boot picks it up. Subsequent iterations on a UI-only change typically take under a minute. Cold-cache iterations are dominated by the Yocto bake (~30 min).

`.env` (gitignored) is auto-sourced for `BPI_HOST` and the optional `ROOT_PASSWORD_HASH`. See [`.env.example`](../.env.example) for the layout.

## Iterate-loop ordering

The explicit sequence in `pixi run iterate` is `build-webadmin → build-server-arm → build-stats-arm → build-dashboard-arm → setup-rclone-arm → build (Yocto) → serve-tftp → reboot → wait-board-down → extract-rootfs → serve`.

`wait-board-down` pings `$BPI_HOST` once a second until it stops answering (or 30 s timeout) — this guarantees `extract-rootfs` runs against an idle NFS share so the running BPI's libraries don't get swapped under it mid-shutdown. Earlier the order was `build → (serve-tftp + extract-rootfs) → reboot`, which deterministically wedged the board into "Running in chroot, ignoring request" mode every iteration; if you ever see that error, you're running an old `pixi.toml` and need to update.

## Single-recipe iteration

When iterating on a single recipe rather than the full image, drop into the bitbake environment manually instead of re-running the whole `kas build`:

```bash
pixi shell
kas shell kas.yml -c 'bitbake <recipe>'              # e.g. linux-mainline, u-boot-sunxi, bananas-image
kas shell kas.yml -c 'bitbake -c <task> <recipe>'    # single task: cleansstate, compile, devshell, ...
```

`kas` materialises the upstream layers (`poky/`, `meta-openembedded/`, `meta-sunxi/`, `meta-arm/`) into the repo root and writes everything else under `build/` (`build/conf/`, `build/tmp/`, `build/downloads/`, `build/sstate-cache/`). Both the cloned layers and `build/` are gitignored — never commit them.

## Crate workspace

The repo's a Cargo workspace at edition 2024. Five crates:

| Crate | Target | Purpose |
|-------|--------|---------|
| `crates/engine` | armv7 host bin | Privileged ops over `/run/bananas/engine.sock`. |
| `crates/server` | armv7 host bin | HTTP / WebSocket server, port 8080. |
| `crates/webadmin` | wasm32 | Dioxus 0.7 SPA. Built via `dx bundle --release --platform web`. |
| `crates/stats` | armv7 host bin + lib | Sampler daemon. |
| `crates/dashboard` | armv7 host bin | Slint LCD app. |

Cross-compile via `pixi run build-server-arm` etc. — uses `cargo-zigbuild` for the armv7 targets and runs only the host-side packages (`crates/webadmin` is wasm-only and excluded via `-p` flags).

The `bananas-server.bb` recipe `bbfatal`s if `serve/webadmin/index.html` is missing, so forgetting `pixi run build-webadmin` fails loudly instead of shipping a 404-only image.

## Where to look when bitbake explodes

- A *-native* recipe failing with `-Werror` or a const-qualifier error from the host compiler → add a bbappend in `layers/meta-bananas/recipes-devtools/`. Pattern lives in `elfutils_0.191.bbappend` / `pseudo_git.bbappend` / `dtc_1.7.0.bbappend`.
- Yocto release line is `scarthgap`. If you bump it, update every `refspec` in `kas.yml` together and re-test the bbappends — version-pinned appends like `elfutils_0.191.bbappend` and `dtc_1.7.0.bbappend` will silently stop applying when the upstream recipe version changes.
- `pixi.lock` is marked `merge=binary linguist-generated=true -diff` in `.gitattributes`; regenerate it via `pixi` rather than hand-editing or attempting a 3-way merge.
