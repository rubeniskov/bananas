# bananas-router

The frontend reverse proxy. Sits in front of the webadmin and any micro-frontends (currently just `bananas-cloud`), routing requests by URL prefix.

## Where it runs

- **User**: `bananas`
- **Listening on**: TCP `0.0.0.0:80` (or whatever port the deployment uses; default is the standard HTTP port)
- **Started by**: systemd unit `bananas-router.service`

## Routing rules

- `/api/*`, `/`, `/static/*` → `bananas-webadmin` (loopback)
- `/cloud/*` → `bananas-cloud` (loopback)

## Why a separate process

Two reasons:

1. **Micro-frontend composition.** New service-shaped UIs (cloud, future ones) ship their own SPA + API and get mounted under a path prefix without rebuilding webadmin.
2. **Single port externally.** The board exposes one HTTP port to the LAN; everything else is loopback-only and reached through the router.

## Configuration

Compiled in for now. Routing rules live in `src/main.rs`.

## Logs

```bash
journalctl -u bananas-router.service -e
```

## Source

`crates/router/` in the [main repo](https://github.com/rubeniskov/bananas/tree/main/crates/router).
