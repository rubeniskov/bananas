//! Container harness smoke test. Boots a fresh `bananas-test`
//! container, verifies `/api/login` answers with a real session
//! cookie, then exercises one root-only path (writing /etc/exports
//! via the engine RPC) to prove engine is actually running as
//! root inside the container.
//!
//! Marked `#[ignore]` so default `cargo test --workspace --tests`
//! doesn't fire it — the container build is a heavy prerequisite
//! and not every dev needs it locally. Run with:
//!
//!     pixi run build-test-container
//!     cargo test -p bananas-devtool --tests -- --ignored
//!
//! Re-running without rebuilding the container is fine; the test
//! launches a fresh container per run.

#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use bananas_devtool::{CONTAINER_TEST_PASSWORD, Container};

#[tokio::test]
#[ignore]
async fn container_boots_and_login_works() -> Result<()> {
    let c = Container::launch().await?;
    let cookie = c.login("root", CONTAINER_TEST_PASSWORD).await?;
    assert!(!cookie.is_empty(), "login returned an empty session cookie");

    // Sanity: the engine inside the container is running as root.
    // `id -u` of the engine process should be 0. `pgrep` is on the
    // archlinux base image.
    let out = c
        .exec(&["pgrep", "-u", "0", "-x", "bananas-engine"])
        .await?;
    assert!(
        out.ok(),
        "expected bananas-engine running as root: stdout={:?} stderr={:?}",
        out.stdout,
        out.stderr
    );

    Ok(())
}
