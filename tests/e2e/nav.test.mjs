// Hash-route URL stays canonical across click + reload across every
// nav tab. Catches the class of bugs that bit cloud-ui twice now —
// dioxus-web's launch path mutating window.history based on
// Dioxus.toml's base_path.
const TABS = ['Stats', 'Exports', 'Storage', 'Users', 'Cloud', 'Settings', 'Updates'];

export default async function ({ origin, page }) {
  await page.goto(`${origin}/`, { waitUntil: 'load' });
  await page.waitForSelector('a:has-text("Stats")', { timeout: 5000 });

  for (const label of TABS) {
    const slug = label.toLowerCase();
    const expected = `${origin}/#${slug}`;

    await page.click(`a:has-text("${label}")`);
    await page.waitForTimeout(400);
    const afterClick = page.url();
    if (afterClick !== expected) {
      throw new Error(`${label} click: expected ${expected}, got ${afterClick}`);
    }

    await page.reload({ waitUntil: 'load' });
    await page.waitForTimeout(800);
    const afterReload = page.url();
    if (afterReload !== expected) {
      throw new Error(`${label} reload: expected ${expected}, got ${afterReload}`);
    }
  }
}
