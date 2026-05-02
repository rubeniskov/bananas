# Conventions

The bits the pre-commit hooks and CI will block on, plus the unwritten rules.

## Code style (Rust)

- **Edition 2024** across the workspace.
- **`cargo fmt`** is enforced — the pre-commit `cargo fmt --check` hook blocks commits that change Rust files without a fmt pass. Run `cargo fmt --all` before committing if you don't have format-on-save.
- **Workspace `version` and `edition`** in `[package]` should use `{ workspace = true }`. New crates inherit the workspace version (single source of truth: `assets/version.txt`).
- No `unsafe` outside of FFI shims unless there's a comment explaining why a safe alternative doesn't work.

## Commits — conventional commits

The project uses [Conventional Commits](https://www.conventionalcommits.org/). The release workflow's semantic-release step parses the log to compute the next version bump.

| Prefix | When | Bump |
|---|---|---|
| `feat:` | New feature, new plugin, new tab | minor |
| `fix:` | Bug fix | patch |
| `docs:` | Docs-only change (README, mdbook, comments) | none |
| `chore:` | Tooling, deps, gitignore, anything not feature/fix/docs | none |
| `ci:` | Workflow file changes | none |
| `refactor:` | Code reshuffle without behavior change | none |
| `test:` | Adding/modifying tests | none |
| `feat!:` or `fix!:` (with `!`) | Breaking change | major |

Scoped variants are common and welcome: `feat(docs):`, `fix(e2e):`, `ci(release):`, `refactor(server):`. Use the directory or domain that the change primarily touches.

The body should explain **why**, not what — the diff already shows what. A good rule: read your commit in six months and ask "would I understand the motivation?"

## Pre-commit hooks

This repo runs a small `prek` hook set on every commit:

- `trim trailing whitespace`
- `fix end of files` (single trailing newline)
- `check for merge conflicts` (no `<<<<<<< HEAD` markers)
- `check yaml` / `check toml` (well-formed if changed)
- `check for added large files` (rejects > a few MB to keep clones fast)
- `cargo fmt --check` (Rust files only)

If a hook fails, **fix the underlying problem and re-stage** — don't bypass with `--no-verify`. The hooks ran for a reason, usually a small reason that takes 30 seconds to address.

For pre-commit hook failures that block legitimate work (e.g. you added a generated binary the large-file check rejects), open a PR adjusting the hook config rather than carrying `--no-verify` in your workflow.

## Versioning

There is **one version** for the whole workspace and the SD image: `assets/version.txt`. `scripts/bump-cargo-version.sh` writes Cargo.toml + this file in lockstep. Yocto recipes pull it in via `recipes-bsp/bananas-version.inc`, so every `.ipk` (PV = "...") matches `cargo --version` of the same tag.

Don't hand-edit `assets/version.txt` or per-crate `version = "..."` — `semantic-release` does both during the release pipeline.

## YAML / TOML / config files

- 2-space indent (mdbook book.toml, recipe `.toml`, GitHub workflow `.yml`).
- TOML keys aligned by the longest key in the same block when it improves readability:

  ```toml
  id          = "cloud"
  label       = "Cloud"
  socket      = "/run/bananas/cloud.sock"
  api_prefix  = "/api/cloud"
  ```

  Don't go nuts with this — when a block grows past ~6 keys it's usually clearer to drop the alignment.

- GitHub workflow steps grouped by purpose with a header comment above each group, the way `release.yml` is currently structured.

## File and directory naming

- Yocto recipes: `bananas-<id>` (kebab-case).
- Rust crates inside the workspace: same name, e.g. `crates/cloud/` produces `bananas-cloud`. `Cargo.toml` `[package] name` is the full `bananas-cloud`; the directory keeps the short form.
- The plugin manifest in `/etc/bananas/extensions.d/` uses the bare id (no `bananas-` prefix): `cloud.toml`, not `bananas-cloud.toml`.
- Per-service config: `/etc/bananas/<id>.toml` — same naming.
- Per-service runtime data: `/var/lib/bananas/<id>/` if the plugin needs more than a single file; otherwise individual files at the `/var/lib/bananas/` root with the `<id>` prefix.

## Pinning rules

- **Yocto release line** is `scarthgap`. If you bump it, update every `refspec` in `kas.yml` together and re-test the host-toolchain `.bbappend` files. Version-pinned appends like `elfutils_0.191.bbappend` and `dtc_1.7.0.bbappend` will silently stop applying when the upstream recipe version changes.
- **Watch for wrynose (Yocto 6.0 LTS)** to ship. When `git ls-remote https://git.yoctoproject.org/poky refs/heads/wrynose` returns a SHA, revisit the bake stack: bump every `kas.yml` refspec to `wrynose`, drop the `meta-lts-mixins` repo entry (rust 1.94+ ships natively), audit the version-pinned bbappends.
- **`pixi.lock`** is marked `merge=binary linguist-generated=true -diff` in `.gitattributes`. Regenerate via `pixi` rather than hand-editing or attempting a 3-way merge.

## Tests

The repo has Rust unit tests per crate (`crates/*/tests/`) and a small chromiumoxide-based e2e suite for the web admin (run via `pixi run e2e`). When you add a new tab or a new privileged engine op, add a test that exercises it.

Don't mock the Unix socket or the engine in integration tests if you can avoid it — past experience says mocked tests pass while the real interaction breaks.

## Pull request etiquette

- Keep PRs small enough to review in one sitting. A new plugin is one PR; a new plugin plus a refactor of the engine RPC is two.
- Include the test plan in the PR body — what you tested, what you didn't, what reviewers should poke at.
- The CI runs the full Yocto bake on every release tag (~25-30 minutes). For PRs, only the lighter cargo + e2e checks run; the bake fires on merge-to-main + release.
