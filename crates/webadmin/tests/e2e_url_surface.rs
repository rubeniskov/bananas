//! Public URL surface — webadmin's TCP listener exposes only the
//! host SPA at `/`, the assets sub-proxy at `/assets/*`, and the API
//! sub-proxy at `/api/*`. Plugin SPAs (cloud-ui's index.html) MUST
//! NOT leak through any other path.
//!
//! Ported from `tests/e2e/url-surface.test.mjs`. No browser needed —
//! pure HTTP probes.

#![cfg(not(target_arch = "wasm32"))]

use anyhow::Result;
use bananas_devtool::{Harness, TEST_PASSWORD};

#[tokio::test]
async fn url_surface() -> Result<()> {
    let h = Harness::new().await?;
    let _cookie = h.login("root", TEST_PASSWORD).await?;
    let origin = h.origin();
    let http = h.http();

    // / → host SPA
    let r = http.get(format!("{origin}/")).send().await?;
    assert_eq!(r.status(), 200, "/ must serve the host SPA");
    let body = r.text().await?;
    assert!(
        body.contains("<title>BanaNAS</title>"),
        "/ missing host <title>"
    );

    // /cloud/index.html → host SPA fallback (not cloud-ui)
    let r = http
        .get(format!("{origin}/cloud/index.html"))
        .send()
        .await?;
    assert_eq!(r.status(), 200, "/cloud/index.html");
    let body = r.text().await?;
    assert!(
        body.contains("<title>BanaNAS</title>"),
        "/cloud/index.html leaked plugin SPA: title not host's"
    );

    // /cloud/cloud → host SPA fallback (the original bug surface)
    let r = http.get(format!("{origin}/cloud/cloud")).send().await?;
    assert_eq!(r.status(), 200, "/cloud/cloud");
    let body = r.text().await?;
    assert!(
        body.contains("<title>BanaNAS</title>"),
        "/cloud/cloud leaked plugin SPA"
    );

    // /assets/cloud/index.html → 404 (cloud daemon refuses)
    let r = http
        .head(format!("{origin}/assets/cloud/index.html"))
        .send()
        .await?;
    assert_eq!(
        r.status(),
        404,
        "/assets/cloud/index.html: plugin daemon must refuse"
    );

    Ok(())
}
