# Troubleshooting

## "Filesystem too large to mount safely" (16 TiB ext4 wall)

> `EXT4-fs: filesystem too large to mount safely on this system`

The Allwinner A20 is 32-bit, so `pgoff_t = unsigned long = 32 bits = 16 TiB at 4 KiB pages`. **Hard kernel cap**, not a tunable.

### Consequences

- Any ext4 partition larger than 16 TiB will not mount on the board.
- Buffered I/O on the raw block device silently truncates past 16 TiB. Reads return zero bytes; writes are dropped without error.
- Running `e2fsck` on a > 16 TiB filesystem from the board has destroyed primary superblocks in the past — metadata writes past the cap were lost.

### Fix

If you have a > 16 TiB drive, do **all** filesystem maintenance off-board on an x86_64 host. To shrink an existing > 16 TiB ext4 partition while keeping the data:

```bash
e2fsck -fy /dev/sda1
resize2fs -p /dev/sda1 4294967295   # 16 TiB minus one 4 KiB block — exactly under pgoff_t_max
```

Then use `sgdisk` to shrink partition 1 and create partition 2 in the freed tail.

## NFS netboot dev loop is wedged on "Running in chroot, ignoring request"

You're running an old `pixi.toml`. The fix shipped in the iterate-loop reorder: the new ordering is `build → serve-tftp → reboot → wait-board-down → extract-rootfs → serve`. The earlier order ran `extract-rootfs` while the BPI was still up, swapping libraries under it mid-shutdown. Pull a fresh `main` and re-run `pixi run iterate`.

## LCD panel not lighting up after boot

The display pipeline is `display-frontend → display-backend → tcon0 → panel-simple → pwm-backlight`. **Every** node must probe successfully or `sun4i-drm` (the component master) never registers and `/sys/class/drm/` stays empty even though the sub-drivers all bind.

The single biggest gotcha: `&de { status = "okay"; };` must be set in the device tree. Without it, all sub-drivers bind individually but `/sys/class/drm/card0` never appears.

If you've inherited a DT overlay from the legacy SD-card flow that uses `connector-type = "lvds"` — that's wrong for the BPI-M1+ panel. Use `panel-dpi` (parallel RGB888 through DPI), not LVDS.

## "missing clock-frequency" warning on boot

The BanaNAS image carries a kernel patch (`0001-bananapro-cpu-clock.patch`) that adds `clock-frequency = <912000000>` on `cpu@0` / `cpu@1`. If you see this warning, you've booted a kernel without the patch. Rebuild the image — `bitbake -c cleansstate linux-mainline && bitbake bananas-image`.

## Web admin returns 502 / connection refused

Check both daemons:

```bash
systemctl status bananas-webadmin   # unprivileged HTTP daemon, port 8080
systemctl status bananas-engine     # root helper, /run/bananas/engine.sock
```

The webadmin reaches the engine over a Unix socket. If `bananas-engine` is down, every privileged action (write fstab, write exports, manage users, cloud sync) returns 502 from the API. Logs: `journalctl -u bananas-engine -e`.

## opkg upgrade pulls nothing

The opkg feed at `https://rubeniskov.github.io/bananas/feed/` only ships the `bananas-*` runtime packages — base-OS packages live in the SD-card image bake and aren't republished. So `opkg upgrade` only ever updates BanaNAS components. To upgrade the kernel or base userspace, re-flash the SD card from the latest release.
