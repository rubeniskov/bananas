//! Build the cloud SPA via `dx build` and stage the output under
//! `$OUT_DIR/ui/` so `src/embedded.rs` can `include_dir!` it into the
//! daemon binary. Same shape as `prost-build` for protobufs: generated
//! artifacts never touch the source tree.
//!
//! Two non-obvious bits:
//!
//! 1. **wasm32 self-build skip**: when `dx` itself recurses into cargo
//!    to compile the SPA bin for `wasm32-unknown-unknown`, this very
//!    build.rs runs again with `TARGET=wasm32-…`. We must no-op there
//!    or get an infinite-recursion deadlock. The `wasm32-` guard at
//!    the top is the loop-breaker.
//!
//! 2. **Recursive cargo via dx**: dx wraps cargo for the wasm32 build,
//!    which writes to `target/wasm32-unknown-unknown/…` — a different
//!    arch subdir from the outer cargo's host-target subdir. The
//!    outer's package-cache lock is released while build.rs runs, so
//!    the nested cargo does not deadlock.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Skip when building the SPA bin itself (TARGET=wasm32-…). The
    // outer daemon build only ever has TARGET=<host or armv7>.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("wasm32-") {
        return;
    }

    // Re-run when SPA sources or the Dioxus.toml/index.html shells
    // change. Daemon-only edits under src/api.rs etc. don't touch
    // these paths so build.rs stays skipped and incremental cargo
    // links stay fast.
    println!("cargo:rerun-if-changed=src/ui");
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=Dioxus.toml");
    println!("cargo:rerun-if-changed=index.html");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR set by cargo"));
    let ui_out = out.join("ui");

    // Wipe previous embed copy — dx is happy to overwrite, but stale
    // .br/.gz companions from a prior build would otherwise sit in
    // the include_dir tree and bloat the binary.
    let _ = std::fs::remove_dir_all(&ui_out);

    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    let pkg = std::env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME set by cargo");
    let bin = format!("{pkg}-ui");

    // Wipe stale content-hashed assets from prior dx invocations.
    // dx itself doesn't clean its asset subdirs between runs, so old
    // `*-<hash>.wasm` siblings accumulate and would inflate the
    // embedded tree. We only touch the assets/ and wasm/ subdirs —
    // wiping the whole `public/` tree breaks dx's expectation that
    // its parent dirs exist.
    let workspace_target = std::path::Path::new(&manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("target"))
        .expect("locate workspace target/");
    let dx_public = workspace_target
        .join("dx")
        .join(&bin)
        .join("release")
        .join("web")
        .join("public");
    let _ = std::fs::remove_dir_all(dx_public.join("assets"));
    let _ = std::fs::remove_dir_all(dx_public.join("wasm"));

    // dx-cli wraps cargo internally; it writes outputs under
    // `<workspace>/target/dx/<pkg>/release/web/public/`. We can't
    // redirect that with a flag (dx 0.7 only takes `--target` for
    // the target triple), so just consume the path it picks.
    let mut cmd = Command::new("dx");
    cmd.args([
        "build",
        "--release",
        "--package",
        &pkg,
        "--bin",
        &bin,
        "--platform",
        "web",
        "--features",
        "wasm-ui",
    ])
    .current_dir(&manifest_dir);

    // Strip cargo-zigbuild's armv7-targeted env vars before invoking
    // dx — the nested cargo inside dx is for wasm32, and zigbuild's
    // CC/LINKER/RUSTFLAGS overrides break that build with "Failed to
    // write executable" because they rewire its output paths.
    for (key, _) in std::env::vars() {
        if key.starts_with("CARGO_TARGET_ARMV7_")
            || key.starts_with("CARGO_TARGET_AARCH64_")
            || key == "CARGO_BUILD_TARGET"
            || key == "CARGO_BUILD_TARGET_DIR"
            || key == "CARGO_TARGET_DIR"
            || key == "RUSTFLAGS"
            || key == "CARGO_ENCODED_RUSTFLAGS"
            || (key.starts_with("CC_") && key.contains("armv7"))
            || (key.starts_with("CXX_") && key.contains("armv7"))
            || (key.starts_with("AR_") && key.contains("armv7"))
        {
            cmd.env_remove(key);
        }
    }

    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "cargo:warning=dx not on PATH; skipping SPA build (host check / cargo-only consumer)"
            );
            std::fs::create_dir_all(out.join("ui")).ok();
            // Daemon's embedded.rs include_str!s mfe_entry.txt; write
            // an empty stub so the daemon binary still compiles.
            std::fs::write(out.join("mfe_entry.txt"), b"").ok();
            return;
        }
        Err(e) => panic!("dx build failed to spawn: {e}"),
    };
    if !status.success() {
        panic!("dx build {bin} failed with exit {status}");
    }

    if !dx_public.exists() {
        panic!(
            "dx build succeeded but expected output dir is missing: {}",
            dx_public.display()
        );
    }
    let public = &dx_public;

    // Copy public/ → $OUT_DIR/ui/ so include_dir!("$OUT_DIR/ui") finds
    // a self-contained, hash-stable tree.
    let mut copy_opts = fs_extra::dir::CopyOptions::new();
    copy_opts.overwrite = true;
    copy_opts.copy_inside = true;
    fs_extra::dir::copy(&public, &ui_out, &copy_opts).expect("copying dx output to OUT_DIR/ui");

    // Extract the MFE entry-script URL once at build time, write it
    // to $OUT_DIR/mfe_entry.txt, then drop index.html (+ companions)
    // from the embedded tree — the daemon never publicly serves it,
    // and the only runtime consumer (the entry-URL string) is now a
    // compile-time constant. Saves the parse on every daemon start
    // and ~12 KB of dead bytes per binary.
    extract_and_strip_index(&out, &ui_out);

    // Pre-compress every text/wasm file with brotli (q11) and gzip
    // (level 9). The serving handler picks the right variant based
    // on Accept-Encoding — same shape as ServeDir's
    // .precompressed_br().precompressed_gzip() did when reading from
    // /usr/share/bananas/cloud-ui/.
    precompress_tree(&ui_out);
}

/// Read `<ui_out>/index.html`, scan for the first
/// `<script type="module" src="…">` tag, write the URL to
/// `<out>/mfe_entry.txt`, and delete `index.html` (+ `.br`/`.gz`
/// companions if present) from the embed tree. Panics on missing
/// or malformed input — the daemon binary cannot work without the
/// entry URL, so failing the build is the correct response.
fn extract_and_strip_index(out: &std::path::Path, ui_out: &std::path::Path) {
    let html_path = ui_out.join("index.html");
    let html = std::fs::read_to_string(&html_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", html_path.display()));
    let src = extract_module_src(&html).unwrap_or_else(|| {
        panic!(
            "no <script type=\"module\" src=\"…\"> in {}",
            html_path.display()
        )
    });
    let entry_path = out.join("mfe_entry.txt");
    std::fs::write(&entry_path, src.as_bytes())
        .unwrap_or_else(|e| panic!("write {}: {e}", entry_path.display()));

    for name in ["index.html", "index.html.br", "index.html.gz"] {
        let p = ui_out.join(name);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// Plain-string scan for the first `<script type="module" src="…">`
/// tag's src attribute. dx-cli emits a small, deterministic
/// `index.html`, so a real HTML parser is overkill.
fn extract_module_src(html: &str) -> Option<String> {
    let mut cursor = 0usize;
    while cursor < html.len() {
        let rel = html[cursor..].find("<script")?;
        let tag_start = cursor + rel;
        let close = html[tag_start..].find('>')?;
        let tag = &html[tag_start..tag_start + close];
        let is_module = tag.contains("type=\"module\"") || tag.contains("type='module'");
        if is_module {
            for needle in ["src=\"", "src='"] {
                if let Some(pos) = tag.find(needle) {
                    let after = &tag[pos + needle.len()..];
                    let quote = needle.chars().last().unwrap();
                    if let Some(end) = after.find(quote) {
                        return Some(after[..end].to_string());
                    }
                }
            }
        }
        cursor = tag_start + close + 1;
    }
    None
}

/// Walk `root` and write `.br` + `.gz` companions next to every file
/// whose extension is in the precompress set. Skips files that are
/// already compressed (any `.br`/`.gz`) so re-runs are idempotent.
fn precompress_tree(root: &std::path::Path) {
    const TARGET_EXT: &[&str] = &["wasm", "js", "css", "svg", "html"];
    fn walk(p: &std::path::Path) {
        for entry in std::fs::read_dir(p).expect("read_dir during precompress") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            let ft = entry.file_type().expect("file type");
            if ft.is_dir() {
                walk(&path);
            } else if ft.is_file() {
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default();
                if TARGET_EXT.contains(&ext) {
                    compress_one(&path);
                }
            }
        }
    }
    walk(root);
}

fn compress_one(path: &std::path::Path) {
    // brotli -Z (=quality 11) keeps companions; -k preserve, -f overwrite.
    let br = Command::new("brotli")
        .args(["-Z", "-k", "-f"])
        .arg(path)
        .status();
    if !br.map(|s| s.success()).unwrap_or(false) {
        eprintln!(
            "cargo:warning=brotli failed for {} — embedded build will skip .br variant",
            path.display()
        );
    }
    let gz = Command::new("gzip")
        .args(["-9", "-k", "-f"])
        .arg(path)
        .status();
    if !gz.map(|s| s.success()).unwrap_or(false) {
        eprintln!(
            "cargo:warning=gzip failed for {} — embedded build will skip .gz variant",
            path.display()
        );
    }
}
