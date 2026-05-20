# Backups & cloud sync

## Strategy

A typical BanaNAS backup setup looks like this:

1. **Stage local data** under `/srv/<some-label>/` via the [Storage tab](./web-admin/storage.md).
2. **Connect a cloud account** in the [Cloud tab](./web-admin/cloud.md) — Google Drive, Dropbox, OneDrive, S3-compatible, WebDAV, or FTP.
3. **Create a sync entry** that maps a local path under `/srv/` to a remote path in the cloud account.
4. **Run on demand** when you want to push (or schedule it via the entry's options).

## What gets stored on disk

Token storage is kept conservative:

- **Tokens** live in `/etc/bananas/cloud.toml`. The web admin redacts them from `GET /api/cloud/accounts`.
- **`rclone.conf`** is **not** written — sync invocations build the config in env vars at runtime (`RCLONE_CONFIG_<NAME>_TOKEN=…`) so the token is never persisted in `rclone`'s native config.
- **Sync entries** (source/destination/schedule) live in the same `cloud.toml` as the accounts.

## Restoring on a re-flashed card

Cloud accounts and sync entries round-trip through Save / Load Config. Flow:

1. Before re-flashing: **Save config** in the web admin → downloads a TOML bundle.
2. Flash a fresh image, complete the password rotation.
3. **Load config** → uploads the bundle, restores accounts (with tokens) and sync entries in one click.

The bundle includes shadow hashes for users + the cloud token blobs, so don't email it around or check it into a public repo.

## Privilege model during sync

The sync runner drops to the `bananas` user (UID/GID `1000`) before exec'ing `rclone`. Files written under `/srv/*` end up owned by `bananas:bananas`, which matches the default user the web admin's user-management code creates and what NFS clients see when they mount with `all_squash`.
