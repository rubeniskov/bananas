# bananas-dashboard

The Slint 1.16 application that draws live tiles on the on-board 5″ LCD: CPU, memory, network sparklines (per interface), disk I/O sparklines (per device), partitions with usage bars, temperatures (CPU on-die + per-disk SMART via the kernel `drivetemp` module).

Renders straight to `/dev/dri/card0` via `linuxkms-noseat` — no X server, no Wayland.

## Where it runs

- **User**: root (for KMS access; could be tightened later with cap_sys_admin)
- **Display**: `/dev/dri/card0` (the on-board panel)
- **Started by**: systemd unit `bananas-dashboard.service`

## How live data reaches it

The dashboard subscribes to **bananas-stats**'s Unix socket at `/run/bananas-stats/live.sock`. No SQLite reads on the live path — the dashboard never opens the stats DB.

## GPU status

Mali-400 / lima userspace acceleration is scaffolded behind a feature flag. Currently the dashboard runs the Slint software renderer, which is fast enough on a 5″ 800×480 panel at 30 fps. Flip the feature flag once a working lima userspace ships in our Yocto sysroot.

## Configuration

None — display layout is compiled in. The data source URL is hardcoded to `/run/bananas-stats/live.sock`.

## Logs

```bash
journalctl -u bananas-dashboard.service -e
```

## Source

`crates/dashboard/` in the [main repo](https://github.com/rubeniskov/bananas/tree/main/crates/dashboard).
