# Hardware

BanaNAS targets a single board: the **Banana Pro (BPI-M1+)** with the Allwinner A20 SoC. Building for the original BPI-M1 will not work — the M1+ has different DTB pin assignments for the on-board SDIO WiFi and the LCD/RGB pin mux.

## What you need

- **Banana Pro (BPI-M1+)** — the only supported SBC.
- **microSD card**, ≥ 4 GB, Class 10 or better. The bake produces a `.wic` image you `dd` straight onto the card.
- **Wired Ethernet** — the image disables `brcmfmac` so the on-board WiFi does not bring up; this is intentional (no 13 s of boot-time spam, headless NAS doesn't need WiFi).
- **A power supply** — 5 V / 2 A USB-C is plenty. The board idles around 5 W.

## Optional but recommended

- **5″ RGB888 DPI panel** — the LCD dashboard runs on the BL050-RGB-002 (800×480). Without a panel attached you get a headless image; the dashboard service simply does not start.
- **SATA disk** — one or more drives connected to the on-board SATA port. USB drives also work.
- **A spare SSH public key** — needed for `pixi run iterate` (development/dev-loop use). Drop it into `layers/meta-bananas/recipes-core/images/files/authorized_keys` before building.

## Storage caveat (32-bit ARM)

The A20 is 32-bit, so `pgoff_t = 32 bits = 16 TiB at 4 KiB pages`. This is a **hard kernel cap** that affects every ext4 filesystem on the board:

- An ext4 partition larger than 16 TiB **cannot be mounted** (`EXT4-fs: filesystem too large to mount safely on this system`).
- Buffered I/O on the raw block device silently truncates past 16 TiB. Reads return zero bytes; writes are dropped without error.
- If you have a > 16 TiB drive, partition it on an x86_64 host into ≤ 16 TiB partitions before plugging it into the board.

Full details, including the keep-the-data shrink path for a drive that already has a > 16 TiB partition, live in [Troubleshooting](./troubleshooting.md).
