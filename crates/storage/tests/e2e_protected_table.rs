//! Capture the storage Mount-points table with and without
//! "Show protected mounts" toggled. The Harness pre-writes a
//! fstab fixture (3 protected + 3 regular) so the table has rows
//! either way.
//!
//! The test always passes — its purpose is producing two PNGs in
//! `screenshots/` for the operator to eyeball when something
//! looks broken in the rendered table. Run with:
//!
//!     cargo test -p bananas-storage --test e2e_protected_table
//!
//! Outputs:
//!   - `screenshots/storage-mounts-default.png`   — only user rows
//!   - `screenshots/storage-mounts-protected.png` — every row
//!
//! The protected-on screenshot is the one to look at when the
//! user reports "table is broken when I toggle protected mounts".

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;

use anyhow::Result;
use bananas_devtool::{Browser, Harness, TEST_PASSWORD};

fn screenshots_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("screenshots");
    std::fs::create_dir_all(&dir).expect("mkdir screenshots/");
    dir
}

#[tokio::test]
async fn storage_table_with_and_without_protected() -> Result<()> {
    let h = Harness::new().await?;
    let cookie = h.login("root", TEST_PASSWORD).await?;
    let origin = h.origin();

    let browser = Browser::launch().await?;
    let page = browser.new_page(&origin, &cookie).await?;
    page.goto(&format!("{origin}/#storage")).await?;
    page.wait_for_function(
        r#"() => {
          const r = document.getElementById('storage-mfe-root');
          return !!r && r.innerHTML.length > 100;
        }"#,
    )
    .await?;
    // The mounts table has loaded once at least one fstab row is
    // visible. The default fixture has 3 user rows so they should
    // show even with protected hidden.
    page.wait_for_function(
        r#"() => document.querySelectorAll('#storage-mfe-root table.rows tbody tr').length > 0"#,
    )
    .await?;

    let dir = screenshots_dir();
    page.save_screenshot_png(&dir.join("storage-mounts-default.png"))
        .await?;

    // Toggle "Show protected mounts" on.
    page.click_css(r#"#storage-mfe-root label.muted-toggle input[type="checkbox"]"#)
        .await?;
    page.wait_for_function(
        r#"() => document.querySelector('#storage-mfe-root label.muted-toggle input[type="checkbox"]').checked"#,
    )
    .await?;
    // Wait for the row count to grow — fixture has 3 protected
    // rows added on top of the 3 regular ones.
    page.wait_for_function(
        r#"() => document.querySelectorAll('#storage-mfe-root table.rows tbody tr').length >= 6"#,
    )
    .await?;

    page.save_screenshot_png(&dir.join("storage-mounts-protected.png"))
        .await?;

    Ok(())
}
