//! Build both the SLINT app's UI module and the wasm SPA's dist
//! tree. This crate ships three binaries:
//!  - `bananas-dashboard` (SLINT LCD app)
//!  - `bananas-dashboard-web` (web plugin daemon)
//!  - `bananas-dashboard-web-ui` (wasm SPA, gated by `wasm-ui`)
//!
//! `slint_build::compile` is the historical step; the dx-runner
//! block was lifted from `crates/cloud/build.rs` so the web
//! daemon's `embedded.rs` finds `$OUT_DIR/ui/`.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Slint compiles `ui/dashboard.slint` into Rust pulled in by
    // `src/app.rs::slint::include_modules!()`. Always run; idempotent
    // and cheap.
    slint_build::compile("ui/dashboard.slint").expect("compiling dashboard.slint");

    // Skip dx for self-recursion: when dx invokes cargo for the
    // wasm32 SPA bin, this build.rs runs again with TARGET=wasm32-…
    // We must no-op there or we infinite-loop.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("wasm32-") {
        return;
    }

    println!("cargo:rerun-if-changed=src/ui");
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=Dioxus.toml");
    println!("cargo:rerun-if-changed=index.html");

    let Ok(out) = std::env::var("OUT_DIR") else {
        return;
    };
    let out = PathBuf::from(out);
    let ui_out = out.join("ui");
    let _ = std::fs::remove_dir_all(&ui_out);

    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    let pkg = std::env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME set by cargo");
    // Web SPA bin is `<pkg>-web-ui` (the `web` infix matches the
    // web-daemon bin name `<pkg>-web`).
    let bin = format!("{pkg}-web-ui");

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

    // If dx isn't on PATH this is the cross-rs Slint LCD
    // cross-build path (build-dashboard-arm pins
    // `--bin bananas-dashboard`). Only the LCD bin compiles in
    // that container, and it doesn't reference $OUT_DIR/ui — so
    // the empty SPA tree is fine here. The web daemon
    // (bananas-dashboard-web) cross-builds natively in
    // build-webadmin-arm where dx is always present and the
    // real SPA bytes get baked in via include_dir!.
    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("cargo:warning=dx not on PATH; skipping SPA build (cross-rs LCD path)");
            std::fs::create_dir_all(out.join("ui")).ok();
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

    let mut copy_opts = fs_extra::dir::CopyOptions::new();
    copy_opts.overwrite = true;
    copy_opts.copy_inside = true;
    fs_extra::dir::copy(public, &ui_out, &copy_opts).expect("copying dx output to OUT_DIR/ui");

    extract_and_strip_index(&out, &ui_out);

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
