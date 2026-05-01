// Header dropdown structure: items appear in the documented order
// and the new "Updates" / "Settings" items navigate to the right
// host pages (no longer in the main nav).
const EXPECTED_ITEMS = [
  // section 1: theme
  { kind: 'theme' },
  // section 2: config + nav
  { kind: 'item', label: 'Save config' },
  { kind: 'item', label: 'Load config' },
  { kind: 'item', label: 'Updates' },
  { kind: 'item', label: 'Settings' },
  // section 3: destructive / session-end
  { kind: 'item', label: 'Reboot' },
  { kind: 'item', label: 'Sign out' },
];

export default async function ({ origin, page }) {
  await page.goto(`${origin}/`, { waitUntil: 'load' });
  await page.waitForSelector('a:has-text("Stats")', { timeout: 5000 });

  // Settings + Updates must NOT be in the main nav anymore.
  const navHasSettings = await page.locator('nav.app-nav a:has-text("Settings")').count();
  const navHasUpdates = await page.locator('nav.app-nav a:has-text("Updates")').count();
  if (navHasSettings > 0) throw new Error('Settings still in main nav — should be in dropdown');
  if (navHasUpdates > 0) throw new Error('Updates still in main nav — should be in dropdown');

  // Open the menu.
  await page.click('button.user-menu-trigger');
  await page.waitForSelector('.user-menu-popover', { timeout: 2000 });

  // Walk the popover children in DOM order, asserting against EXPECTED_ITEMS.
  const items = await page.evaluate(() => {
    const popover = document.querySelector('.user-menu-popover');
    return Array.from(popover.children).map(el => ({
      cls: el.className,
      label: el.querySelector('span:not(.theme-picker-label)')?.textContent ?? '',
      tag: el.tagName.toLowerCase(),
    }));
  });

  let cursor = 0;
  let sectionDividerCount = 0;
  for (const want of EXPECTED_ITEMS) {
    // Skip dividers in the DOM.
    while (cursor < items.length && items[cursor].cls.includes('user-menu-sep')) {
      cursor++;
      sectionDividerCount++;
    }
    if (cursor >= items.length) {
      throw new Error(`ran out of dropdown items at ${JSON.stringify(want)}`);
    }
    const got = items[cursor];
    if (want.kind === 'theme') {
      if (!got.cls.includes('theme-picker')) {
        throw new Error(`expected theme picker at index ${cursor}, got ${JSON.stringify(got)}`);
      }
    } else if (want.kind === 'item') {
      if (got.label.trim() !== want.label) {
        throw new Error(`expected "${want.label}" at index ${cursor}, got "${got.label}"`);
      }
    }
    cursor++;
  }
  if (sectionDividerCount !== 2) {
    throw new Error(`expected 2 dividers between sections, saw ${sectionDividerCount}`);
  }

  // "Settings" item navigates to /#settings and renders the settings page.
  await page.click('.user-menu-popover button:has-text("Settings")');
  await page.waitForTimeout(400);
  if (page.url() !== `${origin}/#settings`) {
    throw new Error(`expected ${origin}/#settings after clicking Settings, got ${page.url()}`);
  }
  // Settings page no longer has tabs — single-section.
  const settingsTabs = await page.locator('.settings-tabs').count();
  if (settingsTabs > 0) {
    throw new Error('Settings page still renders tabs; expected single section');
  }

  // "Updates" item navigates to /#updates.
  await page.click('button.user-menu-trigger');
  await page.waitForSelector('.user-menu-popover', { timeout: 2000 });
  await page.click('.user-menu-popover button:has-text("Updates")');
  await page.waitForTimeout(400);
  if (page.url() !== `${origin}/#updates`) {
    throw new Error(`expected ${origin}/#updates after clicking Updates, got ${page.url()}`);
  }
}
