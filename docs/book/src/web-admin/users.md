# Users

System user + group management.

## Adding a user

1. Click **+ Add user**.
2. Fill in:
   - **Username** — POSIX username (`a-z0-9_-`, ≤ 32 chars).
   - **Password** — hashed with SHA-512 server-side; the plaintext never hits disk.
   - **Groups** — checkboxes for `bananas-admin` (web-admin login privilege), `bananas-readers`, etc.
3. **Save** — the helper updates `/etc/passwd`, `/etc/shadow`, `/etc/group` atomically.

## Demoting root

After you've created a normal admin user, you can drop `root` out of `bananas-admin` so day-to-day login uses the new account. Root SSH key auth still works (the image bakes `authorized_keys` into `/home/root/.ssh/`), so root remains available for emergency use.

## Save Config bundle

Save Config exports the user table including the SHA-512 shadow hashes, so a freshly-flashed card can be restored to the same login set in one click.
