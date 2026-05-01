//! Header dropdown structure: the documented order holds, Settings
//! + Updates aren't in the main nav anymore, and clicking either
//! navigates to the corresponding hash route.
//!
//! Ported from `tests/e2e/dropdown.test.mjs`. The original used
//! two 400 ms sleeps after Settings + Updates clicks; we wait on
//! the matching `location.hash` predicate instead.

#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use bananas_devtool::{Browser, Harness, TEST_PASSWORD};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Item {
    cls: String,
    label: String,
}

enum Want {
    Theme,
    Item(&'static str),
}

const EXPECTED: &[Want] = &[
    // section 1: theme
    Want::Theme,
    // section 2: config + nav
    Want::Item("Save config"),
    Want::Item("Load config"),
    Want::Item("Updates"),
    Want::Item("Settings"),
    // section 3: destructive / session-end
    Want::Item("Reboot"),
    Want::Item("Sign out"),
];

#[tokio::test]
async fn dropdown_structure_and_navigation() -> Result<()> {
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

    // Settings + Updates must NOT be in the main nav anymore.
    let settings_in_nav = page.count(r#"nav.app-nav a"#).await?;
    assert!(settings_in_nav > 0, "main nav must render some tabs");
    let nav_has_settings: bool = page.evaluate(
        r#"() => Array.from(document.querySelectorAll('nav.app-nav a')).some(a => a.textContent.trim() === 'Settings')"#,
    ).await?;
    assert!(
        !nav_has_settings,
        "Settings still in main nav — should be in dropdown"
    );
    let nav_has_updates: bool = page.evaluate(
        r#"() => Array.from(document.querySelectorAll('nav.app-nav a')).some(a => a.textContent.trim() === 'Updates')"#,
    ).await?;
    assert!(
        !nav_has_updates,
        "Updates still in main nav — should be in dropdown"
    );

    // Open the menu.
    page.click_css("button.user-menu-trigger").await?;
    page.wait_for_function(r#"() => !!document.querySelector('.user-menu-popover')"#)
        .await?;

    // Walk the popover children in DOM order, asserting against EXPECTED.
    let items: Vec<Item> = page
        .evaluate(
            r#"() => {
              const popover = document.querySelector('.user-menu-popover');
              return Array.from(popover.children).map(el => ({
                cls: el.className,
                label: el.querySelector('span:not(.theme-picker-label)')?.textContent ?? '',
              }));
            }"#,
        )
        .await?;

    let mut cursor = 0usize;
    let mut divider_count = 0usize;
    for want in EXPECTED {
        while cursor < items.len() && items[cursor].cls.contains("user-menu-sep") {
            cursor += 1;
            divider_count += 1;
        }
        assert!(
            cursor < items.len(),
            "ran out of dropdown items at {cursor}"
        );
        let got = &items[cursor];
        match want {
            Want::Theme => {
                assert!(
                    got.cls.contains("theme-picker"),
                    "expected theme picker at index {cursor}, got {got:?}"
                );
            }
            Want::Item(label) => {
                assert_eq!(
                    got.label.trim(),
                    *label,
                    "expected {label:?} at index {cursor}, got {got:?}"
                );
            }
        }
        cursor += 1;
    }
    assert_eq!(
        divider_count, 2,
        "expected 2 dividers between sections, saw {divider_count}"
    );

    // "Settings" item navigates to /#settings and renders the
    // settings page (single section — no tabs since the schema
    // renaming).
    page.click_text(".user-menu-popover button", "Settings")
        .await?;
    page.wait_for_function(r#"() => location.hash === '#settings'"#)
        .await?;
    let url = page.url().await?;
    assert_eq!(url, format!("{origin}/#settings"));
    let settings_tabs = page.count(".settings-tabs").await?;
    assert_eq!(
        settings_tabs, 0,
        "Settings page still renders tabs; expected single section"
    );

    // "Updates" item navigates to /#updates.
    page.click_css("button.user-menu-trigger").await?;
    page.wait_for_function(r#"() => !!document.querySelector('.user-menu-popover')"#)
        .await?;
    page.click_text(".user-menu-popover button", "Updates")
        .await?;
    page.wait_for_function(r#"() => location.hash === '#updates'"#)
        .await?;
    assert_eq!(page.url().await?, format!("{origin}/#updates"));

    Ok(())
}
