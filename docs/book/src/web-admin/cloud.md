# Cloud sync

`rclone`-based backups to Google Drive, Dropbox, OneDrive, S3-compatible, WebDAV, FTP. Two panels:

- **Accounts** — connected providers. Adding an account opens an `rclone authorize`-style flow that returns a token. Tokens are stored in `/etc/bananas/cloud.toml`, redacted from the API GET response, and included in TOML config bundles.
- **Sync entries** — local-path → remote-path mappings, run on demand from the UI (or scheduled — see the entry's options).

## How sync runs

When you click **Run sync** on an entry, the helper:

1. Drops privileges from root → the `bananas` user (so synced files inherit the right ownership under `/srv/*`).
2. Invokes `rclone` with **env-var-only config** — `RCLONE_CONFIG=/dev/null` plus per-account `RCLONE_CONFIG_<NAME>_TOKEN=…` env vars.
3. The access token never hits disk during the sync.

## Adding a provider

The provider catalog is a one-line const append in `crates/cloud/src/providers.rs`. If a provider is missing from the dropdown, that's where it gets added. Out-of-the-box: Google Drive, Dropbox, OneDrive, S3-compatible, WebDAV, FTP.
