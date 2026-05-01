//! MFE handshake for the Stats plugin. Stats is the default tab,
//! so the handshake fires from `goto(/)` rather than from a click.
//! No click_text is needed; just wait for the mount.

#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use bananas_devtool::{Browser, Harness, TEST_PASSWORD};

#[tokio::test]
async fn stats_mfe_handshake() -> Result<()> {
    let h = Harness::new().await?;
    let cookie = h.login("root", TEST_PASSWORD).await?;
    let origin = h.origin();

    let browser = Browser::launch().await?;
    let page = browser.new_page(&origin, &cookie).await?;
    page.goto(&format!("{origin}/")).await?;

    page.wait_for_function(
        r#"() => {
          const r = document.getElementById('stats-mfe-root');
          return !!r && r.innerHTML.length > 100;
        }"#,
    )
    .await?;
    assert_eq!(page.url().await?, format!("{origin}/#stats"));
    Ok(())
}
