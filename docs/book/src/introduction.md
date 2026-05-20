# BanaNAS

A Yocto-based, batteries-included NAS distribution for the Banana Pro (BPI-M1+, Allwinner A20). Web admin, NFS exports, live stats dashboard on the LCD, cloud sync — all over a 5″ panel and a 5 W SoC.

## Why?

Off-the-shelf NAS boxes either cost a lot, lock you into a vendor cloud, or both. The Banana Pro has just enough horsepower for a quiet, always-on home file server (gigabit Ethernet, native SATA, USB hosts, a 5″ touch-friendly LCD panel) — but turning a bare board into a *usable* NAS means stitching together a bootloader, kernel, init system, NFS daemon, an admin UI, sync engine, splash screens, status displays, and the world's worst per-device cross-compile dance.

**BanaNAS bundles all of that into one Yocto image.** Build it from source with a single `pixi run iterate`, drop it on an SD card, plug in a SATA disk, and you get:

- A wired-only NFS server with a friendly web admin.
- A live stats dashboard on the on-board LCD.
- Backup-to-Google-Drive (and 5 other providers) without manual `rclone.conf` edits.
- A boot story (U-Boot logo → progress bar → live tiles) that doesn't look like a hobbyist project.
- A reproducible image you can rebuild months later because every dep is pinned in `kas.yml` + `pixi.lock`.

Reproducibility, low idle power, no cloud lock-in, and "your own data, your own metal" — that's the goal.

## Where to go next

- **[Hardware](./hardware.md)** — what board + peripherals are supported.
- **[Install the SD card](./install.md)** — flash and boot in 5 minutes.
- **[Web admin tour](./web-admin/index.md)** — what each tab in the UI does.
- **[Components](./components/webadmin.md)** — service-by-service reference.
