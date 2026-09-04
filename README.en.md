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
| Article, 15 KB | **9 ms / 6 MB** | 167 ms / 68 MB | 18× time, 11× memory |
| Client-rendered with `fetch` | **10 ms / 7 MB** | 139 ms / 67 MB | 14× / 10× |
| Work spread over 3 s | **9 ms / 7 MB** | 123 ms / 66 MB | 14× / 9× |
| Large page, 145 KB | **10 ms / 7 MB** | 151 ms / 78 MB | 15× / 11× |

The harness checks that both engines did the same work: it looks in each
engine's output for a marker that only appears once the scripts have run. For
the client-rendered case that is text arriving from `fetch`; for the deferred
page it is the sixth chunk, appended three seconds in. Both engines find both
markers, so the timings compare equal amounts of work rather than two different
ones.

### Startup and size on disk

| | mar | chrome-headless-shell |
|---|---|---|
| Cold start | **4.2 ms / 2.3 MB** | 11.9 ms / 5.8 MB |
| On disk | **6.0 MB** (9.6 MB with `browser-tls`) | 193 MB |

The 30× disk difference has practical consequences: container image size,
deployment time, CI cache volume.

### As a service

The figures above include process startup on every page. A service starts once,
and the picture changes:

```
60 client-rendered pages on 4 workers in 93 ms  =  645 pages/sec, 1.6 ms each
```

### Real sites

Here the wall time includes the network and says more about the site than the
engine. So does most of the memory now: the engine runs the page's
application — every chunk it loads, every worker it starts — and a news front
page ships a great deal of both. What is left of the table's earlier, smaller
numbers is the pages that ship little.

| Site | total | render | memory | extracted | scripts |
|---|---|---|---|---|---|
| example.com | 316 ms | 4 ms | 7.7 MB | 125 chars | 0 |
| lenta.ru | 522 ms | 405 ms | 38.1 MB | 14,469 | 14 |
| ria.ru | 928 ms | 637 ms | 29.4 MB | 8,131 | 66 |
| habr.com | 7.6 s | 6.7 s | 67.2 MB | 1,102 | 9 |
| gosuslugi.ru | 3.0 s | 757 ms | 43.0 MB | 0 | 3 |
| docs.python.org | 861 ms | 380 ms | 14.3 MB | 12,316 | 12 |
| Wikipedia, long article | 1.2 s | 732 ms | 46.9 MB | 91,550 | 4 |
| MDN | 7.2 s | 6.9 s | 18.4 MB | 2,002 | 5 |

habr.com is the cost of running an application: its front page pulls forty
chunks from its CDN, one import at a time, and runs them. MDN is the same
story with a search index. Neither yields more text than the server-rendered
markup already held, which is why `mar read` keeps that markup when the
scripts leave less behind — but the scripts still have to run to find out.

The gosuslugi.ru row is shown deliberately, and it is not what it looks like.
The site does not serve the portal at all. It serves a 9 KB interstitial whose
one script computes a CRC32 proof of work, sets a cookie and reloads, and only
then is the real page served. That is where the seconds go, and it is the one
page in the table where an interpreter without a JIT is genuinely the wrong
tool. It does get through inside the default budget now; nothing is extracted
even then, because the portal draws its body from a JavaScript challenge of its
own after the first.

Wikipedia is the memory ceiling for a document: a page over a megabyte costs
47 MB, which is what Chromium spends on an empty tab.

### Coverage

Speed is worth nothing on a page that comes out blank, so there is a second
harness: `bench/corpus.json`, 453 live sites — the top of the Tranco list with
infrastructure, parked and duplicate hosts removed, plus the Russian web and a
set of client-rendered applications — read by `chrome-headless-shell` and by
`mar` in the same session, reduced to visible text the same way, and compared.
A page "reads" at 60% of Chrome's text or more.

| | reads | of what Chrome read |
|---|---|---|
| Chrome | 342 / 453 | — |
| mar, rustls handshake | 267 | 78% |
| mar, `browser-tls` | 276 | 81% |

Most of the rest is a bot check that decides on the browser itself rather than
on the handshake. Runs differ by a few pages either way: one proxy, six pages
at once, and a fifteen-second budget. `python3 bench/corpus.py --chrome`
reproduces the table.

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

**No renderer.** There is no layout and no paint. Nothing is positioned, nothing
wraps, nothing is composited. This is the whole saving, and it is why
screenshots are out of scope.

But a page that measures itself and finds zero concludes it has no room and
renders its collapsed branch, which on real sites is the difference between an
article and an empty shell. So an element reports a box derived from what the
CSS actually says: an explicit width in a stylesheet or an inline style, a
`width` attribute on a picture, otherwise an estimate from the content. The
cascade behind that is real — rules are matched with the same selector engine
the DOM uses — which is more than the inline-attribute shortcut this class of
engine usually takes. The numbers are plausible, not true: nothing here knows
where anything sits.

`IntersectionObserver` delivers one record per observed target rather than never
firing, for the same reason: a script that waits for the callback before
rendering cannot survive silence. A CDP client that clicks is a separate matter
again — it needs boxes that differ per element so a coordinate maps back to what
it hit, so while a client is measuring it gets tiles from an imaginary grid
instead of the page's own numbers. Two rulers, for two different questions.

**Thin native surface, thick JavaScript prelude.** Rust binds only what needs
the document, the page state or the network: about thirty functions. `Event`,
`classList`, `style`, `fetch`, `XMLHttpRequest`, `URL`, `localStorage`, `Intl`,
the element interfaces `instanceof` needs, `MessageChannel`, `TreeWalker` and
the rest are written in JavaScript on top of that — roughly two hundred names.
Closing a gap for a site that does not render usually means editing
`prelude.js`, not the engine.

Arguments cross that surface the way the DOM says they do. `setAttribute('x',
42)` and `el.className = 1` are ordinary code, so a string parameter takes
`ToString(value)` rather than refusing anything that is not already a string.

**The parser is the web's, not the spec's, where the two disagree.** QuickJS
is vendored with one change: `for (f() in o)` parses, runs the call, and throws
a ReferenceError only when there is a key to assign, which is what V8 does.
A bot check wraps `for (f() in [])` in `try/catch` to tell a browser from
another engine, and an engine that refuses to parse it fails the whole script.

**Classic scripts run in sloppy mode**, because that is what a `<script>` is.
Under strict mode an assignment to an undeclared name is a ReferenceError, and
that is not a corner case: React's streaming markup bootstraps itself with bare
`$RC = function(...)` assignments, so a strict engine silently loses every
suspense boundary on every server-rendered React page.

**Modules are the host's job too.** An `import` is resolved against the
importing module's URL — or through the page's `<script type="importmap">`,
which is what gives `import '@wordpress/interactivity'` somewhere to go — and
fetched through the same seam as every other subresource, on the same budget.
QuickJS asks for it synchronously, part-way through the module that imported
it, which is exactly what a blocking provider and a virtual clock are for. A
module's body runs inside a promise, so a throw on its first line is watched
for rather than lost — otherwise an application that died immediately looks
like one that rendered nothing on purpose. `import.meta.url` is set, because
`new URL('.', import.meta.url)` is how a bundle finds its own assets.

**A script the page inserts runs.** Webpack loads every lazy chunk by
appending a `<script src>`, a tag manager loads everything that way, and a page
whose inserted scripts never ran is a page whose application never started. An
inserted script is fetched and run with `document.currentScript` pointing at
it, fires `load` or `error` on its element, and a submitted form is a
navigation — a POST with a body when the form says so, which is how a
single-sign-on redirect chain gets through.

**A node is one object.** The same element is the same JavaScript object every
time the page reaches it, so a `WeakMap` keyed by element, an expando such as
`el.__reactFiber$…`, and `a.parentNode === b.parentNode` all behave. Each node
comes out of the bridge with the prototype for its tag — `HTMLDivElement`,
`HTMLTemplateElement`, `Text` — chained the way the DOM chains them, so a
polyfill that patches one interface's prototype reaches that interface and
nothing else.

**A virtual clock.** Timers sit in a heap keyed by due time. When microtasks are
drained and no network call is outstanding, the clock jumps to the next timer.
Wall-clock time is never spent waiting. Timers past a horizon never fire, which
stops a polling page from staying alive forever. On a corpus of real sites this
saves nothing on the median page — the median page is waiting on the network —
and more than a second on one in eight.

**The engine never opens a socket.** It calls a `NetworkProvider` the host
installs, so policy lives in one place and tests run offline against canned
responses.

**Requests overlap, because a browser's do.** `Promise.all([fetch(a), fetch(b)])`
means two requests in flight, not two in a row, and on a page with a hundred of
them the difference is most of the wall clock. So `fetch` and asynchronous XHR
hand the request to the host and return a promise; the settle loop delivers each
answer as it lands, and a page with a request outstanding is not a settled page.
A synchronous XHR and an `import` still block, because they block in a browser
too.

Everything a page announces before running — `<script src>`, `modulepreload`,
`preload as=script` — is fetched together up front, and each module's own
imports are scanned out of its source and started before the engine asks for
them. A hundred subresources cost roughly one round trip's latency instead of a
hundred.

**Everything is bounded.** JS heap ceiling, stack ceiling, wall-clock budget,
virtual-time horizon, timer-callback budget, subresource-request budget, console
byte budget. A page that misbehaves is truncated and reported as truncated.

The wall-clock budget bounds the page and not merely its script loop: fetching
the page's code, loading its modules, following a navigation it asked for and
the scripts themselves all come out of the same number. And the interpreter
honours it — `while (true) {}` is stopped mid-instruction rather than waited
out, which a budget checked only between callbacks can never do.

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

The handshake can look like a browser's too. A build with the `browser-tls`
feature speaks TLS and HTTP/2 through BoringSSL exactly as a current Chrome
does — cipher order, extension set, GREASE, ALPS, the HTTP/2 SETTINGS frame —
so JA3, JA4 and Akamai's HTTP/2 fingerprint all match, and `navigator.userAgent`
says the same version the handshake does. On the corpus this is what opens
reddit, x.com, tass.ru, lamoda.ru, mvideo.ru, target.com and g1.globo.com,
each of which refuses rustls' handshake outright.

```bash
cargo build --release --features browser-tls   # needs cmake and a C++ compiler
mar read https://www.reddit.com/r/rust/
mar --no-impersonate read https://example.com/  # rustls' own handshake
```

What it does not do is solve a challenge. Cloudflare's "Just a moment" page
and Akamai's "Access Denied" decide on more than the handshake — they run
JavaScript that measures the browser — and a matching fingerprint gets the
challenge served rather than the page. A proxy still works for whatever the
build lacks:

```bash
mar --proxy socks5://127.0.0.1:1080 read https://example.com/
```

A proxy that will not parse fails every request rather than quietly going
direct: silently falling back is how a scrape leaks the address it was trying
not to use. With no `--proxy`, `HTTPS_PROXY`, `HTTP_PROXY` and `NO_PROXY` from
the environment are honoured, the way `curl` and a browser honour them.

## Safety

Requests to loopback, private, link-local and other reserved ranges are refused
by default, `localhost` included, and only `http` and `https` are allowed. A
rendering service fetches URLs its caller chose and then runs scripts that
choose more; without this, `http://169.254.169.254/` reads cloud credentials on
the caller's behalf. Pass `--allow-private` for local development, and
`--allow-host` to restrict fetching to a set of domains.

Scripts cannot navigate the host anywhere by themselves. A `location.href`
assignment or a `location.reload()` is recorded and followed only because a
browser would, bounded to three hops with loop detection, and subresource
requests to known trackers are refused.

`--obey-robots` asks each site's `robots.txt` before fetching a page from it,
once per host. Documents only: `robots.txt` governs what a crawler may go and
read, not which stylesheet a page it was already allowed to read pulls in.

## Limits

Known and deliberate:

- **No screenshots or PDFs.** They need the renderer this project removes.
- **TLS fingerprint only with `browser-tls`.** The default build's handshake
  is rustls, and a site that decides from the handshake alone can tell. The
  feature costs a BoringSSL build; `--proxy` is the other way around it.
- **No WebSocket, WebGL or media.** They construct without throwing and do
  nothing. A canvas hands out a 2D context that measures and draws nothing,
  which is enough for the libraries that open one at import time.
- **Workers share the thread.** A `Worker` runs its script in the page's
  interpreter, inside a scope that hides `window` and `document` and supplies
  `self`, `postMessage` and `importScripts`; messages cross on the next turn
  of the loop. A bot check that hashes in a worker gets its answer, and
  `crypto.subtle.digest` computes SHA-1 and SHA-256 for it. Nothing runs in
  parallel, so a worker that spins waits for the same budget as the page.
- **Custom elements upgrade in the light DOM only.** `customElements.define`
  runs the class's constructor against every element with that name,
  `connectedCallback` and `attributeChangedCallback` included. What a
  component renders into a shadow root is not the page's text, so a page built
  entirely from shadow-DOM components still reads as its light DOM.
- **No layout.** Nothing is positioned or wrapped. What an element reports for
  `getBoundingClientRect` and `offsetWidth` is derived from the cascade — an
  explicit `width` in a stylesheet or an inline style, a `width` attribute on a
  picture, otherwise an estimate from the content — because a page that
  measures zero concludes it has no room and renders its collapsed branch. The
  numbers are plausible, not true: nothing here knows where anything sits.
- **No locale database.** `Intl` is a shim with English and Russian tables and
  a neutral fallback for every other language, because QuickJS is built without
  ICU. Dates and prices come out plausible rather than exact. It is a shim and
  not an omission for one reason: without any `Intl` at all, a page that formats
  a single price throws inside its render and paints nothing.
- **Trackers are not fetched.** Analytics, ad exchanges and chat widgets are
  skipped by domain: they cost a request each and change nothing a reader will
  see. Everything else the page asks for is fetched, wherever it is hosted,
  because an application's own bundle and API routinely live on other hosts.
- **Non-HTML is not parsed.** A PDF or an image is refused with a clear
  message rather than served as text.
- **Extraction is heuristic.** A page with no clear article is reported with
  `low_confidence: true` rather than guessed at.

## Testing

```bash
cargo test
```

About a hundred and thirty tests: DOM structure and serialization round-trips, CSS
selector semantics, charset sniffing in the spec's precedence order (including a
regression for double-decoded koi8-r), SSRF policy, which hosts count as the
same site, `robots.txt` grouping and longest-match precedence, the cookie jar
the page shares with the client, article extraction, a CDP session driven end to
end including request interception and synthetic input, the MCP protocol
framing, and end-to-end rendering with promises, `fetch`, the virtual clock,
sloppy-mode globals, `Intl`, the element interfaces `instanceof` needs,
`document.currentScript`, error isolation and the runaway-page budget. None
touch the network.

## Licence

MIT.
