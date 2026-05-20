<div align="center">
  <img src="assets/splashscreen.png" alt="BanaNAS" width="640" />

  <h1>BanaNAS</h1>

  <p>
    <a href="https://github.com/rubeniskov/bananas/actions/workflows/ci.yml"><img src="https://github.com/rubeniskov/bananas/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"/></a>
    <a href="https://github.com/rubeniskov/bananas/releases/latest"><img src="https://img.shields.io/github/v/release/rubeniskov/bananas?display_name=tag&sort=semver" alt="Latest release"/></a>
    <a href="LICENSE"><img src="https://img.shields.io/github/license/rubeniskov/bananas" alt="License: MIT"/></a>
  </p>

  <p>
    <strong>A Yocto-based, batteries-included NAS distribution for the Banana Pro (BPI-M1+, Allwinner A20) and Raspberry Pi 3/4/5.</strong><br/>
    Web admin · NFS exports · live stats dashboard on the LCD · cloud sync · all over a 5″ panel and a 5 W SoC.
  </p>

  <p>
    <a href="https://rubeniskov.github.io/bananas/">Website</a> ·
    <a href="https://rubeniskov.github.io/bananas/docs/">Documentation</a> ·
    <a href="https://github.com/sponsors/rubeniskov">Sponsor</a>
  </p>
</div>

---

## What it gives you

- **NFS server** with web-managed exports — no `/etc/exports` edits.
- **Live LCD tiles** — CPU, memory, network, disk, temperatures, all on the on-board panel.
- **Cloud sync** to Google Drive, Dropbox, OneDrive, S3, WebDAV via `rclone`.
- **Reproducible bake** from `pixi run build` — every dep pinned in `kas-base.yml` (+ per-machine overlays) and `pixi.lock`.

## Documentation

The full docs live at **<https://rubeniskov.github.io/bananas/docs/>**. Highlights:

- **[Install the SD card](https://rubeniskov.github.io/bananas/docs/install.html)** — flash and boot in 5 minutes.
- **[Web admin tour](https://rubeniskov.github.io/bananas/docs/web-admin/index.html)** — what each tab does.
- **[Components](https://rubeniskov.github.io/bananas/docs/components/webadmin.html)** — service-by-service reference.
- **[Development](https://rubeniskov.github.io/bananas/docs/development/index.html)** — write a custom plugin (IPK) for the BanaNAS extension model.
- **[Troubleshooting](https://rubeniskov.github.io/bananas/docs/troubleshooting.html)** — known caveats (the 16 TiB ext4 wall, NFS netboot loop, etc.).

## Building from source

You'll need [`pixi`](https://pixi.sh/) plus a few host packages (`chrpath cpio diffstat hostname/inetutils rpcsvc-proto`). Then:

```bash
pixi run info     # sanity check
pixi run build    # full Yocto bake — outputs build/tmp/deploy/images/bananas-bpi/bananas-image-bananas-bpi.rootfs.wic
```

For the network-boot dev loop (TFTP + NFS netboot, no SD-card flashes per change), see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Optional plugins (install on demand)

The default image stays lean and ships only the NAS-essentials. The rest of the `bananas-*` plugin set is in the public opkg feed and installs in seconds:

```bash
ssh root@bananas.local
opkg update
opkg install bananas-cloud           # Google Drive / Dropbox / S3 sync (pulls bananas-rclone)
opkg install bananas-dashboard       # Slint LCD app + per-tab web SPA
```

The Cloud / Dashboard / etc. tabs appear in the SPA the moment the install finishes — `bananas-router` and `bananas-webadmin` reload the manifest list via the postinst.

## Contributing

PRs welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE) — © 2026 rubeniskov.
