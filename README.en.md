# mini-agent-reader

[![Release](https://img.shields.io/github/v/release/Blynskyniki/mini-agent-reader?label=release&sort=semver)](https://github.com/Blynskyniki/mini-agent-reader/releases/latest)
[![License: MIT](https://img.shields.io/github/license/Blynskyniki/mini-agent-reader)](LICENSE)
[![Русский](https://img.shields.io/badge/язык-Русский-informational)](README.md)

*[Читать по-русски](README.md)*

A headless browser that runs JavaScript but never draws anything, plus a reader
that turns the settled page into Markdown.

It exists because agents pay Chromium's full cost — layout, style cascade,
compositing, GPU process — to obtain text. None of that machinery affects the
text. Removing it leaves a 5.3 MB browser that renders a client-side page in
8 ms and 6 MB of memory. The same page through `chrome-headless-shell` on the
same machine: 118 ms and 66 MB. Every figure is measured; the harness is in
`bench/`.

```
$ mar read https://example.com/spa-page
# Deploying without downtime

Rolling deploys work by never taking the whole fleet out of rotation at once…
```

## What it costs

Every figure below is measured, not estimated. The harness is in `bench/`: the
same local pages are served to both engines, each gets a fresh process per page,
time runs from spawn to exit, and peak memory comes from the kernel through
`/usr/bin/time` rather than from either program's own accounting. Median of five
runs, first discarded as a warm-up.

The comparison is against `chrome-headless-shell` 150, which is Chromium's
lightest and fastest path: no interface, no GPU process. Full Chromium through
Puppeteer costs more.

```bash
python3 bench/fixtures.py 8760 &     # the local pages
python3 bench/run.py                 # the comparison
```

### One page, one process

M-series Mac, release build. `mar fetch` and `chrome --dump-dom` do the same
thing: run the scripts and print the resulting DOM.

| Page | mar | Chromium | ratio |
|---|---|---|---|
| Article, 15 KB | **8 ms / 6 MB** | 121 ms / 66 MB | 15× time, 11× memory |
| Client-rendered with `fetch` | **8 ms / 6 MB** | 118 ms / 66 MB | 15× / 11× |
| Work spread over 3 s | **7 ms / 5 MB** | 119 ms / 66 MB | 17× / 13× |
| Large page, 145 KB | **8 ms / 6 MB** | 139 ms / 70 MB | 17× / 12× |

The harness checks that both engines did the same work: it looks in each
engine's output for a marker that only appears once the scripts have run. For
the client-rendered case that is text arriving from `fetch`; for the deferred
page it is the sixth chunk, appended three seconds in. Both engines find both
markers, so the timings compare equal amounts of work rather than two different
ones.

### Startup and size on disk

| | mar | chrome-headless-shell |
|---|---|---|
| Cold start | **3.2 ms / 2.2 MB** | 9.4 ms / 5.8 MB |
| On disk | **5.3 MB** | 193 MB |

The 36× disk difference has practical consequences: container image size,
deployment time, CI cache volume.

### As a service

The figures above include process startup on every page. A service starts once,
and the picture changes:

```
60 client-rendered pages on 4 workers in 68 ms  =  888 pages/sec, 1.1 ms each
```

### Real sites

Here the wall time includes the network and says more about the site than the
engine. Memory does not: it is set by document size and how much script the page
runs.

| Site | total | render | memory | extracted | scripts |
|---|---|---|---|---|---|
| example.com | 268 ms | 10 ms | 6.9 MB | 125 chars | 0 |
| lenta.ru | 213 ms | 9 ms | 9.1 MB | 15,982 | 10 |
| ria.ru | 178 ms | 19 ms | 8.9 MB | 8,317 | 20 |
| habr.com | 465 ms | 12 ms | 10.4 MB | 1,651 | 7 |
| gosuslugi.ru | 36 s | 36 s | 15.5 MB | 0 | 1 |
| docs.python.org | 1.0 s | 640 ms | 8.6 MB | 12,316 | 12 |
| Wikipedia, long article | 572 ms | 80 ms | 27.7 MB | 91,550 | 4 |
| MDN | 1.6 s | 1.3 s | 12.4 MB | 2,002 | 5 |

The gosuslugi.ru row is shown deliberately, and it is not what it looks like.
The site does not serve the portal at all. It serves a 9 KB interstitial whose
one script computes a CRC32 proof of work, sets a cookie and reloads, and only
then is the real page served. That is where the 36 seconds go: they are the
proof of work, and this is the one page in the table where an interpreter
without a JIT is genuinely the wrong tool.

It does get through — the row above is the portal, not the interstitial — but
the default 15-second budget is not enough, so this one needs
`--timeout-ms 60000`. Nothing is extracted even then, for a reason further down
this page: the portal's body arrives as cross-origin ES modules, and neither of
those is fetched.

Wikipedia is the memory ceiling: a document over a megabyte costs 27.7 MB, still
half what Chromium spends on an empty tab.

### Where the difference comes from

Three sources, in order of contribution.

**No renderer.** No layout, no style cascade, no paint, no compositor, no GPU
process. That is most of Chromium's memory and time, and none of it affects the
text.

**A virtual clock.** A page spreading its work over three seconds of timers
settles in microseconds, because time only advances when nothing else is
runnable. Chromium has a comparable mechanism, `--virtual-time-budget`, and it
is enabled in the comparison above — without it the gap on that row would be
400×, not 17×.

**An arena DOM.** Nodes live in a flat `Vec` addressed by a 32-bit index, with
no per-node reference counting. Dropping a document is one walk rather than a
cascade of counters.

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

In practice this covers five or more major sites that do not open at all
without the bundled roots. Verified by comparing `--trust public-only` against
the default: `sberbank.ru`, `vtb.ru`, `alfabank.ru`, `mkb.ru`, `rzd.ru`.

```bash
mar certs                    # what is bundled, and until when
mar read https://www.gosuslugi.ru/
mar read https://sberbank.ru/
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

**No renderer.** There is no layout, no style cascade, no paint. Every box the
page itself measures is zero-sized at the origin, and `getComputedStyle` returns
what the inline style says. This is the whole saving, and it is why screenshots
are out of scope.

`IntersectionObserver` does deliver one record per observed target rather than
never firing: a script that waits for the callback before rendering its content
cannot survive silence, and the answer it gets — visible — is the one that
unblocks it. A CDP client that clicks is a separate matter: it measures an
element before clicking, so it is handed a synthetic rectangle from an imaginary
grid, unique per element and enough to hit-test against. Those rectangles exist
only while a client is asking. The page's own scripts still measure zeroes,
which is what keeps this a browser without a layout engine rather than one with
a bad one.

**Thin native surface, thick JavaScript prelude.** Rust binds only what needs
the document, the page state or the network: about thirty functions. `Event`,
`classList`, `style`, `fetch`, `XMLHttpRequest`, `URL`, `localStorage` and the
rest are written in JavaScript on top of that. Closing a gap for a site that
does not render usually means editing `prelude.js`, not the engine.

**Modules are the host's job too.** An `import` is resolved against the
importing module's URL and fetched through the same seam as every other
subresource, on the same budget. QuickJS asks for it synchronously, part-way
through the module that imported it, which is exactly what a blocking provider
and a virtual clock are for. A module's body runs inside a promise, so a throw
on its first line is watched for rather than lost — otherwise an application
that died immediately looks like one that rendered nothing on purpose.

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

Take something other than the HTML — the visible text, the links, every
subresource the page refers to, or the response body with nothing done to it:

```bash
mar fetch https://example.com/ --dump text
mar fetch https://example.com/ --dump links
mar fetch https://example.com/ --dump assets     # one JSON object per line
mar fetch https://example.com/logo.png --dump original -o logo.png
```

`assets` is read off the settled document rather than off the requests that were
made, because most of what a page refers to is images, fonts and stylesheets
this browser never fetches, and what it *would* pull in is the interesting part.
`original` parses nothing, which makes it the only mode that works on a URL that
is not a page.

Read many pages at once. The per-page cost of starting a process disappears
here: the engine starts once and the workers share its connections and cookies.

```bash
mar scrape https://a.example/ https://b.example/ --concurrency 8
cat urls.txt | mar scrape - --shape eval --eval "document.title"
```

One JSON object per line, emitted as pages finish rather than in the order
given, so a slow page never holds up the ones behind it. A page that failed is a
line carrying an `error`, not a missing line: diffing the input against the
output should account for every URL.

## Model Context Protocol

```bash
mar mcp
```

Speaks MCP over stdin and stdout, so an agent can read pages without going
through a shell. Five tools: `read`, `fetch_html`, `evaluate`, `links` and
`metadata`. There is no screenshot tool and no PDF tool, and each description
says so, which stops a model reaching for one.

```json
{
  "mcpServers": {
    "mar": { "command": "mar", "args": ["mcp"] }
  }
}
```

The global safety flags are fixed before the first message is read: a client can
narrow what a page costs, and cannot widen what it is allowed to reach.

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

Requests can be intercepted, which is the point at which this stops being a
reader and starts being usable for tests: block the third-party bundle, serve a
fixture instead of the API, fail one request and see what the page does.

```js
await page.setRequestInterception(true);
page.on('request', (r) =>
  r.url().includes('/analytics') ? r.abort()
  : r.url().includes('/api/') ? r.respond({ status: 200, body: fixture })
  : r.continue());
```

The document request is intercepted too, so mocking the page itself works. It
sits at the seam where the engine hands a request to whatever the host installed
— the engine never opens a socket — and a pause that is never answered ends at
the page's existing wall-clock budget rather than hanging it.

`page.click()` and `page.type()` work, which needed more than dispatching an
event: Puppeteer measures an element and clicks a coordinate, and with no layout
every centre is the same point. Elements are handed synthetic rectangles from an
imaginary grid while a client is measuring, and the click is hit-tested against
those. `Network.getResponseBody`, `Storage.getCookies` and `Storage.setCookies`
are there as well.

Response-stage interception (`Fetch.continueResponse`, `continueWithAuth`) is
not, and returns a clear error rather than silently never pausing. So does a
method that needs the renderer: `Page.captureScreenshot` and `Page.printToPDF`
say what is missing instead of handing back a blank image.

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
- one cookie jar shared between the document and its subresources, which
  `document.cookie` reads from and writes to like a browser's.

That last one is what gets past a JavaScript challenge. A page that computes
something, sets a cookie and calls `location.reload()` is served the real thing
on the second request — which also means a repeat of a URL counts as a loop only
when the cookies are unchanged too, because otherwise it is a different request.

What is missing: TLS fingerprint spoofing. The handshake is rustls, and its JA3
and JA4 differ from Chrome's. Sites that check exactly that will see the
difference. If you need parity, put a fingerprint-impersonating proxy in front
and point `--proxy` at it:

```bash
mar --proxy socks5://127.0.0.1:1080 read https://example.com/
```

A proxy that will not parse fails every request rather than quietly going
direct: silently falling back is how a scrape leaks the address it was trying
not to use.

## Safety

Requests to loopback, private, link-local and other reserved ranges are refused
by default, `localhost` included, and only `http` and `https` are allowed. A
rendering service fetches URLs its caller chose and then runs scripts that
choose more; without this, `http://169.254.169.254/` reads cloud credentials on
the caller's behalf. Pass `--allow-private` for local development, and
`--allow-host` to restrict fetching to a set of domains.

Scripts cannot navigate the host anywhere by themselves. A `location.href`
assignment or a `location.reload()` is recorded and followed only because a
browser would, bounded to three hops with loop detection, and cross-origin
subresource requests are refused.

`--obey-robots` asks each site's `robots.txt` before fetching a page from it,
once per host. Documents only: `robots.txt` governs what a crawler may go and
read, not which stylesheet a page it was already allowed to read pulls in.

## Limits

Known and deliberate:

- **No screenshots or PDFs.** They need the renderer this project removes.
- **No TLS fingerprint spoofing.** Headers look like Chrome; the handshake
  does not. `--proxy` is the way around it.
- **No import maps.** A bare specifier (`import x from 'lodash'`) has nothing
  to resolve against and is reported as such rather than guessed at.
- **No WebSocket, Worker, WebGL, canvas or media.** They construct without
  throwing and do nothing.
- **No layout for the page itself.** An element measures zero, and a page that
  branches on its own geometry takes the zero branch.
- **Cross-origin classic scripts are not fetched.** Third-party bundles are
  almost always analytics and advertising: slow, and they add nothing to read.
  Cross-origin *modules* are fetched, because an application shipping native
  modules keeps its bundle on a CDN and the same rule would skip the
  application along with the tag manager.
- **Non-HTML is not parsed.** A PDF or an image is refused with a clear
  message rather than served as text.
- **Extraction is heuristic.** A page with no clear article is reported with
  `low_confidence: true` rather than guessed at.

## Testing

```bash
cargo test
```

Ninety-five tests: DOM structure and serialization round-trips, CSS selector
semantics, charset sniffing in the spec's precedence order (including a
regression for double-decoded koi8-r), SSRF policy, `robots.txt` grouping and
longest-match precedence, the cookie jar the page shares with the client,
article extraction, a CDP session driven end to end including request
interception and synthetic input, the MCP protocol framing, and end-to-end
rendering with promises, `fetch`, the virtual clock, error isolation and the
runaway-page budget. None touch the network.

## Licence

MIT.
