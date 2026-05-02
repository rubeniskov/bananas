# First boot

## Sign in to the web admin

Browse to **`http://bananapro.local:8080/`** (or the IP). First sign-in is **`root` / `bananas`**, the placeholder credential the image ships with.

The login form immediately bounces you into a "Set a new password to continue" screen — the placeholder stops working the moment you rotate it.

## Recommended initial walkthrough

After rotating the password:

1. **[Mount points](./web-admin/storage.md)** — add fstab entries for any SATA / USB disk you have plugged in. The image ships with no defaults; the UI handles `mkdir`, fstab edit, and `systemctl daemon-reload`.
2. **[Exports](./web-admin/exports.md)** — declare which paths to share over NFS and to what client / CIDR. The image ships an empty `/etc/exports`; the UI rewrites it via the privileged helper.
3. **[Users](./web-admin/users.md)** — add normal admin users (member of `bananas-admin`); demote root to emergency-use.
4. **[Cloud](./web-admin/cloud.md)** (optional) — connect Google Drive / Dropbox / S3 / etc. for backup syncs.
5. **Save config** — drops a TOML bundle of the entire setup (exports + fstab + users + cloud) onto your laptop. Use **Load config** on a re-flashed card to restore in one click.

## SSH access (optional, recommended)

Root password stays unset by default. OpenSSH's default `PermitRootLogin prohibit-password` allows key auth and blocks password auth, so SSH-as-root works only if you baked an `authorized_keys` file into the image at build time. See `layers/meta-bananas/recipes-core/images/files/authorized_keys` and the `install_root_authkey` post-process step in `bananas-image.bb` for details.
