# bananas-engine

The root-privileged helper daemon. Performs every operation that requires root: rewrite `/etc/exports`, write `/etc/fstab`, manage users/groups, write per-service TOMLs under `/etc/bananas/`, run `smartctl`/`lsblk`, opkg upgrades, system reboot.

The webadmin (and any other unprivileged caller) reaches it over a Unix socket — there is no HTTP listener.

## Where it runs

- **User**: `root`
- **Listening on**: Unix socket `/run/bananas/engine.sock`
- **Started by**: systemd unit `bananas-engine.service`

## Wire protocol

Newline-delimited JSON on a Unix socket. Request shape:

```json
{"op": "<name>", "args": { … }}
```

Response: a single JSON object (`{"ok": …}` or `{"err": …}`) terminated by `\n`.

## Configuration

The engine itself takes no configuration file. The state it manages lives in the canonical Linux locations:

- `/etc/exports` — NFS exports
- `/etc/fstab` — mount points
- `/etc/passwd`, `/etc/shadow`, `/etc/group` — users
- `/etc/bananas/*.toml` — per-service config (stats, cloud, etc.)

## Logs

```bash
journalctl -u bananas-engine.service -e
```

## Source

`crates/engine/` in the [main repo](https://github.com/rubeniskov/bananas/tree/main/crates/engine).
