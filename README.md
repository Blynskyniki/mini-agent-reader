# mini-agent-reader

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
| Peak RSS, typical page | 6–7 MB | 250–400 MB |
| Peak RSS, large page (Wikipedia) | 19 MB | 400 MB+ |
| Render time, local SPA | 4 ms | 300–800 ms |
| Cold start | ~2 ms | 150–400 ms |

The virtual clock is the reason for the render figure. A page that spreads its
work over three seconds of `setTimeout` settles in microseconds, because time
only advances when nothing else is runnable.

## What it is

Five crates, each usable on its own.

- **`mar-dom`** — HTML parsing and an arena DOM. Nodes live in a flat `Vec`
  addressed by a 32-bit index, with no per-node reference counting. CSS matching
  is delegated to Servo's `selectors`, so `:has`, `:is` and `:nth-child` work.
- **`mar-js`** — QuickJS with DOM bindings, a virtual-clock event loop, and a
  JavaScript prelude that builds the browser environment.
- **`mar-net`** — HTTP with browser-shaped headers, charset sniffing per the
  HTML spec, and a policy layer that blocks private address space.
- **`mar-extract`** — Readability-style article detection, metadata, Markdown.
- **`mar-cli`** — the `mar` binary and the HTTP server.

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

## Use

```bash
cargo build --release          # ~30s, no system dependencies beyond a C compiler
```

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

Serve it:

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
- **No TLS fingerprint spoofing.** Headers look like Chrome; the TLS handshake
  does not. Sites behind an interstitial challenge will still serve one.
- **No module loader.** A `type="module"` script compiles and runs, but an
  `import` of another URL fails. Page runtimes that bundle this way log an
  error and the rest of the page still renders.
- **No WebSocket, Worker, WebGL, canvas or media.** They construct without
  throwing and do nothing.
- **Cross-origin scripts are not fetched.** Third-party bundles are almost
  always analytics and advertising: slow, and they add nothing to read.
- **Extraction is heuristic.** A page with no clear article is reported with
  `low_confidence: true` rather than guessed at.

## Testing

```bash
cargo test
```

Thirty-nine tests: DOM structure and serialization round-trips, CSS selector
semantics, charset sniffing against the spec's precedence order, SSRF policy,
article extraction, and end-to-end rendering including promises, fetch, the
virtual clock, error isolation and the runaway-page budget. None touch the
network.
