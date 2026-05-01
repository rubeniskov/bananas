//! Compiles every `.proto` file under `proto/` into Rust modules
//! at $OUT_DIR. Uses `protoc-bin-vendored` so cross-builds (Yocto
//! bitbake, cargo-zigbuild) don't depend on a system-installed
//! protoc binary on the build host.
//!
//! Each generated module is named after its package: `bananas.health.v1`
//! → `bananas.health.v1.rs`. `src/lib.rs` re-exports them under
//! Rust-friendly module paths.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Pin protoc to the vendored binary. tonic-build picks this up
    // via the PROTOC env var.
    unsafe {
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }

    println!("cargo:rerun-if-changed=proto");

    // Inputs: every *.proto under proto/. Walk the dir explicitly
    // so adding a new proto file just requires dropping it in
    // place.
    let proto_dir = std::path::PathBuf::from("proto");
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&proto_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("proto") {
            files.push(path);
        }
    }

    // tonic 0.14 split prost-based codegen out into a separate
    // crate. The shape we want — server traits + client stubs
    // generated from prost-derived messages — lives in
    // `tonic_prost_build::configure()`.
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        // Re-export the generated `mod`s under nicer Rust names —
        // `bananas.health.v1` becomes `bananas::health::v1`. See
        // src/lib.rs for the include glue.
        .compile_protos(&files, &[proto_dir])?;

    Ok(())
}
