// Drive the CDP endpoint with an unmodified puppeteer-core and fail loudly if
// any part of the usual flow stops working. Runs against a page served from
// this checkout, so the check never depends on the public internet.
import puppeteer from 'puppeteer-core';
import { createServer } from 'node:http';

const PAGE = `<!doctype html><html><head><title>Loading</title></head><body>
<div id="root"></div>
<script>
  const items = [{n: 'alpha'}, {n: 'beta'}, {n: 'gamma'}];
  document.getElementById('root').innerHTML =
    '<h1>Rendered</h1>' + items.map(i => '<p class="item">' + i.n + '</p>').join('');
  document.title = 'Rendered (' + items.length + ')';
  setTimeout(() => {
    const late = document.createElement('p');
    late.id = 'late';
    late.textContent = 'appended after two seconds';
    document.body.appendChild(late);
  }, 2000);
</script></body></html>`;

const server = createServer((_, res) => {
  res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
  res.end(PAGE);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const origin = `http://127.0.0.1:${server.address().port}/`;

const failures = [];
const expect = (label, actual, wanted) => {
  const ok = JSON.stringify(actual) === JSON.stringify(wanted);
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${label.padEnd(28)} ${JSON.stringify(actual)}`);
  if (!ok) failures.push(`${label}: expected ${JSON.stringify(wanted)}, got ${JSON.stringify(actual)}`);
};

const browser = await puppeteer.connect({
  browserWSEndpoint: 'ws://127.0.0.1:9222',
  protocolTimeout: 30_000,
});
const page = await browser.newPage();
page.setDefaultTimeout(30_000);
page.setDefaultNavigationTimeout(30_000);

await page.goto(origin);

expect('page.url()', page.url(), origin);
expect('page.title()', await page.title(), 'Rendered (3)');
expect('evaluate', await page.evaluate(() => document.querySelector('h1').textContent), 'Rendered');
expect('evaluate with arguments', await page.evaluate((a, b) => a + b, 20, 22), 42);
expect('$eval', await page.$eval('h1', (el) => el.tagName), 'H1');
expect('$$eval', await page.$$eval('.item', (els) => els.map((e) => e.textContent)),
  ['alpha', 'beta', 'gamma']);
expect('$$ length', (await page.$$('.item')).length, 3);

const handle = await page.$('#root');
expect('elementHandle.evaluate', await handle.evaluate((el) => el.children.length), 4);

// The virtual clock must have run the deferred append before the page settled.
expect('deferred content', await page.evaluate(() => !!document.getElementById('late')), true);
expect('content() is html', (await page.content()).includes('<h1>Rendered</h1>'), true);

await page.close();
await browser.disconnect();
server.close();

if (failures.length) {
  console.error(`\n${failures.length} check(s) failed:`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log('\nAll CDP checks passed.');
process.exit(0);
