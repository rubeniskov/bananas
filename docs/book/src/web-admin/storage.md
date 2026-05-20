# Storage (mount points)

`/etc/fstab` rules. Each row maps a device or label to a mount point.

The image ships with an **empty `/etc/fstab`** — there are no hardcoded `LABEL=media` defaults. Operators add entries here for whatever disks they have plugged in.

## Adding a mount

1. Click **+ Add mount**.
2. Fill in:
   - **Device** — a `LABEL=…`, `UUID=…`, or `/dev/sdXY` path.
   - **Mount point** — typically under `/srv/` or `/mnt/`. The directory is created if it doesn't exist.
   - **Filesystem** — usually `ext4`. Other supported types: `ext3`, `vfat`, `exfat`, `xfs`, `btrfs`.
   - **Options** — recommended baseline: `defaults,noatime,nofail,x-systemd.device-timeout=10`. `nofail` avoids dropping to rescue mode if the disk is missing; `device-timeout=10` lets a slow drive enumerate without hanging boot indefinitely.
3. **Save** — the helper writes the fstab line, creates the mount point, and runs `systemctl daemon-reload && mount -a`.

## On-device partition surgery

The image ships `e2fsprogs-resize2fs`, `e2fsprogs-tune2fs`, `e2fsprogs-mke2fs`, `e2fsprogs-e2fsck`, and `gptfdisk` so you can repartition ≤ 16 TiB drives without an external host. **Do not** run filesystem maintenance against partitions > 16 TiB on the board itself — see [Troubleshooting](../troubleshooting.md).

## Permissions modal

Each row has a **Permissions** button (small chmod icon). It opens a modal where you can change ownership, mode bits, and optionally apply recursively. Allowlisted to `/srv`, `/mnt`, `/media`, `/home`, `/opt` — paths outside that set are rejected by the helper.
