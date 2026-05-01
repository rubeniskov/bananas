// HTTP surface checks — the public router exposes only / + /assets
// + /api. Plugin SPAs (cloud-ui's index.html) MUST NOT leak.
export default async function ({ origin, page }) {
  // Helper: raw HEAD via fetch instead of page.goto (faster, no nav)
  const head = async (path) => {
    const r = await page.context().request.head(`${origin}${path}`);
    return { status: r.status(), headers: r.headers() };
  };
  const get = async (path) => {
    const r = await page.context().request.get(`${origin}${path}`);
    return {
      status: r.status(),
      headers: r.headers(),
      body: await r.text(),
    };
  };

  // / → host SPA
  let r = await get('/');
  if (r.status !== 200) throw new Error(`/ status=${r.status}`);
  if (!r.body.includes('<title>BanaNAS</title>'))
    throw new Error(`/ missing host <title>`);

  // /cloud/index.html → host SPA fallback (not cloud-ui)
  r = await get('/cloud/index.html');
  if (r.status !== 200) throw new Error(`/cloud/index.html status=${r.status}`);
  if (!r.body.includes('<title>BanaNAS</title>'))
    throw new Error(`/cloud/index.html leaked plugin SPA: title not host's`);

  // /cloud/cloud → host SPA fallback (the original bug surface)
  r = await get('/cloud/cloud');
  if (r.status !== 200) throw new Error(`/cloud/cloud status=${r.status}`);
  if (!r.body.includes('<title>BanaNAS</title>'))
    throw new Error(`/cloud/cloud leaked plugin SPA`);

  // /assets/cloud/index.html → 404 (cloud daemon refuses)
  r = await head('/assets/cloud/index.html');
  if (r.status !== 404) throw new Error(`/assets/cloud/index.html status=${r.status}, want 404`);
}
