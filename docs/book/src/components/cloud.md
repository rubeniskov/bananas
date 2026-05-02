# bananas-cloud

The cloud-sync micro-frontend. Same shape as `bananas-webadmin`: one Cargo package with the daemon at `src/main.rs` and the SPA at `src/ui/main.rs` (bin `bananas-cloud-ui`), embedded the same way.

Mounted under `/cloud/*` by **bananas-router**. The host webadmin's MFE loader (`crates/webadmin/src/ui/mfe.rs`) fetches `/cloud/index.html` to discover the cloud SPA's content-hashed JS shim and injects a `<script type="module">` tag at runtime, so the cloud admin renders inline inside the webadmin shell.

## Where it runs

- **User**: `bananas`
- **Listening on**: TCP loopback (port allocated by systemd; the router proxies it)
- **Started by**: systemd unit `bananas-cloud.service`

## What it does

- Manages the **Accounts** and **Sync entries** tables behind the [Cloud tab](../web-admin/cloud.md).
- Calls the engine for token storage (`/etc/bananas/cloud.toml`).
- Invokes `rclone` with env-var-only config — token never hits disk during sync.
- Drops privileges from root → `bananas` user before invoking `rclone` so synced files inherit `bananas:bananas` ownership under `/srv/*`.

## Provider catalog

Compiled in. Adding a new provider is a one-line const append in `src/providers.rs`. Currently shipped: Google Drive, Dropbox, OneDrive, S3-compatible, WebDAV, FTP.

## Configuration

- `/etc/bananas/cloud.toml` — accounts (with token blobs) and sync entries (managed by this component, not hand-edited)

## Logs

```bash
journalctl -u bananas-cloud.service -e
```

## Source

`crates/cloud/` in the [main repo](https://github.com/rubeniskov/bananas/tree/main/crates/cloud).
