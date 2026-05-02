//! Regression: clicking form controls inside plugin SPAs (Storage's
//! "Show protected mounts" toggle, Users' "Show system users"
//! checkbox, Dashboard's form fields) was incorrectly opening the
//! header dropdown. Visible on /#storage, /#users, /#dashboard.
//!
//! The dropdown trigger lives in the host webadmin SPA; the
//! checkbox lives in the plugin SPA. They share the same document
//! but render through separate dioxus-web roots — events should
//! NOT cross-fire between roots.

#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use bananas_devtool::{Browser, Harness, TEST_PASSWORD};

#[tokio::test]
async fn storage_show_protected_does_not_open_dropdown() -> Result<()> {
    let h = Harness::new().await?;
    let cookie = h.login("root", TEST_PASSWORD).await?;
    let origin = h.origin();

    let browser = Browser::launch().await?;
    let page = browser.new_page(&origin, &cookie).await?;
    page.goto(&format!("{origin}/#storage")).await?;

    // Wait for the storage SPA to mount.
    page.wait_for_function(
        r#"() => {
          const r = document.getElementById('storage-mfe-root');
          return !!r && r.innerHTML.length > 100;
        }"#,
    )
    .await?;
    // And specifically the muted-toggle label inside it.
    page.wait_for_function(
        r#"() => !!document.querySelector('#storage-mfe-root label.muted-toggle input[type="checkbox"]')"#,
    )
    .await?;

    // Pre-condition: dropdown popover is NOT in the DOM.
    let pre: usize = page.count(".user-menu-popover").await?;
    assert_eq!(pre, 0, "popover should be closed before any interaction");

    // Click the checkbox.
    page.click_css(r#"#storage-mfe-root label.muted-toggle input[type="checkbox"]"#)
        .await?;

    // Give the click a brief window to propagate any spurious
    // toggles before we check.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // The checkbox itself should now be checked (proves the click
    // landed where we intended) …
    let checked: bool = page
        .evaluate(
            r#"() => document.querySelector('#storage-mfe-root label.muted-toggle input[type="checkbox"]').checked"#,
        )
        .await?;
    assert!(checked, "the checkbox toggle didn't fire — selector wrong?");

    // … but the host dropdown must remain closed.
    let post: usize = page.count(".user-menu-popover").await?;
    assert_eq!(
        post, 0,
        "REGRESSION: clicking the storage checkbox opened the host dropdown"
    );

    Ok(())
}

#[tokio::test]
async fn users_show_system_does_not_open_dropdown() -> Result<()> {
    let h = Harness::new().await?;
    let cookie = h.login("root", TEST_PASSWORD).await?;
    let origin = h.origin();

    let browser = Browser::launch().await?;
    let page = browser.new_page(&origin, &cookie).await?;
    page.goto(&format!("{origin}/#users")).await?;
    page.wait_for_function(
        r#"() => {
          const r = document.getElementById('users-mfe-root');
          return !!r && r.innerHTML.length > 100;
        }"#,
    )
    .await?;
    page.wait_for_function(
        r#"() => !!document.querySelector('#users-mfe-root label.muted-toggle input[type="checkbox"]')"#,
    )
    .await?;

    let pre: usize = page.count(".user-menu-popover").await?;
    assert_eq!(pre, 0, "popover should be closed before any interaction");

    page.click_css(r#"#users-mfe-root label.muted-toggle input[type="checkbox"]"#)
        .await?;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let checked: bool = page
        .evaluate(
            r#"() => document.querySelector('#users-mfe-root label.muted-toggle input[type="checkbox"]').checked"#,
        )
        .await?;
    assert!(checked, "users toggle didn't actually fire");

    let post: usize = page.count(".user-menu-popover").await?;
    assert_eq!(
        post, 0,
        "REGRESSION: clicking the users system-toggle opened the host dropdown"
    );
    Ok(())
}

#[tokio::test]
async fn dashboard_form_input_does_not_open_dropdown() -> Result<()> {
    let h = Harness::new().await?;
    let cookie = h.login("root", TEST_PASSWORD).await?;
    let origin = h.origin();

    let browser = Browser::launch().await?;
    let page = browser.new_page(&origin, &cookie).await?;
    page.goto(&format!("{origin}/#dashboard")).await?;
    page.wait_for_function(
        r#"() => {
          const r = document.getElementById('dashboard-mfe-root');
          return !!r && r.innerHTML.length > 100;
        }"#,
    )
    .await?;
    // Wait for any form input inside the dashboard SPA. The
    // dashboard-config form has number / select / text inputs;
    // focus + blur on the first one is enough to exercise the
    // event path the user reported.
    page.wait_for_function(
        r#"() => !!document.querySelector('#dashboard-mfe-root input, #dashboard-mfe-root select')"#,
    )
    .await?;

    let pre: usize = page.count(".user-menu-popover").await?;
    assert_eq!(pre, 0);

    // What element actually gets clicked? Diagnostic for the
    // initial debug pass — keep it so future regressions surface
    // the offending element name.
    let target: String = page
        .evaluate(
            r#"() => {
              const el = document.querySelector('#dashboard-mfe-root input, #dashboard-mfe-root select');
              if (!el) return 'none';
              return `<${el.tagName.toLowerCase()} type=${el.type} class=${el.className}>`;
            }"#,
        )
        .await?;
    eprintln!("dashboard click target: {target}");

    page.click_css(r#"#dashboard-mfe-root input, #dashboard-mfe-root select"#)
        .await?;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let post: usize = page.count(".user-menu-popover").await?;
    assert_eq!(
        post, 0,
        "REGRESSION: clicking a dashboard form field ({target}) opened the host dropdown"
    );
    Ok(())
}
