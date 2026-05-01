// MFE handshake works end-to-end for every installed plugin:
// click the nav tab → `/api/<id>/__mfe_entry` fires → script tag
// injected → plugin SPA mounts inline at `<id>-mfe-root`.
const PLUGINS = [
  { id: 'cloud', label: 'Cloud' },
  { id: 'exports', label: 'Exports' },
  { id: 'storage', label: 'Storage' },
];

export default async function ({ origin, page }) {
  await page.goto(`${origin}/`, { waitUntil: 'load' });
  await page.waitForSelector('a:has-text("Stats")', { timeout: 5000 });

  for (const { id, label } of PLUGINS) {
    const reqs = [];
    const onRequest = r => reqs.push(r.url());
    page.on('request', onRequest);

    await page.click(`a:has-text("${label}")`);
    // Give the wasm a moment to boot + the plugin SPA to render.
    await page.waitForTimeout(2000);

    const sawMfeEntry = reqs.some(u => u.endsWith(`/api/${id}/__mfe_entry`));
    if (!sawMfeEntry) {
      throw new Error(`${label}: expected GET /api/${id}/__mfe_entry — MFE loader did not handshake`);
    }

    const sawAssets = reqs.some(u => u.includes(`/assets/${id}/`));
    if (!sawAssets) {
      throw new Error(`${label}: expected GET /assets/${id}/* — plugin SPA assets never loaded`);
    }

    const mountId = `${id}-mfe-root`;
    const mounted = await page.evaluate(mid => {
      const root = document.getElementById(mid);
      return !!root && root.innerHTML.length > 100;
    }, mountId);
    if (!mounted) {
      throw new Error(`${label}: ${mountId} has no content — plugin SPA did not mount`);
    }

    if (page.url() !== `${origin}/#${id}`) {
      throw new Error(`${label}: expected ${origin}/#${id}, got ${page.url()}`);
    }

    page.off('request', onRequest);
  }
}
