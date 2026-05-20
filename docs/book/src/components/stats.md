# bananas-stats

The sampler + writer that backs both the dashboard's live tiles and the web admin's WebSocket bus.

Architecture is a sampler over a WAL'd SQLite at `/var/lib/bananas/stats.db` plus a live Unix-socket publisher at `/run/bananas-stats/live.sock`. Both downstream consumers (dashboard + webadmin's WebSocket bus) subscribe to the live socket — live data never round-trips through SQLite.

## Where it runs

- **User**: `bananas`
- **Listening on**: Unix socket `/run/bananas-stats/live.sock` (pub/sub)
- **DB**: `/var/lib/bananas/stats.db` (SQLite WAL)
- **Started by**: systemd unit `bananas-stats.service`

## Defaults

- **Sampling rate**: 1 Hz
- **Flush window**: 5 s
- **Raw retention**: 24 h
- **1-minute aggregate retention**: 30 d
- **DB budget**: auto-sized to 5 % of free space on `/var/lib`, capped at 100 MiB. Hourly retention sweeps prune anything past retention.

All four are editable from the web admin's [Stats tab](../web-admin/stats.md) and persist to `/etc/bananas/stats.toml`.

## Configuration

- `/etc/bananas/stats.toml` — sampling/retention knobs (managed by the web admin)

## Logs

```bash
journalctl -u bananas-stats.service -e
```

## Source

`crates/stats/` in the [main repo](https://github.com/rubeniskov/bananas/tree/main/crates/stats).
