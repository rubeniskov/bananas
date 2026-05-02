# Exports

NFS export rules. Each row in the table corresponds to one line in `/etc/exports`.

## Adding an export

1. Click **+ Add export**.
2. Fill in:
   - **Path** — the directory to export. Must already exist on the box. Common pattern: a subdirectory under a mounted disk, e.g. `/srv/media/movies` once `/srv/media` is set up in [Storage](./storage.md).
   - **Client** — CIDR or hostname. `192.168.1.0/24` for a typical LAN, or a single IP.
   - **Options** — defaults are `rw,sync,no_subtree_check,all_squash,anonuid=65534,anongid=65534`.
3. **Save** — the helper rewrites `/etc/exports` and runs `exportfs -ra`.

## Protected rows

System mounts (anything created automatically by the image) render with a 🔒 lock icon and refuse edits/deletes server-side. The Save Config bundle filters them out so backups stay clean.
