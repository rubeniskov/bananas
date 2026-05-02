//! Hash-route URL stays canonical across click + reload across
//! every nav tab. Catches the class of bugs that bit cloud-ui
//! twice — dioxus-web's launch path mutating window.history based
//! on Dioxus.toml's base_path.
//!
//! Settings + Updates moved to the header dropdown; the dropdown
//! suite (e2e_dropdown.rs) covers their click + URL behaviour.
//!
//! Ported from `tests/e2e/nav.test.mjs`. All
//! `waitForTimeout`s replaced with predicate waits — the click
//! handler updates `location.hash` in <100 ms, so polling at 50 ms
//! catches it on the first or second tick instead of the original
//! 400+800 ms hard sleeps per tab.

#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use bananas_devtool::{Browser, Harness, TEST_PASSWORD};

const TABS: &[&str] = &["Stats", "Exports", "Storage", "Users", "Cloud"];

#[tokio::test]
async fn nav_hash_route_canonical_across_click_and_reload() -> Result<()> {
    let h = Harness::new().await?;
    let cookie = h.login("root", TEST_PASSWORD).await?;
    let origin = h.origin();

    let browser = Browser::launch().await?;
    let page = browser.new_page(&origin, &cookie).await?;
    page.goto(&format!("{origin}/")).await?;
    page.wait_for_function(
        r#"() => Array.from(document.querySelectorAll('nav.app-nav a')).some(a => a.textContent.trim() === 'Stats')"#,
    )
    .await?;

    for label in TABS {
        let slug = label.to_lowercase();
        let expected = format!("{origin}/#{slug}");

        // Wait for the nav to be re-rendered before clicking — after
        // a reload, the SPA briefly shows the loading shell with no
        // nav anchors. We need both the URL state AND the DOM mounted
        // before we trust click_text to find the right tab.
        page.wait_for_function(&format!(
            r#"() => Array.from(document.querySelectorAll('nav.app-nav a')).some(a => (a.textContent || '').includes('{label}'))"#
        ))
        .await?;

        page.click_text("nav.app-nav a", label).await?;
        page.wait_for_function(&format!("() => location.hash === '#{slug}'"))
            .await?;
        let after_click = page.url().await?;
        assert_eq!(
            after_click, expected,
            "{label} click: expected {expected}, got {after_click}"
        );

        page.reload().await?;
        page.wait_for_function(&format!(
            "() => location.hash === '#{slug}' && Array.from(document.querySelectorAll('nav.app-nav a')).some(a => (a.textContent || '').includes('{label}'))"
        ))
        .await?;
        let after_reload = page.url().await?;
        assert_eq!(
            after_reload, expected,
            "{label} reload: expected {expected}, got {after_reload}"
        );
    }
    Ok(())
}
