// MFE handshake works end-to-end for every installed plugin:
// either via the initial-load auto-activation (default tab) or
// after a NavTab click. Each plugin's `/api/<id>/__mfe_entry`
// fires, the script tag injects, the plugin SPA mounts at
// `<id>-mfe-root`, and the URL settles at `/#<id>`.
const PLUGINS = [
  { id: 'stats', label: 'Stats' },
  { id: 'cloud', label: 'Cloud' },
  { id: 'exports', label: 'Exports' },
  { id: 'storage', label: 'Storage' },
  { id: 'users', label: 'Users' },
  { id: 'dashboard', label: 'Dashboard' },
];

export default async function ({ origin, page }) {
  // Start capturing network requests BEFORE the goto so we see the
  // default-tab auto-activation (`stats` lands first because the
  // host SPA defaults to Page::Plugin("stats") on bare /).
  const reqs = [];
  const onRequest = r => reqs.push(r.url());
  page.on('request', onRequest);

  await page.goto(`${origin}/`, { waitUntil: 'load' });
  await page.waitForSelector('a:has-text("Stats")', { timeout: 5000 });
  await page.waitForTimeout(1500);

  for (const { id, label } of PLUGINS) {
    // Only click if the plugin isn't already the active tab; the
    // default tab auto-activates so its handshake already fired
    // during goto.
    const alreadyActive = await page.evaluate(
      slug => window.location.hash === `#${slug}`,
      id,
    );
    if (!alreadyActive) {
      await page.click(`a:has-text("${label}")`);
      await page.waitForTimeout(2000);
    }

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
  }

  page.off('request', onRequest);
}
