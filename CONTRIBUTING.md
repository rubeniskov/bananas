# Contributing to BanaNAS

Thanks for picking this up. The repo is a Yocto image plus a Cargo workspace plus a wasm SPA — three different toolchains, all driven by `pixi run`. This guide covers the bare minimum to get a productive feedback loop and ship a change.

For the full **dev environment setup, host packages, the iterate-loop ordering, and the build-from-source walkthrough**, see [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md). The README's Quick Start is now operator-focused (flash the prebuilt SD image from a release tag); this file picks up where DEVELOPMENT.md leaves off — code style, commit messages, PR rules.

---

## TL;DR

```bash
git clone https://github.com/rubeniskov/bananas.git && cd bananas
pixi run info                 # confirm pixi env + kas resolve
pixi run setup-prek           # install prek + wire pre-commit / pre-push hooks
pixi run build                # full Yocto bake (≈30 min cold cache)
pixi run iterate              # build → bake → reboot the BPI → re-export rootfs
```

If a `pixi run` invocation says "command not found", you don't have `pixi` installed yet:

```bash
curl -fsSL https://pixi.sh/install.sh | sh
```

The detailed setup (host apt packages, Docker context, U-Boot env for netboot, SSH key drop) is in [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md).

### Pre-commit hooks (`prek`)

The repo ships a [`.pre-commit-config.yaml`](.pre-commit-config.yaml) consumed by [prek](https://github.com/j178/prek), a Rust drop-in for `pre-commit`. After running `pixi run setup-prek` once:

- **pre-commit** stage runs `cargo fmt --all -- --check` + the standard hygiene hooks (trailing whitespace, EOF newline, YAML / TOML lint, large-file guard). Cheap; runs on every `git commit`.
- **pre-push** stage adds `cargo clippy --workspace`, `cargo test --workspace`, and `cargo check -p bananas-webadmin --target wasm32-unknown-unknown`. Slower; runs on `git push`.

CI re-runs the same hooks via `prek run --all-files --hook-stage pre-push`, so anything that's clean locally is clean on the runner. To skip a hook on a one-off basis use `git commit --no-verify` (or `SKIP=cargo-clippy git push` for a single hook), but PRs that fail the CI prek job get rejected before they reach merge.

---

## What lives where

| Path | What |
|------|------|
| `crates/` | Rust workspace — server, helper, web admin (wasm), stats sampler, dashboard. |
| `layers/meta-bananas/` | Yocto layer with all BanaNAS-specific recipes (`recipes-bsp/`, `recipes-core/`, `recipes-kernel/`, `recipes-graphics/`). |
| `kas.yml` | Layer composition (poky, meta-openembedded, meta-sunxi, meta-arm pinned to `scarthgap`) + `local_conf_header` overrides. |
| `pixi.toml` | All build/run tasks (`build-webadmin`, `build-server-arm`, `build-stats-arm`, `build-dashboard-arm`, `setup-rclone-arm`, `iterate`, …). |
| `compose.yml` | TFTP + NFS containers for the netboot iterate loop. |
| `serve/` | Build artifacts staged for Yocto recipes (`serve/bin/` = prebuilt arm binaries; `serve/webadmin/` = wasm SPA bundle). Gitignored. |
| `assets/` | Source-of-truth artwork (the splash png lives here; recipe-side bbfile copies are derived). |
| `docs/` | Hardware references + LCD config notes + screenshots. |
| `memory/` | Decision logs / "why we deferred X" notes. Read these before reviving a deferred backlog item. |

---

## Dev workflow

### Web UI only

`crates/webadmin` is a Dioxus 0.7 wasm app. Edit Rust + CSS, then:

```bash
pixi run build-webadmin  # rebuilds the wasm bundle into serve/webadmin/
pixi run iterate         # ships it (bake will be fast — only the SPA changes)
```

### Server / helper / stats (Rust crates that run on the BPI)

```bash
pixi run build-server-arm     # bananas-webadmin + bananas-engine via cargo-zigbuild
pixi run build-stats-arm      # bananas-stats via cargo-zigbuild
pixi run build-dashboard-arm  # bananas-dashboard via cross-rs (Slint needs system libs)
pixi run iterate
```

### Recipe-only changes (Yocto)

```bash
pixi shell
kas shell kas.yml -c 'bitbake -c <task> <recipe>'   # e.g. -c devshell linux-mainline
```

The two-pass `pixi run build` (`bitbake -c rootfs -f bananas-image && bitbake bananas-image`) is what you want for normal "rebake" iteration.

### Where the bake fails most often

- **Provider conflicts** (`Nothing PROVIDES virtual/X`): something flips a `PREFERRED_PROVIDER` based on `MACHINEOVERRIDES`. Check `bananas-bpi.conf` order — overrides set *after* `require sun7i.inc` won't propagate to includes inside that require chain.
- **Patch-status QA** (`Missing Upstream-Status in patch`): every patch in `SRC_URI` needs `Upstream-Status: …` in the header. Use `Inappropriate [<reason>]` for repo-specific changes.
- **`file-rdeps`**: a binary's NEEDED entry has no provider in `RDEPENDS`. Yocto's package names sometimes have SONAME suffixes (`libdrm` → no, `libdrm` is the package name; the file is `libdrm2.ipk`). Read `build/tmp/deploy/ipk/<machine>/` if unsure what's actually built.

---

## Code style

### Rust

- `rustfmt` defaults — `cargo fmt` from the workspace root.
- `cargo clippy --all-targets --workspace` should be clean. Per-crate `#[allow(non_snake_case)]` is fine for Dioxus components since `#[component]` requires PascalCase.
- Prefer `?` and explicit error types over `unwrap` outside of tests / smoke paths.
- New helper commands need to be allowlisted, not path-driven — see `is_safe_perms_path` and `service_config_target` in `crates/engine/src/main.rs`.

### Yocto recipes

- One-line `SUMMARY`, longer `DESCRIPTION` — but **don't mix `\"` and unescaped apostrophes** in either. Bitbake's parser will reject the line. ([Hit twice already](memory/feedback_bitbake_strings.md).)
- All shipped patches need `Upstream-Status:`.
- `INHIBIT_PACKAGE_STRIP = "1"` + `INSANE_SKIP:${PN} += "arch already-stripped"` for prebuilt-binary recipes (everything in `serve/bin/` lands that way).
- New providers go in a layer with priority ≥ `meta-sunxi`'s 10 if you need to override a `=` assignment; otherwise use `:remove` / `:append` operators which are order-independent.

### CSS / wasm

- Stick with the existing variable-driven palette (`--accent`, `--ok`, `--warn`, `--danger`).
- Tooltip strings live in HTML attributes (`data-tip="..."`); avoid `title=` because it conflicts with the form's native browser tooltip.

---

## Commit messages

The release pipeline (semantic-release via GitHub Actions) derives the next version from your commit messages. Use the [Conventional Commits](https://www.conventionalcommits.org/) format:

| Prefix | When | Triggers |
|--------|------|----------|
| `feat:` | New user-visible feature | Minor version bump |
| `fix:` | Bug fix | Patch version bump |
| `docs:` | Documentation only | No release |
| `chore:` | Internal cleanup, no behavior change | No release |
| `refactor:` | Code change without feature/fix | No release |
| `test:` | Tests only | No release |
| `BREAKING CHANGE:` (in body) or `feat!:` | Backwards-incompatible change | Major version bump |

Optional scope in parens — `feat(server):`, `fix(dashboard):`, `feat(image):` — picks up nicely in the generated `CHANGELOG.md`.

Examples (lifted from this repo's history):

- `feat(stats): live Unix-socket pub/sub; remove SQLite read amplification`
- `fix(psplash): use outsuffix=default so /usr/bin/psplash actually ships`
- `chore(docker): drop DOCKER_CONTEXT prefixes; require active context = default`

Co-authored-by trailers are encouraged when the change is collaborative.

---

## Pull requests

1. Open against `main`. PRs against any other branch are auto-rejected by the CI workflow.
2. Run the equivalent of `pixi run build` locally and confirm it bakes cleanly. The CI re-runs this in a clean container.
3. If the change touches the BPI hardware path (kernel cfg, DT, U-Boot, panel timings), include a paste of the resulting boot log from a real iterate.
4. Update the relevant `docs/` page if the change introduces a new on-device file or breaks an existing one.
5. The CI runs `cargo test --workspace` + the wasm + arm cross-builds — these need to pass before merge.

---

## Hardware target

This repo currently bakes two MACHINEs: `bananas-bpi` (LeMaker BananaPro / BPI-M1+ — Allwinner A20, armv7) and `bananas-rpi` (unified Raspberry Pi 3/4/5 — aarch64). PRs that introduce another machine config are welcome but should keep both existing paths green:

- New `MACHINE_FEATURES` should be guarded behind a machine override that doesn't fire on `bananas-bpi` / `bananas-rpi` unless explicitly intended.
- New per-machine recipes should widen `COMPATIBLE_MACHINE = "(bananas-bpi|bananas-rpi|<your-machine>)"`.

---

## Backlog memory

`memory/` holds long-form decision logs for items we deliberately deferred. Read those before reviving a deferred path — they capture the reasons we stopped and the cheapest path back. Examples:

- `project_gpt_repair.md` — why GPT repair on the 24 TB drive needs an off-board x86_64 host.
- `project_mali_gpu.md` — the Mali-400 lima/femtovg revival recipe.
- `project_handoff_blackout.md` — the U-Boot → kernel display blackout, with cheap mitigations + the real fix.
- `feedback_bitbake_strings.md` — the recipe-string parse-error trap (don't mix `\"` and apostrophes).

Adding a new memo when you defer something is a courtesy to whoever picks it up later.

---

## License & copyright

By submitting a PR you agree that your contribution is licensed under the project's [MIT License](LICENSE).
