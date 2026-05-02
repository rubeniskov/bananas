# bananas-webadmin

The unprivileged HTTP daemon that serves both the JSON API at `/api/*` and the wasm SPA that lives at `/`. Runs as the `bananas` user on port 8080.

## Where it runs

- **User**: `bananas`
- **Listening on**: TCP `0.0.0.0:8080`
- **Started by**: systemd unit `bananas-webadmin.service`

## Architecture

One Cargo package, two binaries:

- `bananas-webadmin` (native armv7) — the daemon. Embeds the SPA's bytes at compile time via `include_dir!("$OUT_DIR/ui")`. The custom axum handler in `src/embedded.rs` does Accept-Encoding negotiation (`.br` → `.gz` → raw) and falls back to `index.html` for SPA routes.
- `bananas-webadmin-ui` (wasm32) — the Dioxus 0.7 SPA, gated behind `required-features = ["wasm-ui"]` so workspace-native `cargo check` skips it. The daemon's `build.rs` runs `dx build --bin bananas-webadmin-ui --features wasm-ui --platform web` and copies the dist tree into `$OUT_DIR/ui/`.

## Configuration

- The webadmin itself is configuration-free. All persistent state (exports, fstab, users, cloud accounts) is owned by the **engine**; the webadmin holds no DB.

## Talking to the engine

Every privileged operation is forwarded to `bananas-engine` over `/run/bananas/engine.sock` (newline-delimited JSON). If the engine is down, the webadmin returns 502 from the affected API endpoints — see [Troubleshooting](../troubleshooting.md).

## Logs

```bash
journalctl -u bananas-webadmin.service -e
```

## Source

`crates/webadmin/` in the [main repo](https://github.com/rubeniskov/bananas/tree/main/crates/webadmin).
