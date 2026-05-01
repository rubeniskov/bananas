// MFE handshake works end-to-end: click Cloud → /api/cloud/__mfe_entry
// fires → script tag injected → cloud-ui mounts inline.
export default async function ({ origin, page }) {
  await page.goto(`${origin}/`, { waitUntil: 'load' });
  await page.waitForSelector('a:has-text("Cloud")', { timeout: 5000 });

  // Track requests so we can assert the right discovery path was hit.
  const reqs = [];
  page.on('request', r => reqs.push(r.url()));

  await page.click('a:has-text("Cloud")');
  // Give the wasm a moment to boot + the cloud SPA to render.
  await page.waitForTimeout(2000);

  const sawMfeEntry = reqs.some(u => u.endsWith('/api/cloud/__mfe_entry'));
  if (!sawMfeEntry) {
    throw new Error('expected GET /api/cloud/__mfe_entry — MFE loader did not handshake');
  }

  const sawAssets = reqs.some(u => u.includes('/assets/cloud/'));
  if (!sawAssets) {
    throw new Error('expected GET /assets/cloud/* — cloud SPA assets never loaded');
  }

  // The cloud-mfe-root div should now have rendered cloud-ui content.
  const mounted = await page.evaluate(() => {
    const root = document.getElementById('cloud-mfe-root');
    return !!root && root.innerHTML.length > 100;
  });
  if (!mounted) {
    throw new Error('cloud-mfe-root has no content — cloud SPA did not mount');
  }

  // URL is still the host's hash route, not the launch-time-rewrite
  // bug shape `/assets/cloud/assets/cloud`.
  if (page.url() !== `${origin}/#cloud`) {
    throw new Error(`expected ${origin}/#cloud, got ${page.url()}`);
  }
}
