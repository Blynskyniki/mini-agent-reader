#!/usr/bin/env python3
"""Peak memory and extraction size on real pages.

Timings here include the network, so they say more about the site than the
engine. Memory does not: it is set by the size of the document and how much
script the page runs, which is what these numbers are for.
"""
import json, subprocess, sys, time
from pathlib import Path

MAR = Path(__file__).resolve().parent.parent / "target" / "release" / "mar"
SITES = [
    ("example.com", "https://example.com/"),
    ("lenta.ru", "https://lenta.ru/"),
    ("ria.ru", "https://ria.ru/"),
    ("habr.com", "https://habr.com/ru/articles/"),
    ("gosuslugi.ru", "https://www.gosuslugi.ru/"),
    ("docs.python.org", "https://docs.python.org/3/tutorial/introduction.html"),
    ("Wikipedia (large)", "https://en.wikipedia.org/wiki/Fusion_power"),
    ("MDN", "https://developer.mozilla.org/en-US/docs/Web/API/Fetch_API"),
]

def run(url):
    start = time.perf_counter()
    p = subprocess.run(
        ["/usr/bin/time", "-l", str(MAR), "read", url, "--format", "json"],
        capture_output=True, timeout=90,
    )
    wall = (time.perf_counter() - start) * 1000
    rss = 0.0
    for line in p.stderr.decode(errors="replace").splitlines():
        if "maximum resident set size" in line:
            digits = "".join(c for c in line if c.isdigit())
            rss = int(digits) / (1 << 20) if digits else 0.0
            break
    if p.returncode != 0:
        return None
    d = json.loads(p.stdout)
    return d["reading"], d["report"], wall, rss

print(f"{'Site':<20}{'wall':>8}{'render':>8}{'memory':>9}{'chars':>8}{'scripts':>9}")
print("-" * 62)
for name, url in SITES:
    try:
        out = run(url)
        if out is None:
            print(f"{name:<20}{'failed':>8}")
            continue
        reading, report, wall, rss = out
        print(f"{name:<20}{wall:>7.0f}ms{report['render_ms']:>7}ms"
              f"{rss:>7.1f}MB{reading['length']:>8}{report['scripts_run']:>9}")
    except Exception as e:
        print(f"{name:<20}{'error':>8}  {str(e)[:40]}")
