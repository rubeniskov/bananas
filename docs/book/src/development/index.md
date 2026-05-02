# Development

This section is for people who want to **extend BanaNAS** — write a new tab for the web admin, add a daemon that talks to the engine, ship a feature behind an `opkg install` instead of baking it into every release.

## What "extending" means here

The default SD image is deliberately small. Only the NAS-essentials are baked in:

- `bananas-engine` — the root-privileged helper
- `bananas-router` — the public HTTP gateway
- `bananas-webadmin` — the SPA shell + session layer
- `bananas-storage`, `bananas-users`, `bananas-exports` — the core admin surface
- `bananas-stats` (+ `bananas-stats-web`) — live metrics

Everything else is a **plugin** — a separate IPK package in the GitHub-Pages opkg feed that an operator installs on demand:

```bash
opkg update
opkg install bananas-cloud       # rclone-based backup + sync
opkg install bananas-dashboard   # Slint LCD app + web SPA
opkg install bananas-config      # operator TUI / CLI
```

Adding a new plugin means adding **one Rust crate** + **one Yocto recipe** + **one extension manifest**. The recipe builds an IPK; the manifest tells the router and the webadmin how to wire the plugin in. There is no "register plugin with the kernel" step — the integration is purely conventional and based on file layout.

## When to write a plugin vs. modify the core

Build a plugin when:

- Your feature is **opt-in** — not every operator wants it.
- It has **its own tab** in the web admin (Cloud, Dashboard config, etc.).
- It depends on **heavy packages** an operator might not want (rclone is ~50 MB; the Slint dashboard pulls in DRM/KMS userspace).
- It can run as a **separate process** behind the router, talking to the engine for any privileged work.

Modify the core when:

- The behavior change applies to **every BanaNAS install** by default (e.g. a new tab in the always-shipped Storage flow).
- You're touching a primitive other plugins depend on (engine RPC, session model, manifest format).
- You're adding a host-toolchain workaround to a `*-native` recipe (the `recipes-devtools/` `.bbappend` pattern).

## What this section covers

- **[Architecture](./architecture.md)** — the privilege model, engine RPC, router URL mounts, and the extension manifest.
- **[Dev environment](./dev-setup.md)** — `pixi`, the network-boot dev loop, building one recipe at a time.
- **[Creating a plugin (IPK)](./creating-a-plugin.md)** — the full step-by-step, from `cargo new` to `opkg install`.
- **[Conventions](./conventions.md)** — code style, conventional commits, pre-commit hooks, the version-pinning rule.

If you're contributing to the core (not a plugin), most of the same guidance applies — just skip the recipe/IPK chapter.
