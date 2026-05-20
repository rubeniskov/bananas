# Install the SD card

## 1. Grab the SD image

Head to the [latest release](https://github.com/rubeniskov/bananas/releases/latest) and download `bananas-image-armv7.tar.gz`. That tarball wraps a single `.wic` file ready to be `dd`-ed straight onto a card — U-Boot SPL, kernel, dtb, and rootfs all baked in.

## 2. Flash it

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

## 3. Boot the BPI

Insert the card, plug in Ethernet, power on. The first boot:

- U-Boot shows the BanaNAS splash on the LCD if one is attached.
- A psplash progress bar covers the kernel → userspace handoff.
- The rootfs auto-grows to fill the rest of the SD card (one-time, NFS netboots are skipped automatically).
- mDNS publishes the box as `bananapro.local` via avahi.

Find the LAN IP via `ping bananapro.local` or your router's DHCP table.

Continue to **[First boot](./first-boot.md)** for sign-in and initial setup.
