# Benchmarks

Numbers in the project README come from here. Two harnesses.

## `run.py` — head to head against Chromium

Serves identical local pages to `mar` and to `chrome-headless-shell`, asks both
for the same thing (the DOM after scripts have run), and measures each the same
way: a fresh process per page, wall time from spawn to exit, peak resident
memory read from the kernel through `/usr/bin/time`.

Local pages, so the figures describe the engines and not the network. Median of
several runs, first discarded, because a first run pays for page cache.

```bash
python3 bench/fixtures.py 8760 &
BENCH_RUNS=7 python3 bench/run.py
```

It also checks that both engines produced the same page. Each case has a marker
that appears only once the scripts have run — text arriving from `fetch`, or a
chunk appended three seconds in. If one engine misses its marker, the timings
are comparing different amounts of work and the comparison is void.

Chromium comes from a Playwright install (`npx playwright install chromium`).
Without one the harness skips the comparison and reports `mar` alone.

## `real.py` — real sites

Wall time here includes the network and says more about the site than the
engine. Memory does not: it follows document size and how much script the page
runs, which is what these numbers are for.

```bash
python3 bench/real.py
```

## `corpus.py` — does it read the real web?

`corpus.json` is 453 live URLs: the top of the Tranco list with
infrastructure, parked, adult and duplicate hosts removed, plus the Russian
web and a set of client-rendered applications. Sites behind a bot check are
kept on purpose; hosts that resolve to nothing were dropped, because they
measure the network and not the engine.

Every page is reduced the same way in both engines — the settled document
with script, style, template and noscript subtrees removed, whitespace
collapsed — and mar's text is compared with Chrome's on the same URL in the
same session. A page "reads" at 60% of Chrome's text or more.

```bash
python3 bench/corpus.py --chrome                          # Chrome too, ~7 min
python3 bench/corpus.py --chrome-from bench/corpus_results.json   # reuse it
python3 bench/corpus.py --chrome-from ... -- --no-impersonate     # rustls handshake
```

Run the whole corpus at `--concurrency 6`, which is what Chrome gets. A page
that reads on its own and not in the batch is usually the batch: one proxy,
six pages of subresources at once, and a budget of fifteen seconds.

## Reading the results

`chrome-headless-shell` is Chromium's lightest path: no interface, no GPU
process. Comparing against it rather than against full Chromium through
Puppeteer is the harder comparison to win, and the honest one to publish.

The deferred-work case runs Chromium with `--virtual-time-budget`, its own
mechanism for collapsing timer delays. Without that flag the gap on that row
would be roughly 400×, which would say more about the flag than about either
engine.
