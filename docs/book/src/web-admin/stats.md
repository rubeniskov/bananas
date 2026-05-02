# Stats

Live charts for the box. CPU, memory, per-interface network sparklines, per-device disk I/O, partition usage bars, temperatures (CPU on-die + per-disk SMART via the kernel `drivetemp` module).

The page subscribes to `/api/stats/live` over a WebSocket. *N* connected web clients = **one** subscription to the `bananas-stats` daemon, regardless of *N* — the server multiplexes.

## Settings

The Stats tab also exposes the sampling/retention knobs:

- **Sampling rate** — 1 Hz default.
- **Flush window** — 5 s default.
- **Raw retention** — 24 h default.
- **1-minute aggregate retention** — 30 d default.

Every knob is editable in the UI and writes to `/etc/bananas/stats.toml` via the privileged helper.

## Storage cap

The stats DB lives at `/var/lib/bananas/stats.db` (SQLite, WAL mode). The auto-sized budget is **5 % of free space, capped at 100 MiB**. Hourly retention sweeps prune anything past the configured retention.
