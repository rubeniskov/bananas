//! Stats config modal: the primary Save action lives in the
//! `modal-footer`, not buried inside the form body.

#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use bananas_devtool::{Browser, Harness, TEST_PASSWORD};

#[tokio::test]
async fn stats_config_modal_save_lives_in_footer() -> Result<()> {
    let h = Harness::new().await?;
    let cookie = h.login("root", TEST_PASSWORD).await?;
    let origin = h.origin();

    let browser = Browser::launch().await?;
    let page = browser.new_page(&origin, &cookie).await?;
    page.goto(&format!("{origin}/#stats")).await?;
    page.wait_for_function(
        r#"() => {
          const r = document.getElementById('stats-mfe-root');
          return !!r && r.innerHTML.length > 100;
        }"#,
    )
    .await?;

    // The "Stats config" button (gear icon + label) in the
    // section header opens the modal. Wait for it to render
    // before clicking — initial paint may show only the
    // skeleton.
    page.wait_for_function(
        r#"() => Array.from(document.querySelectorAll('#stats-mfe-root .section-header button')).some(b => (b.textContent || '').includes('Stats config'))"#,
    )
    .await?;
    page.click_text("#stats-mfe-root .section-header button", "Stats config")
        .await?;
    page.wait_for_function(r#"() => !!document.querySelector('.stats-config-modal')"#)
        .await?;

    // Footer must exist and contain a primary Save submit
    // button referencing the in-body form by id.
    let footer_save: bool = page
        .evaluate(
            r#"() => {
              const btn = document.querySelector('.stats-config-modal .modal-footer button.primary[type="submit"]');
              return !!btn && btn.getAttribute('form') === 'stats-config-form';
            }"#,
        )
        .await?;
    assert!(
        footer_save,
        "expected a primary submit button in the modal footer with form=\"stats-config-form\""
    );

    // The OLD inline `.settings-actions` Save block must not be
    // rendered when the form is in modal mode.
    let inline_save: usize = page.count(".stats-config-modal .settings-actions").await?;
    assert_eq!(
        inline_save, 0,
        "inline save block should be suppressed when the modal owns the footer"
    );

    Ok(())
}
