import { read, evaluate, html, certificates, version, createReader, launch } from '../lib/index.js';

const show = async (label, fn) => {
  try { console.log(`${label.padEnd(24)} ${String(await fn()).slice(0, 88)}`); }
  catch (e) { console.log(`${label.padEnd(24)} FAILED: ${String(e.message).slice(0, 70)}`); }
};

await show('version()',        () => version());
await show('certificates()',   async () => (await certificates()).map(c => c.name).join(', '));
await show('read()',           async () => { const r = await read('https://example.com/'); return `${r.title} | ${r.length}ch | ${r.report.total_ms}ms`; });
await show('read() ru',        async () => { const r = await read('https://lenta.ru/'); return `${r.title} | ${r.length}ch`; });
await show('read() gosuslugi', async () => { const r = await read('https://www.gosuslugi.ru/'); return `${r.report.status} | ${r.title}`; });
await show('evaluate()',       () => evaluate('https://example.com/', 'document.querySelectorAll("a").length'));
await show('html()',           async () => (await html('https://example.com/')).length + ' bytes');
// A non-HTML response must be refused, not parsed as text.
await show('non-html refused', async () => {
  try { await read('https://httpbin.org/image/png'); return 'NOT REFUSED'; }
  catch (e) { return 'rejected: ' + e.message.slice(0, 55); }
});

const reader = await createReader({ workers: 2 });
await show('createReader.read',  async () => { const r = await reader.read('https://example.com/'); return `${r.title} via ${reader.url}`; });
await show('createReader x3',    async () => {
  const t = Date.now();
  await Promise.all([1,2,3].map(() => reader.read('https://example.com/')));
  return `${Date.now() - t}ms for 3 concurrent`;
});
await reader.close();

const mar = await launch();
await show('launch().wsEndpoint', () => mar.wsEndpoint);
await show('launch().version',    async () => (await mar.version()).Browser);
await mar.close();
process.exit(0);
