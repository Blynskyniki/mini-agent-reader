#!/usr/bin/env python3
"""Does the engine read the real web? A corpus of live sites, scored against Chrome.

Every page is reduced the same way in both engines: the settled document with
script, style, template and noscript subtrees removed, then whitespace
collapsed, then counted. A page "reads" when mar's text is at least 60% of
Chrome's on the same URL; "partial" from 10%; "empty" below that.

    python3 bench/corpus.py --chrome                 # measure Chrome too (slow)
    python3 bench/corpus.py --chrome-from corpus_results.json   # reuse a Chrome column

`bench/corpus.json` is 453 URLs: the top of the Tranco list with
infrastructure, parked, adult and duplicate hosts removed, plus the Russian web
and a set of client-rendered applications. Sites behind a bot check are kept on
purpose — they are the honest hard cases — but a host that resolved to nothing
for either engine is not a measurement of anything and was dropped.
"""
import argparse, json, re, statistics, subprocess, sys, time
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from html.parser import HTMLParser
from pathlib import Path

HERE = Path(__file__).resolve().parent
MAR = HERE.parent / "target" / "release" / "mar"
CHROME_CANDIDATES = sorted(
    Path.home().glob("Library/Caches/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-mac-arm64/chrome-headless-shell")
) + sorted(Path.home().glob(".cache/ms-playwright/chromium_headless_shell-*/chrome-linux/headless_shell"))

SKIP = {"script", "style", "noscript", "template", "svg", "head"}

class Text(HTMLParser):
    """Visible text with script and style subtrees actually removed. A regex
    cuts at the first `</script>` inside a JS string, and mar inlines bundles
    into the document, so the error would land on one engine only."""
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.depth = 0
        self.parts = []
    def handle_starttag(self, tag, attrs):
        if tag in SKIP:
            self.depth += 1
    def handle_endtag(self, tag):
        if tag in SKIP and self.depth:
            self.depth -= 1
    def handle_data(self, data):
        if not self.depth:
            self.parts.append(data)

def visible_text(markup):
    p = Text()
    try:
        p.feed(markup)
    except Exception:
        pass
    return re.sub(r"\s+", " ", "".join(p.parts)).strip()

ERROR_LINE = re.compile(r"([A-Za-z]*Error|Error):\s*([^\n]{0,90})")

def signature(message):
    """Collapse one error to the shape of the bug behind it."""
    m = ERROR_LINE.search(message)
    if not m:
        return re.sub(r"https?://\S+", "<url>", message.split("\n")[0])[:80]
    kind, text = m.group(1), re.sub(r"\b\d+\b", "N", re.sub(r"https?://\S+", "<url>", m.group(2)))
    return f"{kind}: {text.strip()}"[:90]

def classify(mar_text, chrome_text, failed):
    if failed:
        return "mar refused"
    if chrome_text < 400 and mar_text < 400:
        return "both empty"
    if chrome_text < 400:
        return "chrome empty, mar has text"
    if mar_text < 0.1 * chrome_text:
        return "mar empty"
    if mar_text < 0.6 * chrome_text:
        return "mar partial"
    return "reads"

ORDER = ["reads", "mar partial", "mar empty", "mar refused", "both empty", "chrome empty, mar has text"]

def chrome_one(chrome, url, budget_ms):
    started = time.perf_counter()
    try:
        r = subprocess.run(
            [str(chrome), "--headless", "--disable-gpu", "--no-sandbox", "--hide-scrollbars",
             "--disable-dev-shm-usage", f"--virtual-time-budget={budget_ms}",
             f"--timeout={budget_ms + 12000}", "--dump-dom", url],
            capture_output=True, timeout=(budget_ms + 25000) / 1000)
        ok, out = r.returncode == 0, r.stdout
    except subprocess.TimeoutExpired as e:
        ok, out = False, e.stdout or b""
    ms = round((time.perf_counter() - started) * 1000)
    return {"text": len(visible_text(out.decode("utf-8", "replace"))) if ok else 0, "ms": ms, "ok": ok}

def mar_pass(binary, urls, timeout_ms, concurrency, extra):
    """One `mar scrape --shape html` over the corpus, streamed: a corpus of
    settled documents with bundles inlined runs to hundreds of megabytes."""
    cmd = [str(binary), "scrape", "-q", "--shape", "html", "--timeout-ms", str(timeout_ms),
           "-c", str(concurrency), *extra, "-"]
    started = time.perf_counter()
    records = {}
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    proc.stdin.write("\n".join(urls).encode())
    proc.stdin.close()
    for raw in proc.stdout:
        raw = raw.strip()
        if not raw:
            continue
        try:
            obj = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if "url" not in obj:
            continue
        if "html" in obj:
            obj["text"] = len(visible_text(obj.pop("html")))
        records[obj["url"]] = obj
        if len(records) % 50 == 0:
            print(f"  {len(records)}/{len(urls)} {time.perf_counter()-started:.0f}s", flush=True)
    err = proc.stderr.read().decode("utf-8", "replace")
    rc = proc.wait()
    print(f"  [mar] {len(records)}/{len(urls)} in {time.perf_counter()-started:.0f}s, exit {rc}", flush=True)
    if "panicked" in err:
        print("  " + err[err.find("panicked"):][:400], flush=True)
    return records

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--corpus", default=str(HERE / "corpus.json"))
    ap.add_argument("--binary", default=str(MAR))
    ap.add_argument("--chrome", action="store_true", help="measure Chrome on every URL first")
    ap.add_argument("--chrome-from", default=None, help="reuse the Chrome column of an earlier results file")
    ap.add_argument("--chrome-binary", default=str(CHROME_CANDIDATES[-1]) if CHROME_CANDIDATES else None)
    ap.add_argument("--chrome-workers", type=int, default=6)
    ap.add_argument("--chrome-budget", type=int, default=8000)
    ap.add_argument("--timeout-ms", type=int, default=15000)
    ap.add_argument("--concurrency", type=int, default=6)
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--out", default=str(HERE / "corpus_results.json"))
    ap.add_argument("extra", nargs="*", help="passed to `mar scrape`, e.g. --no-impersonate")
    args = ap.parse_args()

    urls = json.loads(Path(args.corpus).read_text())
    if args.limit:
        urls = urls[: args.limit]

    chrome = {}
    if args.chrome_from:
        chrome = json.loads(Path(args.chrome_from).read_text())["chrome"]
        print(f"  [chrome] {len(chrome)} reused from {args.chrome_from}", flush=True)
    elif args.chrome:
        if not args.chrome_binary:
            sys.exit("no chrome-headless-shell found; install one with `npx playwright install chromium`")
        started = time.perf_counter()
        with ThreadPoolExecutor(max_workers=args.chrome_workers) as ex:
            for url, res in zip(urls, ex.map(lambda u: chrome_one(args.chrome_binary, u, args.chrome_budget), urls)):
                chrome[url] = res
        print(f"  [chrome] read {sum(1 for r in chrome.values() if r['text'] >= 400)}/{len(urls)} "
              f"in {time.perf_counter()-started:.0f}s", flush=True)
    else:
        print("  no Chrome column: pass --chrome or --chrome-from; classes will be relative to nothing", flush=True)

    records = mar_pass(args.binary, urls, args.timeout_ms, args.concurrency, args.extra)
    rows = []
    for url in urls:
        r = records.get(url) or {}
        report = r.get("report") or {}
        ref = chrome.get(url) or {"text": 0, "ms": 0}
        failed = bool(r.get("error")) and not report
        rows.append({
            "url": url, "status": report.get("status"), "text": r.get("text", 0),
            "chrome_text": ref["text"], "chrome_ms": ref["ms"],
            "ms": report.get("total_ms"), "scripts_run": report.get("scripts_run"),
            "requests": report.get("subresource_requests"), "truncated": report.get("truncated"),
            "final_url": report.get("final_url"), "errors": report.get("errors") or [],
            "failure": r.get("error"), "class": classify(r.get("text", 0), ref["text"], failed),
        })
    Path(args.out).write_text(json.dumps({"urls": urls, "chrome": chrome, "rows": rows}, ensure_ascii=False, indent=0))

    total = len(rows)
    classes = Counter(r["class"] for r in rows)
    print(f"\n=== {args.out} ===")
    for k in ORDER:
        if classes[k]:
            print(f"  {classes[k]:>4}  {classes[k]/total*100:5.1f}%  {k}")
    live = [r for r in rows if r["chrome_text"] >= 400]
    if live:
        print(f"  Chrome read {len(live)}; of those mar reads {sum(1 for r in live if r['class'] == 'reads')}")
    errs, examples = Counter(), defaultdict(list)
    for r in rows:
        for s in {signature(e) for e in r["errors"]}:
            errs[s] += 1
            examples[s].append(r["url"])
    print(f"  pages with script errors: {sum(1 for r in rows if r['errors'])}")
    for s, n in errs.most_common(15):
        print(f"    {n:>4}  {s}    e.g. {examples[s][0]}")
    fails = Counter((r["failure"] or "")[:60] for r in rows if r["class"] == "mar refused")
    if fails:
        print("  refused:")
        for s, n in fails.most_common(8):
            print(f"    {n:>4}  {s}")
    ok = sorted(r["ms"] for r in rows if r["status"] == 200 and r["ms"])
    if ok:
        print(f"  median ms {statistics.median(ok):.0f}  p90 {ok[int(len(ok)*0.9)]}")

if __name__ == "__main__":
    main()
