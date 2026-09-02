#!/usr/bin/env python3
"""Measure mar against headless Chromium on identical local pages.

Both engines are asked for the same thing — the HTML after scripts have run —
and both are measured the same way: a fresh process per page, wall time from
spawn to exit, peak resident memory from the kernel rather than from either
program's own accounting.

Local pages, so the numbers describe the engines and not the network. Several
runs, and the median is reported, because a first run pays for page cache and
the file system.
"""
import json
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MAR = ROOT / "target" / "release" / "mar"
BASE = "http://127.0.0.1:8760"
RUNS = int(os.environ.get("BENCH_RUNS", "7"))

PAGES = [
    ("Static article, 15 KB", "/"),
    ("Client-rendered, fetch", "/spa"),
    ("Deferred over 3s", "/deferred"),
    ("Large page, 145 KB", "/large"),
]


def chromium() -> str | None:
    """The chrome-headless-shell Playwright installed, if there is one."""
    cache = Path.home() / "Library" / "Caches" / "ms-playwright"
    if not cache.exists():
        cache = Path.home() / ".cache" / "ms-playwright"
    matches = sorted(cache.glob("chromium_headless_shell-*/*/chrome-headless-shell"))
    return str(matches[-1]) if matches else None


def measure(argv: list[str], timeout: float = 90.0) -> tuple[float, float, int]:
    """Run a command and return (wall ms, peak RSS MB, output bytes).

    Peak memory comes from `/usr/bin/time`, which reports the high-water mark
    for that one process. Reading getrusage(RUSAGE_CHILDREN) here would not:
    it accumulates across every child the benchmark has ever spawned, so once
    a heavy engine has run, every later measurement inherits its figure.
    """
    if sys.platform == "darwin":
        wrapper = ["/usr/bin/time", "-l"]
        needle = "maximum resident set size"
        scale = 1 << 20  # bytes
    else:
        wrapper = ["/usr/bin/time", "-v"]
        needle = "Maximum resident set size"
        scale = 1 << 10  # kilobytes

    start = time.perf_counter()
    result = subprocess.run(wrapper + argv, capture_output=True, timeout=timeout)
    wall = (time.perf_counter() - start) * 1000

    stderr = result.stderr.decode(errors="replace")
    rss = 0.0
    for line in stderr.splitlines():
        if needle in line:
            digits = "".join(c for c in line if c.isdigit())
            if digits:
                rss = int(digits) / scale
            break

    if result.returncode != 0:
        raise RuntimeError(stderr[-300:])
    return wall, rss, len(result.stdout)


def best_of(argv: list[str], runs: int = RUNS) -> dict:
    """Median of several runs, discarding the first as a warm-up."""
    samples = []
    for i in range(runs + 1):
        try:
            wall, rss, size = measure(argv)
        except Exception as e:
            return {"error": str(e)}
        if i == 0:
            continue  # warm-up
        samples.append((wall, rss, size))
    return {
        "ms": statistics.median(s[0] for s in samples),
        "rss": statistics.median(s[1] for s in samples),
        "bytes": samples[0][2],
    }


def main() -> int:
    if not MAR.exists():
        print(f"build first: {MAR} not found", file=sys.stderr)
        return 1
    chrome = chromium()

    print(f"mar:      {MAR}")
    print(f"chromium: {chrome or 'not installed, skipping the comparison'}")
    print(f"runs:     {RUNS} per case, median reported\n")

    rows = []
    for label, path in PAGES:
        url = BASE + path
        mar_html = best_of([str(MAR), "fetch", url, "--allow-private"])
        mar_read = best_of([str(MAR), "read", url, "--allow-private"])

        chrome_result = {"error": "not installed"}
        if chrome:
            # --dump-dom is the closest equivalent: run the page, print the DOM.
            chrome_result = best_of([
                chrome, "--headless", "--disable-gpu", "--no-sandbox",
                "--disable-dev-shm-usage", "--virtual-time-budget=5000",
                "--dump-dom", url,
            ])

        rows.append({
            "page": label,
            "mar_html": mar_html,
            "mar_read": mar_read,
            "chromium": chrome_result,
        })

    header = f"{'Page':<24}{'mar html':>12}{'mar read':>12}{'Chromium':>12}{'memory':>22}"
    print(header)
    print("-" * len(header))
    for row in rows:
        m, r, c = row["mar_html"], row["mar_read"], row["chromium"]
        mem = f"{m.get('rss', 0):.0f} MB vs {c.get('rss', 0):.0f} MB" if "rss" in c else f"{m.get('rss', 0):.0f} MB"
        chrome_ms = f"{c['ms']:>10.0f}ms" if "ms" in c else f"{'n/a':>12}"
        print(
            f"{row['page']:<24}"
            f"{m.get('ms', 0):>10.0f}ms"
            f"{r.get('ms', 0):>10.0f}ms"
            f"{chrome_ms}"
            f"{mem:>22}"
        )

    # Both engines must have produced the same page, or the timings compare
    # two different amounts of work. The client-rendered case is the one that
    # matters: it is only equal if the scripts ran on both sides.
    print("\nSame work? (rendered markers found in each engine's output)")
    for label, path in PAGES:
        url = BASE + path
        marker = "Rendered by script" if path == "/spa" else "Chunk 6" if path == "/deferred" else "<h1>"
        mar_out = subprocess.run(
            [str(MAR), "fetch", url, "--allow-private"], capture_output=True
        ).stdout.decode(errors="replace")
        chrome_out = ""
        if chrome:
            chrome_out = subprocess.run(
                [chrome, "--headless", "--disable-gpu", "--no-sandbox",
                 "--virtual-time-budget=5000", "--dump-dom", url],
                capture_output=True,
            ).stdout.decode(errors="replace")
        print(
            f"  {label:<24} marker {marker!r:<22}"
            f" mar={marker in mar_out}  chromium={marker in chrome_out if chrome else 'n/a'}"
        )

    print("\nThroughput, one long-running process (mar serve)")
    print(f"  {throughput()}")

    print("\nCold start (process spawn to first byte of output)")
    cold_mar = best_of([str(MAR), "--version"])
    print(f"  mar --version        {cold_mar.get('ms', 0):>7.1f} ms   {cold_mar.get('rss', 0):>5.1f} MB")
    if chrome:
        cold_chrome = best_of([chrome, "--version"])
        print(f"  chrome --version     {cold_chrome.get('ms', 0):>7.1f} ms   {cold_chrome.get('rss', 0):>5.1f} MB")

    print("\nOn-disk size")
    print(f"  mar binary           {MAR.stat().st_size / (1 << 20):>7.1f} MB")
    if chrome:
        root = Path(chrome).parent
        total = sum(f.stat().st_size for f in root.rglob("*") if f.is_file())
        print(f"  chrome-headless-shell{total / (1 << 20):>7.1f} MB")

    print()
    print(json.dumps(rows, indent=1))
    return 0


def throughput(pages: int = 60, workers: int = 4) -> str:
    """Pages per second through one long-running server.

    The per-page numbers above include process startup every time. A service
    starts once, so this is the figure that describes a service.
    """
    import http.client
    import threading

    port = 8781
    server = subprocess.Popen(
        [str(MAR), "serve", "--bind", f"127.0.0.1:{port}",
         "--workers", str(workers), "--allow-private"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        for _ in range(100):
            try:
                conn = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
                conn.request("GET", "/health")
                conn.getresponse().read()
                conn.close()
                break
            except Exception:
                time.sleep(0.05)
        else:
            return "server did not start"

        body = json.dumps({"url": BASE + "/spa"})
        errors = []

        def worker(count: int) -> None:
            conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
            for _ in range(count):
                try:
                    conn.request("POST", "/read", body,
                                 {"Content-Type": "application/json"})
                    conn.getresponse().read()
                except Exception as e:
                    errors.append(str(e))
                    return
            conn.close()

        threads = [threading.Thread(target=worker, args=(pages // workers,))
                   for _ in range(workers)]
        start = time.perf_counter()
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        elapsed = time.perf_counter() - start
        if errors:
            return f"failed: {errors[0][:80]}"
        return (f"{pages} client-rendered pages on {workers} workers in "
                f"{elapsed * 1000:.0f} ms  =  {pages / elapsed:.0f} pages/sec, "
                f"{elapsed / pages * 1000:.1f} ms each")
    finally:
        server.terminate()
        server.wait(timeout=5)


if __name__ == "__main__":
    sys.exit(main())
