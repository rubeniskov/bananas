//! MFE handshake for the Exports plugin. Click the Exports nav
//! tab, host MFE loader fetches /api/exports/__mfe_entry, plugin
//! SPA mounts at `exports-mfe-root`, URL settles at `/#exports`.

#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use bananas_devtool::{Browser, Harness, TEST_PASSWORD};

#[tokio::test]
async fn exports_mfe_handshake() -> Result<()> {
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

    page.click_text("nav.app-nav a", "Exports").await?;
    page.wait_for_function(
        r#"() => {
          const r = document.getElementById('exports-mfe-root');
          return !!r && r.innerHTML.length > 100;
        }"#,
    )
    .await?;
    assert_eq!(page.url().await?, format!("{origin}/#exports"));
    Ok(())
}
