# mini-agent-reader

[![Release](https://img.shields.io/github/v/release/Blynskyniki/mini-agent-reader?label=release&sort=semver)](https://github.com/Blynskyniki/mini-agent-reader/releases/latest)
[![License: MIT](https://img.shields.io/github/license/Blynskyniki/mini-agent-reader)](LICENSE)
[![Русский](https://img.shields.io/badge/язык-Русский-informational)](README.md)

*[Читать по-русски](README.md)*

A headless browser that runs JavaScript but never draws anything, plus a reader
that turns the settled page into Markdown.

It exists because agents pay Chromium's full cost — layout, style cascade,
compositing, GPU process, ~300 MB of resident memory — to obtain text. None of
that machinery affects the text. Removing it leaves a browser that fits in a
5 MB binary and renders a client-side page in single-digit milliseconds.

```
$ mar read https://example.com/spa-page
# Deploying without downtime

Rolling deploys work by never taking the whole fleet out of rotation at once…
```

## What it costs

Measured on an M-series Mac, release build, one page per process.

| | mini-agent-reader | Headless Chromium |
|---|---|---|
| Binary | 5 MB | ~150 MB installed |
| Peak RSS, typical page | 6–8 MB | 250–400 MB |
| Peak RSS, large page (Wikipedia) | 19 MB | 400 MB+ |
| Render time, local SPA | 4 ms | 300–800 ms |
| Cold start | ~2 ms | 150–400 ms |

The virtual clock is the reason for the render figure. A page that spreads its
work over three seconds of `setTimeout` settles in microseconds, because time
only advances when nothing else is runnable.

## Russian sites out of the box

`gosuslugi.ru`, `mos.ru` and others present certificates issued by the Russian
Ministry of Digital Development. No browser or operating system ships that root,
so an ordinary client cannot open those sites at all: the connection fails
during the handshake, before any HTTP is exchanged.

Both certificates are bundled into the binary — but not into the default trust
set. A connection is verified against the public roots first, and only if that
fails is it retried against the extended set. An authority run by a government
can issue a certificate for any domain, so trusting one unconditionally would
let it intercept every site. This ordering means the bundled root can rescue a
site that would otherwise fail, and can never override one that already works.

```bash
mar certs                    # what is bundled, and until when
mar read https://www.gosuslugi.ru/
```

Change the order with `--trust public-only | combined | none`, and add your own
roots with `--ca-bundle <file>`.

Legacy encodings are handled too: `windows-1251` and `koi8-r` are detected in
the order the HTML spec prescribes — byte order mark, header, `<meta>`, then the
bytes themselves.

## What it is

Six crates, each usable on its own.

- **`mar-dom`** — HTML parsing and an arena DOM. Nodes live in a flat `Vec`
  addressed by a 32-bit index, with no per-node reference counting. CSS matching
  is delegated to Servo's `selectors`, so `:has`, `:is` and `:nth-child` work.
- **`mar-js`** — QuickJS with DOM bindings, a virtual-clock event loop, and a
  JavaScript prelude that builds the browser environment.
- **`mar-net`** — HTTP with browser-shaped headers, spec-order charset sniffing,
  bundled root certificates, and a policy layer that blocks private address
  space.
- **`mar-extract`** — Readability-style article detection, metadata, Markdown.
- **`mar-cdp`** — a Chrome DevTools Protocol endpoint, so Puppeteer, Playwright
  and chrome-remote-interface work unchanged.
- **`mar-cli`** — the `mar` binary, the HTTP server and the CDP server.

## Design

**No renderer.** There is no layout, no style cascade, no paint. Every box is
zero-sized at the origin, `getComputedStyle` returns what the inline style says,
and `IntersectionObserver` never fires. This is the whole saving, and it is why
screenshots are out of scope.

**Thin native surface, thick JavaScript prelude.** Rust binds only what needs
the document, the page state or the network: about thirty functions. `Event`,
`classList`, `style`, `fetch`, `XMLHttpRequest`, `URL`, `localStorage` and the
rest are written in JavaScript on top of that. Closing a gap for a site that
does not render usually means editing `prelude.js`, not the engine.

**A virtual clock.** Timers sit in a heap keyed by due time. When microtasks are
drained and no network call is outstanding, the clock jumps to the next timer.
Wall-clock time is never spent waiting. Timers past a horizon never fire, which
stops a polling page from staying alive forever.

**The engine never opens a socket.** It calls a `NetworkProvider` the host
installs, so policy lives in one place and tests run offline against canned
responses.

**Everything is bounded.** JS heap ceiling, stack ceiling, wall-clock budget,
virtual-time horizon, timer-callback budget, subresource-request budget, console
byte budget. A page that misbehaves is truncated and reported as truncated.

## Install

Prebuilt binaries for Linux (x86_64, aarch64), macOS (Intel, Apple Silicon) and
Windows are attached to every
[release](https://github.com/Blynskyniki/mini-agent-reader/releases/latest).
Download the archive for your platform, unpack it, and run `mar`.

From npm:

```bash
npm install mini-agent-reader
```

From source:

```bash
cargo build --release    # ~30s, no system dependencies beyond a C compiler
```

## Command line

Read a page as Markdown:

```bash
mar read https://example.com/article
```

With metadata as YAML front matter, or as one JSON object:

```bash
mar read https://example.com/article --format full
mar read https://example.com/article --format json
```

Skip JavaScript when the page is server-rendered, which is much faster:

```bash
mar read https://example.com/article --no-js
```

Get the rendered HTML, with a cost report on stderr:

```bash
mar fetch https://example.com/ --report
```

Ask the settled page a question:

```bash
mar eval https://example.com/ "[...document.querySelectorAll('h2')].map(h => h.textContent)"
```

## HTTP server

```bash
mar serve --bind 127.0.0.1:3000 --workers 4
```

```
POST /read   {"url": "...", "max_chars": 4000, "images": false}
POST /html   {"url": "..."}
POST /eval   {"url": "...", "expression": "..."}
GET  /health
```

Each worker renders one page at a time. Twelve concurrent requests against a
local page complete in about 40 ms on three workers.

## Chrome DevTools Protocol

```bash
mar cdp --bind 127.0.0.1:9222
```

Existing Puppeteer or Playwright code points here unchanged:

```js
import puppeteer from 'puppeteer-core';

const browser = await puppeteer.connect({
  browserWSEndpoint: 'ws://127.0.0.1:9222',
});
const page = await browser.newPage();
await page.goto('https://lenta.ru/');

await page.title();
await page.evaluate(() => document.querySelectorAll('a').length);
await page.$$eval('h2', els => els.map(e => e.textContent));
await page.content();
```

Verified against unmodified `puppeteer-core`: `connect`, `newPage`, `goto`,
`title`, `evaluate` with arguments, `$`, `$$`, `$eval`, `$$eval`,
`elementHandle.evaluate`, `content` and `close` all work. That needed real
remote object handles, `Runtime.getProperties`, and honouring `awaitPromise`, so
asynchronous code inside `evaluate` genuinely waits for its result.

A method that needs the renderer (`Page.captureScreenshot`, `Page.printToPDF`)
returns a clear error rather than a blank image.

## JavaScript

```js
import { read, evaluate, createReader, launch } from 'mini-agent-reader';

// One page, one process.
const article = await read('https://lenta.ru/');
console.log(article.title, article.length, article.report.total_ms);
console.log(article.content);          // Markdown

// Ask the page a question.
const count = await evaluate('https://example.com/', 'document.links.length');

// A long-running server: one startup, reused connections and cookies.
const reader = await createReader({ workers: 4 });
const pages = await Promise.all(urls.map(u => reader.read(u)));
await reader.close();

// A CDP endpoint for Puppeteer.
const mar = await launch();
const browser = await puppeteer.connect({ browserWSEndpoint: mar.wsEndpoint });
```

TypeScript types ship with the package.

## Looking like a browser

Requests are browser-shaped at the HTTP level, and that includes the ones the
page itself makes through `fetch` or `XMLHttpRequest`:

- the real page URL as `Referer`, not just its origin;
- `Origin` on script-initiated requests;
- a correct `Sec-Fetch-Site`: `same-origin`, `same-site` or `cross-site`;
- `Sec-Fetch-Dest` and `Sec-Fetch-Mode` matching the request type;
- the full `Sec-CH-UA`, `Accept` and `Accept-Language` set;
- no `Content-Length` on a bodyless GET, which browsers never send;
- one cookie jar shared between the document and its subresources.

What is missing: TLS fingerprint spoofing. The handshake is rustls, and its JA3
and JA4 differ from Chrome's. Sites that check exactly that will see the
difference. If you need parity, put a curl-impersonate proxy in front of `mar`.

## Safety

Requests to loopback, private, link-local and other reserved ranges are refused
by default, `localhost` included, and only `http` and `https` are allowed. A
rendering service fetches URLs its caller chose and then runs scripts that
choose more; without this, `http://169.254.169.254/` reads cloud credentials on
the caller's behalf. Pass `--allow-private` for local development, and
`--allow-host` to restrict fetching to a set of domains.

Scripts cannot navigate the host anywhere by themselves. A `location.href`
assignment is recorded and followed only because a browser would, bounded to
three hops with loop detection, and cross-origin subresource requests are
refused.

## Limits

Known and deliberate:

- **No screenshots or PDFs.** They need the renderer this project removes.
- **No TLS fingerprint spoofing.** Headers look like Chrome; the handshake
  does not.
- **No module loader.** A `type="module"` script compiles and runs, but an
  `import` of another URL fails. Page runtimes that bundle this way log an
  error and the rest of the page still renders.
- **No WebSocket, Worker, WebGL, canvas or media.** They construct without
  throwing and do nothing.
- **Cross-origin scripts are not fetched.** Third-party bundles are almost
  always analytics and advertising: slow, and they add nothing to read.
- **Non-HTML is not parsed.** A PDF or an image is refused with a clear
  message rather than served as text.
- **Extraction is heuristic.** A page with no clear article is reported with
  `low_confidence: true` rather than guessed at.

## Testing

```bash
cargo test
```

Forty-five tests: DOM structure and serialization round-trips, CSS selector
semantics, charset sniffing in the spec's precedence order (including a
regression for double-decoded koi8-r), SSRF policy, article extraction, and
end-to-end rendering with promises, `fetch`, the virtual clock, error isolation
and the runaway-page budget. None touch the network.

## Licence

MIT.
