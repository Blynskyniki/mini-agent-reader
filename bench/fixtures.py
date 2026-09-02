#!/usr/bin/env python3
"""Deterministic pages for the benchmark, served locally.

Local so the numbers measure the engines rather than the network, and
deterministic so a rerun is comparable to the last one.
"""
import http.server, json, socketserver, sys

LOREM = ("Rolling deploys work by never taking the whole fleet out of rotation "
         "at once, which sounds obvious until you try it against a database "
         "migration that is not backwards compatible with the code still "
         "running on the other half of the fleet. ")

def article(paragraphs):
    body = "".join(f"<p>{LOREM * 3}</p>" for _ in range(paragraphs))
    return f"""<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>Static article</title><meta name="description" content="A server-rendered article.">
</head><body>
<header><nav>{"".join(f'<a href="/n{i}">Nav {i}</a>' for i in range(12))}</nav></header>
<article><h1>Static article</h1>{body}</article>
<footer><p>&copy; 2026</p></footer></body></html>"""

SPA = """<!doctype html><html><head><meta charset="utf-8"><title>Loading…</title></head>
<body><div id="root"></div><script src="/app.js"></script></body></html>"""

APP = """
async function main() {
  const res = await fetch('/api/article');
  const data = await res.json();
  document.title = data.title;
  document.getElementById('root').innerHTML =
    '<article><h1>' + data.title + '</h1>' +
    data.paragraphs.map(p => '<p>' + p + '</p>').join('') + '</article>';
}
main();
"""

DEFERRED = """<!doctype html><html><head><meta charset="utf-8"><title>Deferred</title></head>
<body><article id="root"><h1>Deferred</h1></article><script>
  // Three seconds of staggered work: the case a virtual clock collapses.
  let step = 0;
  const tick = () => {
    const p = document.createElement('p');
    p.textContent = 'Chunk ' + (++step) + '. ' + %s;
    document.getElementById('root').appendChild(p);
    if (step < 6) setTimeout(tick, 500);
  };
  setTimeout(tick, 500);
</script></body></html>""" % json.dumps(LOREM * 2)

def make_handler(paragraphs):
    class H(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *a): pass
        def send(self, body, ctype):
            if isinstance(body, str): body = body.encode()
            self.send_response(200)
            self.send_header('Content-Type', ctype)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def do_GET(self):
            if self.path == '/app.js':
                return self.send(APP, 'application/javascript')
            if self.path.startswith('/api/'):
                return self.send(json.dumps({
                    "title": "Rendered by script",
                    "paragraphs": [LOREM * 3] * paragraphs,
                }), 'application/json')
            if self.path.startswith('/spa'):
                return self.send(SPA, 'text/html')
            if self.path.startswith('/deferred'):
                return self.send(DEFERRED, 'text/html')
            if self.path.startswith('/large'):
                return self.send(article(200), 'text/html')
            return self.send(article(paragraphs), 'text/html')
    return H

if __name__ == '__main__':
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8760
    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.TCPServer(("127.0.0.1", port), make_handler(20)) as httpd:
        httpd.serve_forever()
