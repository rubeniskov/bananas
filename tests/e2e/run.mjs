// End-to-end test entry point: bring up the three daemons via the
// harness, drive them with playwright, surface a non-zero exit on
// any failure.
//
// Adding a new test: write a new `<name>.test.mjs` next to this file
// that exports a default `async function (ctx)` taking
// `{ origin, page, context }`. The dispatcher below picks them up
// automatically — no per-file wiring.
import { chromium } from 'playwright';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import fs from 'node:fs/promises';
import { Harness, ORIGIN } from './harness.mjs';
import { mintCookie } from './cookie.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));

async function main() {
  const harness = new Harness();
  await harness.build();
  await harness.setup();
  await harness.start();

  const cookie = await mintCookie(harness.sessionKeyPath(), 'root', 3600);
  const browser = await chromium.launch();
  const ctx = await browser.newContext();
  await ctx.addCookies([{
    name: 'bananas_session',
    value: cookie,
    domain: '127.0.0.1',
    path: '/',
  }]);
  const page = await ctx.newPage();

  // Pull anything ending in `.test.mjs` and run it. Each test file
  // owns its own assertions; a thrown exception fails the suite.
  const files = (await fs.readdir(HERE))
    .filter(f => f.endsWith('.test.mjs'))
    .sort();

  let failed = 0;
  for (const file of files) {
    const mod = await import(path.join(HERE, file));
    process.stdout.write(`\n=== ${file} ===\n`);
    try {
      await mod.default({ origin: ORIGIN, page, context: ctx });
      console.log(`  ✓ ${file}`);
    } catch (e) {
      console.log(`  ✗ ${file}: ${e.message}`);
      console.log(e.stack);
      failed++;
    }
  }

  await browser.close();
  await harness.teardown();

  if (failed > 0) {
    console.log(`\n${failed} test file(s) failed`);
    process.exit(1);
  }
  console.log(`\n✓ all e2e tests pass (${files.length} file(s))`);
}

main().catch(async e => {
  console.error('[run.mjs]', e);
  process.exit(2);
});
