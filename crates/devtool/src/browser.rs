//! Thin chromiumoxide wrapper exposing the small slice of CDP we
//! need from per-crate e2e tests: launch headless chrome, navigate,
//! click via CSS or text content, evaluate JS, and wait on a JS
//! predicate.
//!
//! The "wait" primitives are deliberately predicate-driven (poll
//! `evaluate` until truthy) instead of fixed `sleep`s — the
//! playwright tests we're replacing burned ~25 s per warm run on
//! conservative `waitForTimeout`s. Predicate waits drop most of
//! that and surface a clear timeout instead of silently passing on
//! a stale page.
//!
//! Locating the chrome binary: `BANANAS_CHROME` env var if set,
//! otherwise a small allowlist of well-known install paths. The
//! `pixi run setup-chrome` task wires this up; explicit env var
//! takes precedence so tests in CI can pin a specific build.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, anyhow};
use chromiumoxide::{
    Browser as ChromiumBrowser, browser::BrowserConfig,
    cdp::browser_protocol::network::CookieParam, page::Page as ChromiumPage,
    page::ScreenshotParams,
};
use futures_util::StreamExt;
use serde::de::DeserializeOwned;

pub struct Browser {
    inner: ChromiumBrowser,
    _handler: tokio::task::JoinHandle<()>,
    // Keep the user-data-dir alive for the chrome process's
    // lifetime — chromium re-opens profile files after launch
    // (cookie store, history, etc.) and crashes if they vanish.
    _user_data_dir: tempfile::TempDir,
}

impl Browser {
    /// Launch headless chrome. The handler task in the background
    /// drains the CDP event stream — without it the page object
    /// wedges as soon as the first event arrives.
    ///
    /// Each Browser gets a unique `--user-data-dir` so multiple
    /// tests can launch chrome in parallel without colliding on
    /// chrome's `SingletonLock` profile-directory guard.
    pub async fn launch() -> Result<Self> {
        let chrome = locate_chrome().context("locate chrome binary")?;
        let user_data_dir = tempfile::tempdir().context("tempdir for chrome user-data-dir")?;
        // chromiumoxide's `.arg()` prepends `--` itself, so the
        // strings here must NOT carry the leading dashes. Doubled
        // dashes show up in chrome's crash log as `----no-sandbox`
        // and the flag is silently ignored.
        //
        // `no-sandbox` lets chrome start as a non-privileged user
        // inside CI runners (ubuntu-latest's `runner` user lacks the
        // CAP_SYS_ADMIN that chrome's setuid sandbox needs, and
        // Ubuntu 23.10+ AppArmor blocks the user-namespace fallback);
        // locally it's a no-op since the sandbox would have worked
        // anyway. `disable-dev-shm-usage` swaps /dev/shm (often
        // 64 MiB on CI containers) for /tmp so chrome doesn't OOM
        // under the smaller default.
        let config = BrowserConfig::builder()
            .chrome_executable(chrome)
            .user_data_dir(user_data_dir.path())
            .request_timeout(Duration::from_secs(15))
            .arg("no-sandbox")
            .arg("disable-dev-shm-usage")
            .build()
            .map_err(|e| anyhow!("BrowserConfig: {e}"))?;
        let (browser, mut handler) = ChromiumBrowser::launch(config)
            .await
            .context("launch chrome")?;
        let handler_task = tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            inner: browser,
            _handler: handler_task,
            _user_data_dir: user_data_dir,
        })
    }

    /// Create a new tab pre-loaded with the bananas_session cookie.
    /// Cookie scope is the test's origin (127.0.0.1:<port>) — not
    /// the real production domain — so multiple harnesses on the
    /// same host don't trample each other's session state.
    ///
    /// The cookie has to land at the browser level *before* any
    /// page navigates: chromiumoxide refuses `page.set_cookie` on
    /// `about:blank`, and setting after the first goto would race
    /// against the SPA's `/api/me` probe (which fires from the
    /// launch effect and would 401 without the cookie).
    pub async fn new_page(&self, origin: &str, cookie: &str) -> Result<Page> {
        let (host, _port) = parse_origin(origin)?;
        let param = CookieParam::builder()
            .name("bananas_session")
            .value(cookie)
            .domain(host)
            .path("/")
            .build()
            .map_err(|e| anyhow!("CookieParam: {e}"))?;
        self.inner.set_cookies(vec![param]).await?;
        let page = self.inner.new_page("about:blank").await?;
        Ok(Page { inner: page })
    }

    /// Tear the browser down — useful for tests that spin many
    /// pages and want deterministic cleanup before the harness
    /// drops.
    pub async fn close(mut self) -> Result<()> {
        let _ = self.inner.close().await;
        Ok(())
    }
}

pub struct Page {
    inner: ChromiumPage,
}

impl Page {
    /// Navigate, wait for `load`. The implicit `wait_for_navigation`
    /// matches playwright's default `waitUntil: 'load'`.
    pub async fn goto(&self, url: &str) -> Result<()> {
        self.inner.goto(url).await?;
        self.inner.wait_for_navigation().await?;
        Ok(())
    }

    /// Reload the current page and wait for the load event.
    pub async fn reload(&self) -> Result<()> {
        self.inner.reload().await?;
        self.inner.wait_for_navigation().await?;
        Ok(())
    }

    /// The current URL. Returns `""` for `about:blank`.
    pub async fn url(&self) -> Result<String> {
        Ok(self.inner.url().await?.unwrap_or_default())
    }

    /// `document.querySelector(selector).click()` if it exists.
    /// Errors when the element isn't on the page yet — pair with
    /// `wait_for_function` to ride out async DOM mounts.
    pub async fn click_css(&self, selector: &str) -> Result<()> {
        let el = self.inner.find_element(selector).await?;
        el.click().await?;
        Ok(())
    }

    /// Click the first element matching `tag` whose trimmed
    /// textContent **contains** `text`. Substring semantics mirror
    /// playwright's `:has-text("…")` — handy for tabs whose label
    /// is "Cloud sync" but the test wants to refer to it as
    /// "Cloud". Uses an injected JS function so we don't depend on
    /// chromiumoxide's XPath dialect.
    pub async fn click_text(&self, tag: &str, text: &str) -> Result<()> {
        let js = format!(
            r#"() => {{
              const t = {text:?};
              const el = Array.from(document.querySelectorAll({tag:?}))
                .find(e => (e.textContent || '').includes(t));
              if (!el) return false;
              el.click();
              return true;
            }}"#
        );
        let ok: bool = self.inner.evaluate_function(js).await?.into_value()?;
        if !ok {
            anyhow::bail!("click_text({tag:?}, {text:?}): no matching element");
        }
        Ok(())
    }

    /// Number of elements matching `selector`. Cheap; uses a
    /// single querySelectorAll round-trip.
    pub async fn count(&self, selector: &str) -> Result<usize> {
        let js = format!(
            "() => document.querySelectorAll({sel:?}).length",
            sel = selector
        );
        Ok(self.inner.evaluate_function(js).await?.into_value()?)
    }

    /// Run `js` against the page and deserialize the result.
    /// `js` should be an arrow function or expression
    /// (`() => …`). Mirrors playwright's `page.evaluate`.
    pub async fn evaluate<T: DeserializeOwned>(&self, js: &str) -> Result<T> {
        Ok(self.inner.evaluate_function(js).await?.into_value()?)
    }

    /// Save a full-page PNG screenshot to `path`. Useful from
    /// regression tests that want a visual artifact alongside the
    /// failure message — e.g. layout bugs that pass JS-level
    /// assertions but visibly break the rendered table. The
    /// `screenshots/` directory at the repo root is the
    /// conventional dump target; tests should `mkdir_p` it
    /// themselves so the path stays self-contained.
    pub async fn save_screenshot_png(&self, path: &std::path::Path) -> Result<()> {
        let params = ScreenshotParams::builder()
            .full_page(true)
            .omit_background(false)
            .build();
        let bytes = self.inner.screenshot(params).await?;
        std::fs::write(path, &bytes).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Poll `predicate_js` (a JS arrow function returning a truthy
    /// value) every 50 ms until it succeeds, or fail at the
    /// 10 s deadline. Replaces fixed `waitForTimeout` sleeps in
    /// the playwright suite — the underlying signals fire in
    /// ~hundreds of ms, so this typically returns much faster.
    pub async fn wait_for_function(&self, predicate_js: &str) -> Result<()> {
        // BANANAS_E2E_TIMEOUT_MS lets CI bump the deadline without
        // recompiling — GitHub Actions runners are noticeably slower
        // than dev laptops at SPA paint, so the 10 s default tends
        // to flake on the first navigation. Locally the variable is
        // unset and the fast default applies.
        let timeout_ms: u64 = std::env::var("BANANAS_E2E_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10_000);
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let ok = self
                .inner
                .evaluate_function(predicate_js)
                .await
                .ok()
                .and_then(|r| r.into_value::<bool>().ok())
                .unwrap_or(false);
            if ok {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                anyhow::bail!("wait_for_function timed out: {predicate_js}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Resolve the chrome binary path. Order: `BANANAS_CHROME` env,
/// playwright cache (already on disk for the legacy harness),
/// `/usr/bin/chromium`, `/usr/bin/google-chrome*`. Last resort:
/// the same name on `$PATH` (delegated to chromiumoxide).
fn locate_chrome() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("BANANAS_CHROME") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
    }
    let candidates = [
        "/usr/bin/chromium",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/google-chrome",
    ];
    for c in candidates {
        if std::path::Path::new(c).exists() {
            return Ok(PathBuf::from(c));
        }
    }
    // Playwright keeps a versioned chromium under
    // ~/.cache/ms-playwright/chromium-*/chrome-linux*/chrome.
    if let Some(home) = dirs_home() {
        let pw = home.join(".cache").join("ms-playwright");
        if pw.exists() {
            for entry in std::fs::read_dir(&pw).ok().into_iter().flatten().flatten() {
                let chrome = entry.path().join("chrome-linux64").join("chrome");
                if chrome.exists() {
                    return Ok(chrome);
                }
                let chrome = entry.path().join("chrome-linux").join("chrome");
                if chrome.exists() {
                    return Ok(chrome);
                }
            }
        }
    }
    anyhow::bail!("no chrome binary found — set BANANAS_CHROME or install /usr/bin/chromium")
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn parse_origin(origin: &str) -> Result<(String, u16)> {
    // origin is `http://127.0.0.1:<port>` from `Harness::origin()`.
    let s = origin
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let (host, port) = s
        .split_once(':')
        .ok_or_else(|| anyhow!("origin missing port: {origin}"))?;
    Ok((host.to_string(), port.parse().context("port parse")?))
}
