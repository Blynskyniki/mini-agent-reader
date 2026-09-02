// A JavaScript wrapper around the `mar` binary.
//
// Two ways to use it, because they suit different shapes of work:
//
//   read(url)         — one page, one process. Simple and stateless.
//   createReader()    — a long-running server. Amortises startup across pages
//                       and keeps one connection pool and cookie jar.
//   launch()          — a CDP endpoint, so Puppeteer or Playwright can drive it.

import { spawn, execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { once } from 'node:events';

const here = dirname(fileURLToPath(import.meta.url));

/** Where the `mar` binary lives. */
export function binaryPath() {
  if (process.env.MAR_BINARY) return process.env.MAR_BINARY;
  const name = process.platform === 'win32' ? 'mar.exe' : 'mar';
  for (const candidate of [
    join(here, '..', 'bin', name),
    join(here, '..', '..', 'target', 'release', name),
    join(here, '..', '..', 'target', 'debug', name),
  ]) {
    if (existsSync(candidate)) return candidate;
  }
  // Fall back to PATH, so a system install works with no configuration.
  return name;
}

class MarError extends Error {
  constructor(message, { stderr, code } = {}) {
    super(message);
    this.name = 'MarError';
    this.stderr = stderr;
    this.exitCode = code;
  }
}
export { MarError };

function globalFlags(options = {}) {
  const flags = [];
  if (options.allowPrivate) flags.push('--allow-private');
  for (const host of options.allowHosts ?? []) flags.push('--allow-host', host);
  if (options.trust) flags.push('--trust', options.trust);
  for (const bundle of options.caBundles ?? []) flags.push('--ca-bundle', bundle);
  return flags;
}

function renderFlags(options = {}) {
  const flags = [];
  if (options.javascript === false) flags.push('--no-js');
  if (options.externalScripts === false) flags.push('--no-external-scripts');
  if (options.timeoutMs != null) flags.push('--timeout-ms', String(options.timeoutMs));
  if (options.horizonMs != null) flags.push('--horizon-ms', String(options.horizonMs));
  if (options.memoryMb != null) flags.push('--memory-mb', String(options.memoryMb));
  return flags;
}

/** Run `mar` once and collect its output. */
function run(args, { timeoutMs = 60_000 } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(binaryPath(), args, { stdio: ['ignore', 'pipe', 'pipe'] });
    const out = [];
    const err = [];
    let settled = false;

    const timer = setTimeout(() => {
      settled = true;
      child.kill('SIGKILL');
      reject(new MarError(`mar timed out after ${timeoutMs}ms`));
    }, timeoutMs);

    child.stdout.on('data', (c) => out.push(c));
    child.stderr.on('data', (c) => err.push(c));
    child.on('error', (e) => {
      clearTimeout(timer);
      if (settled) return;
      settled = true;
      reject(
        e.code === 'ENOENT'
          ? new MarError(
              `the "mar" binary was not found. Install it, or set MAR_BINARY to its path.`,
            )
          : e,
      );
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      if (settled) return;
      settled = true;
      const stderr = Buffer.concat(err).toString();
      if (code !== 0) {
        reject(new MarError(stderr.trim() || `mar exited with code ${code}`, { stderr, code }));
        return;
      }
      resolve(Buffer.concat(out).toString());
    });
  });
}

/**
 * Read one page: run its scripts, find the article, return Markdown plus
 * metadata and a cost report.
 */
export async function read(url, options = {}) {
  const args = [
    ...globalFlags(options),
    'read', url,
    '--format', 'json',
    ...renderFlags(options),
  ];
  if (options.maxChars != null) args.push('--max-chars', String(options.maxChars));
  if (options.images === false) args.push('--no-images');
  if (options.links === false) args.push('--no-links');

  const raw = await run(args, options);
  const { reading, report } = JSON.parse(raw);
  return { ...reading, report };
}

/** Fetch a page and return the HTML after its scripts have run. */
export async function html(url, options = {}) {
  return run([...globalFlags(options), 'fetch', url, ...renderFlags(options)], options);
}

/** Render a page, then evaluate an expression in it and return the value. */
export async function evaluate(url, expression, options = {}) {
  const raw = await run(
    [...globalFlags(options), 'eval', url, expression, ...renderFlags(options)],
    options,
  );
  return JSON.parse(raw);
}

/** The root certificates bundled into the binary. */
export async function certificates(options = {}) {
  return JSON.parse(await run(['certs', '--json'], options));
}

/** The binary's version string. */
export function version() {
  try {
    return execFileSync(binaryPath(), ['--version'], { encoding: 'utf8' }).trim();
  } catch {
    return null;
  }
}

/**
 * Wait for a server to start listening, by polling its health endpoint.
 *
 * The process prints its address to stderr, but parsing log output is brittle;
 * asking the server whether it is up is not.
 */
async function waitForPort(url, child, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new MarError(`mar exited with code ${child.exitCode} before it started listening`);
    }
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(1000) });
      if (response.ok) return;
    } catch (e) {
      lastError = e;
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new MarError(`mar did not start listening within ${timeoutMs}ms: ${lastError ?? ''}`);
}

function spawnServer(args, options) {
  const child = spawn(binaryPath(), args, {
    stdio: ['ignore', 'inherit', options.verbose ? 'inherit' : 'ignore'],
  });
  child.unref?.();
  return child;
}

/**
 * Start a long-running reader and talk to it over HTTP.
 *
 * Worth it above a handful of pages: the process starts once, and connections
 * and cookies are reused across requests.
 */
export async function createReader(options = {}) {
  const port = options.port ?? 0;
  const host = options.host ?? '127.0.0.1';
  // Port 0 asks the OS to choose, but then the caller cannot address it, so a
  // concrete default is used unless one was given.
  const boundPort = port === 0 ? 3000 + Math.floor(Math.random() * 20_000) : port;
  const bind = `${host}:${boundPort}`;
  const token = options.token ?? null;

  const args = [
    ...globalFlags(options),
    'serve', '--bind', bind,
    '--workers', String(options.workers ?? 4),
  ];
  if (token) args.push('--token', token);

  const child = spawnServer(args, options);
  const base = `http://${bind}`;
  await waitForPort(`${base}/health`, child, options.startTimeoutMs ?? 10_000);

  const headers = { 'Content-Type': 'application/json' };
  if (token) headers.Authorization = `Bearer ${token}`;

  const post = async (path, body) => {
    const response = await fetch(`${base}${path}`, {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
    });
    const payload = await response.json();
    if (!response.ok) throw new MarError(payload.error ?? `HTTP ${response.status}`);
    return payload;
  };

  return {
    url: base,
    async read(url, opts = {}) {
      const { reading, report } = await post('/read', { url, ...toServerOptions(opts) });
      return { ...reading, report };
    },
    async html(url, opts = {}) {
      const { html, report } = await post('/html', { url, ...toServerOptions(opts) });
      return { html, report };
    },
    async evaluate(url, expression, opts = {}) {
      const { result } = await post('/eval', { url, expression, ...toServerOptions(opts) });
      return result;
    },
    async close() {
      if (child.exitCode === null) {
        child.kill('SIGTERM');
        await Promise.race([once(child, 'close'), new Promise((r) => setTimeout(r, 2000))]);
        if (child.exitCode === null) child.kill('SIGKILL');
      }
    },
  };
}

function toServerOptions(options) {
  const body = {};
  if (options.javascript != null) body.javascript = options.javascript;
  if (options.externalScripts != null) body.external_scripts = options.externalScripts;
  if (options.maxChars != null) body.max_chars = options.maxChars;
  if (options.images != null) body.images = options.images;
  if (options.links != null) body.links = options.links;
  if (options.timeoutMs != null) body.timeout_ms = options.timeoutMs;
  if (options.horizonMs != null) body.horizon_ms = options.horizonMs;
  if (options.console != null) body.console = options.console;
  return body;
}

/**
 * Start a Chrome DevTools Protocol endpoint and return its WebSocket URL.
 *
 * Hand the URL to Puppeteer or Playwright and existing code runs unchanged:
 *
 *   const mar = await launch();
 *   const browser = await puppeteer.connect({ browserWSEndpoint: mar.wsEndpoint });
 */
export async function launch(options = {}) {
  const host = options.host ?? '127.0.0.1';
  const port = options.port ?? 9222 + Math.floor(Math.random() * 1000);
  const bind = `${host}:${port}`;
  const token = options.token ?? null;

  const args = [
    ...globalFlags(options),
    'cdp', '--bind', bind,
    ...renderFlags(options),
  ];
  if (token) args.push('--token', token);
  if (options.maxConnections != null) {
    args.push('--max-connections', String(options.maxConnections));
  }

  const child = spawnServer(args, options);
  const base = `http://${bind}`;
  await waitForPort(`${base}/json/version`, child, options.startTimeoutMs ?? 10_000);

  const wsEndpoint = token ? `ws://${bind}?token=${encodeURIComponent(token)}` : `ws://${bind}`;

  return {
    wsEndpoint,
    httpEndpoint: base,
    process: child,
    async version() {
      return (await fetch(`${base}/json/version`)).json();
    },
    async close() {
      if (child.exitCode === null) {
        child.kill('SIGTERM');
        await Promise.race([once(child, 'close'), new Promise((r) => setTimeout(r, 2000))]);
        if (child.exitCode === null) child.kill('SIGKILL');
      }
    },
  };
}

export default { read, html, evaluate, launch, createReader, certificates, version, binaryPath };
