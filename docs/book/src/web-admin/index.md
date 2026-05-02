# Web admin tour

The web admin lives at `http://bananapro.local:8080/`. It's a single-page Dioxus 0.7 wasm UI served alongside a JSON `/api/*` from a Rust HTTP server. Hash-based routing — every tab survives a full-page reload.

The five tabs:

- **[Stats](./stats.md)** — live charts (CPU, mem, network, disk, temps) backed by a WebSocket from `bananas-stats`.
- **[Exports](./exports.md)** — NFS export rules. The UI rewrites `/etc/exports` via the privileged helper.
- **[Storage](./storage.md)** — fstab mount points. The UI handles `mkdir`, fstab edit, and `systemctl daemon-reload`.
- **[Users](./users.md)** — system users + group membership.
- **[Cloud](./cloud.md)** — rclone-based cloud sync (Google Drive, Dropbox, S3, WebDAV, etc.).

Two cross-tab actions live in the header: **Save config** (downloads a TOML bundle of every tab's state) and **Load config** (restores from a saved bundle). Useful for re-flashing.
