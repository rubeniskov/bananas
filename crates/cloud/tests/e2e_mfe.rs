//! MFE handshake for the Cloud plugin: click the Cloud nav tab,
//! the host MFE loader fetches /api/cloud/__mfe_entry, injects the
//! JS shim, and the plugin SPA mounts at `cloud-mfe-root`. URL
//! settles at `/#cloud`.
//!
//! Split out from the original tests/e2e/mfe.test.mjs (one big
//! file walking every plugin) so a regression in one plugin
//! reports as `cargo test -p bananas-cloud` failing instead of
//! polluting the host crate's test output.

#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use bananas_devtool::{Browser, Harness, TEST_PASSWORD};

#[tokio::test]
async fn cloud_mfe_handshake() -> Result<()> {
    let h = Harness::new().await?;
    let cookie = h.login("root", TEST_PASSWORD).await?;
    let origin = h.origin();

    let browser = Browser::launch().await?;
    let page = browser.new_page(&origin, &cookie).await?;
    page.goto(&format!("{origin}/")).await?;
    page.wait_for_function(
        r#"() => Array.from(document.querySelectorAll('nav.app-nav a')).some(a => (a.textContent || '').includes('Stats'))"#,
    )
    .await?;

    page.click_text("nav.app-nav a", "Cloud").await?;
    page.wait_for_function(
        r#"() => {
          const r = document.getElementById('cloud-mfe-root');
          return !!r && r.innerHTML.length > 100;
        }"#,
    )
    .await?;
    assert_eq!(page.url().await?, format!("{origin}/#cloud"));
    Ok(())
}
