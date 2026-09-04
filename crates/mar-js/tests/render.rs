//! End-to-end: HTML in, rendered HTML out, with scripts having run.

use mar_js::{Limits, NoNetwork, Page, StaticNetwork};

fn render(html: &str) -> mar_js::PageOutcome {
    let mut page = Page::new(
        html,
        "https://example.com/page",
        Limits::default(),
        NoNetwork,
    )
    .expect("page builds");
    page.run()
}

#[test]
fn a_spa_shell_renders_its_content() {
    // The shape of every client-rendered page: an empty root plus a script.
    let out = render(
        r#"<!doctype html><html><head><title>Shell</title></head>
        <body><div id="root"></div>
        <script>
          const data = [{name: 'Alpha', price: 10}, {name: 'Beta', price: 20}];
          const root = document.getElementById('root');
          root.innerHTML = '<ul>' + data.map(d =>
            `<li class="item" data-price="${d.price}">${d.name}</li>`).join('') + '</ul>';
          document.title = 'Shell (' + data.length + ')';
        </script></body></html>"#,
    );

    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(out.html.contains("Alpha"), "content rendered: {}", out.html);
    assert!(out.html.contains(r#"data-price="20""#));
    assert_eq!(out.title, "Shell (2)");
    assert_eq!(out.scripts_run, 1);
}

#[test]
fn deferred_work_settles_on_the_virtual_clock() {
    // Three seconds of staggered work must cost no real time.
    let out = render(
        r#"<body><div id="out"></div><script>
          setTimeout(() => {
            document.getElementById('out').textContent = 'late';
            setTimeout(() => {
              document.getElementById('out').textContent += ' later';
            }, 2000);
          }, 1000);
          let n = 0;
          const h = setInterval(() => { if (++n === 3) clearInterval(h); }, 250);
        </script></body>"#,
    );

    assert!(out.html.contains("late later"), "html: {}", out.html);
    assert!(
        out.virtual_ms >= 3000,
        "virtual clock advanced: {}",
        out.virtual_ms
    );
    assert!(out.wall_ms < 1000, "but no real waiting: {}ms", out.wall_ms);
    assert!(!out.truncated);
}

#[test]
fn promises_and_async_await_resolve_before_the_page_settles() {
    let out = render(
        r#"<body><div id="a"></div><script>
          const wait = ms => new Promise(r => setTimeout(r, ms));
          (async () => {
            await wait(50);
            const parts = await Promise.all([
              Promise.resolve('x'),
              wait(10).then(() => 'y'),
            ]);
            document.querySelector('#a').textContent = parts.join('-');
          })();
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert!(out.html.contains("x-y"), "html: {}", out.html);
}

#[test]
fn fetch_feeds_the_dom() {
    let net = StaticNetwork::new().route(
        "/api/items",
        200,
        "application/json",
        r#"{"items":[{"t":"From the API"}]}"#,
    );
    let mut page = Page::new(
        r#"<body><main></main><script>
             fetch('/api/items')
               .then(r => r.json())
               .then(d => {
                 document.querySelector('main').innerHTML =
                   d.items.map(i => '<h2>' + i.t + '</h2>').join('');
               })
               .catch(e => { document.querySelector('main').textContent = 'ERR ' + e; });
           </script></body>"#,
        "https://example.com/",
        Limits::default(),
        net,
    )
    .unwrap();
    let out = page.run();

    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert!(
        out.html.contains("<h2>From the API</h2>"),
        "html: {}",
        out.html
    );
    assert_eq!(out.requests, 1);
}

#[test]
fn events_dom_apis_and_storage_behave() {
    let out = render(
        r#"<body>
        <button id="b">go</button><div id="log"></div>
        <input id="i" value="attr">
        <script>
          const log = document.getElementById('log');
          const b = document.getElementById('b');
          b.addEventListener('click', e => {
            log.textContent += 'clicked:' + e.type + ';';
            e.target.classList.add('done');
          });
          b.click();
          b.click();

          // classList, dataset and style write through to attributes.
          b.classList.add('a', 'b');
          b.classList.remove('a');
          b.dataset.role = 'primary';
          b.style.display = 'none';

          // Form values are off-DOM, as in a browser.
          const i = document.getElementById('i');
          const before = i.value;
          i.value = 'typed';
          log.textContent += before + '->' + i.value + ';';

          localStorage.setItem('k', 'v');
          log.textContent += 'ls:' + localStorage.getItem('k') + localStorage.length + ';';
          document.cookie = 'sid=42';
          log.textContent += 'ck:' + document.cookie + ';';
        </script></body>"#,
    );

    assert!(out.errors.is_empty(), "{:?}", out.errors);
    let log = out
        .html
        .split(r#"<div id="log">"#)
        .nth(1)
        .and_then(|s| s.split("</div>").next())
        .unwrap_or("");
    // ">" is escaped on serialization, as the HTML spec requires.
    assert_eq!(
        log,
        "clicked:click;clicked:click;attr-&gt;typed;ls:v1;ck:sid=42;"
    );

    assert!(
        out.html.contains(r#"class="done b""#),
        "classList: {}",
        out.html
    );
    assert!(out.html.contains(r#"data-role="primary""#));
    assert!(out.html.contains("display: none"));
    // The value attribute is untouched by typing, matching a real browser.
    assert!(out.html.contains(r#"value="attr""#));
    assert_eq!(out.cookies, "sid=42");
}

#[test]
fn a_thrown_script_does_not_stop_the_others() {
    let out = render(
        r#"<body><div id="x"></div>
        <script>document.querySelector('#x').textContent = 'one;';</script>
        <script>null.boom();</script>
        <script>document.querySelector('#x').textContent += 'three';</script>
        </body>"#,
    );
    assert!(out.html.contains("one;three"), "html: {}", out.html);
    assert_eq!(
        out.errors.len(),
        1,
        "the throw was recorded: {:?}",
        out.errors
    );
    assert_eq!(out.scripts_run, 3);
}

#[test]
fn an_infinite_loop_is_stopped_by_the_timer_budget() {
    let limits = Limits {
        max_timer_callbacks: 50,
        ..Limits::default()
    };
    let mut page = Page::new(
        r#"<body><script>
             let n = 0;
             const tick = () => { n++; setTimeout(tick, 1); };
             tick();
             setInterval(() => {}, 1);
           </script></body>"#,
        "https://example.com/",
        limits,
        NoNetwork,
    )
    .unwrap();
    let out = page.run();

    assert!(out.truncated, "the loop was cut short");
    assert!(
        out.timer_callbacks <= 51,
        "budget held: {}",
        out.timer_callbacks
    );
    assert!(out.wall_ms < 5000, "and it did not hang: {}ms", out.wall_ms);
}

#[test]
fn a_synchronous_loop_is_stopped_by_the_wall_clock() {
    // The settle loop checks the budget between callbacks, which is no help
    // inside one. Without an interrupt handler this page never returns and the
    // process waits forever — and a page off the open web gets to do that on
    // purpose.
    let limits = Limits {
        wall_ms: 500,
        ..Limits::default()
    };
    let mut page = Page::new(
        r#"<body><div id="out">before</div><script>
             document.getElementById('out').textContent = 'ran';
             while (true) {}
           </script></body>"#,
        "https://example.com/page",
        limits,
        NoNetwork,
    )
    .expect("page builds");
    let started = std::time::Instant::now();
    let out = page.run();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the loop is cut off at the budget, not waited out"
    );
    let reported = out
        .errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<String>();
    assert!(
        reported.contains("interrupted"),
        "the page is told why it stopped: {reported}"
    );
    assert!(
        out.html.contains("ran"),
        "everything before the loop still counts: {}",
        out.html
    );
}

#[test]
fn navigation_is_reported_not_followed() {
    let out = render(r#"<body><script>location.href = '/next?a=1';</script></body>"#);
    assert_eq!(
        out.requested_navigation.as_ref().map(|n| n.url.as_str()),
        Some("https://example.com/next?a=1"),
        "resolved against the page URL, and not acted on"
    );
    assert_eq!(out.requests, 0);
}

#[test]
fn a_handler_written_into_the_markup_runs() {
    let out = render(
        r#"<body>
             <button id="b" onclick="document.getElementById('out').textContent = 'fired'">go</button>
             <div id="out">none</div>
             <script>
               document.getElementById('b').dispatchEvent(new Event('click', { bubbles: true }));
             </script>
           </body>"#,
    );
    assert!(
        out.html.contains(">fired<"),
        "an onclick attribute is a handler, not just an attribute: {}",
        out.html
    );
}

#[test]
fn a_broken_handler_attribute_does_not_take_the_page_down() {
    let out = render(
        r#"<body><div id="out">still here</div>
             <button onclick="this is not javascript">x</button>
           </body>"#,
    );
    assert!(out.html.contains("still here"));
}

#[test]
fn the_environment_does_not_advertise_itself() {
    let out = render(
        r#"<body><div id="out"></div><script>
          const page = function mine(a) { return a + 1 };
          document.getElementById('out').textContent = JSON.stringify({
            fetch: String(fetch),
            listener: String(addEventListener),
            named: fetch.name,
            page: String(page),
            internals: Object.keys(globalThis).filter(k => k.startsWith('__mar')),
          });
        </script></body>"#,
    );
    let seen = &out.html;
    assert!(
        seen.contains(r#"function fetch() { [native code] }"#),
        "a function written in the prelude still reads as built in: {seen}"
    );
    assert!(
        seen.contains(r#"function addEventListener() { [native code] }"#),
        "and so does one reached through a property walk: {seen}"
    );
    assert!(
        seen.contains(r#"function mine(a) { return a + 1 }"#),
        "while the page's own source is left alone: {seen}"
    );
    assert!(
        seen.contains(r#""internals":[]"#),
        "and the bridge the CDP layer calls is not enumerable: {seen}"
    );
}

#[test]
fn reload_is_a_navigation_to_the_same_url() {
    let out = render(r#"<body><script>location.reload();</script></body>"#);
    assert_eq!(
        out.requested_navigation.as_ref().map(|n| n.url.as_str()),
        Some("https://example.com/page"),
        "a challenge page sets a cookie and reloads; the reload has to be visible"
    );
}

#[test]
fn cookie_assignments_are_kept_verbatim() {
    let out = render(
        r#"<body><script>
          document.cookie = 'a=1; path=/; expires=Thu, 01 Jan 2099 00:00:00 GMT';
          document.cookie = 'b=2';
        </script></body>"#,
    );
    assert_eq!(
        out.cookie_writes,
        vec![
            "a=1; path=/; expires=Thu, 01 Jan 2099 00:00:00 GMT".to_owned(),
            "b=2".to_owned(),
        ],
        "the attributes survive for a host that owns a real jar"
    );
    assert_eq!(out.cookies, "a=1; b=2", "while document.cookie stays flat");
}

#[test]
fn the_page_reports_console_output() {
    let out = render(
        r#"<body><script>
          console.log('plain', 42, {a: [1, 2]});
          console.warn('careful');
          console.error(new Error('bad'));
        </script></body>"#,
    );
    let lines: Vec<_> = out
        .console
        .iter()
        .map(|m| (m.level.as_str(), m.text.as_str()))
        .collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0].0, "log");
    assert!(lines[0].1.starts_with("plain 42 {"), "{:?}", lines[0].1);
    assert_eq!(lines[1], ("warn", "careful"));
    assert!(lines[2].1.starts_with("Error: bad"), "{:?}", lines[2].1);
}

#[test]
fn evaluating_an_expression_returns_json() {
    let mut page = Page::new(
        r#"<body><ul><li data-id="1">a</li><li data-id="2">b</li></ul></body>"#,
        "https://example.com/",
        Limits::default(),
        NoNetwork,
    )
    .unwrap();
    page.run();
    let json = page
        .eval_json(
            "[...document.querySelectorAll('li')].map(l => ({id: l.dataset.id, t: l.textContent}))",
        )
        .unwrap();
    assert_eq!(json, r#"[{"id":"1","t":"a"},{"id":"2","t":"b"}]"#);
}

// -- modules ----------------------------------------------------------------

fn render_with(net: StaticNetwork, html: &str) -> mar_js::PageOutcome {
    let mut page =
        Page::new(html, "https://example.com/page", Limits::default(), net).expect("page builds");
    page.run()
}

#[test]
fn a_classic_script_runs_in_sloppy_mode() {
    // React's streaming markup bootstraps itself with bare assignments:
    // `$RC = function(a, b) {...}` in the first completed boundary, and calls
    // to `$RC(...)` in every one after it. Strict mode makes the first line a
    // ReferenceError, and every suspense boundary on the page is then lost.
    let out = render(
        r#"<body><div id="out"></div>
        <script>$RC = function (id, text) { document.getElementById(id).textContent = text; };</script>
        <script>$RC('out', 'boundary revealed');</script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains("boundary revealed"),
        "an undeclared assignment is a global, as in a browser: {}",
        out.html
    );
}

#[test]
fn the_running_script_is_document_current_script() {
    // Bundlers read `document.currentScript.src` to work out where their own
    // chunks live. webpack throws "Automatic publicPath is not supported"
    // outright when it comes back null, taking the application with it.
    let out = render(
        r#"<body><div id="out"></div>
        <script src="/assets/bundle.js">
          document.getElementById('out').textContent =
            document.currentScript.tagName + ' ' + document.currentScript.src;
        </script>
        <script>
          document.getElementById('out').textContent += ' | after: ' + document.currentScript;
          setTimeout(() => {
            document.getElementById('out').textContent += ' | timer: ' + document.currentScript;
          }, 0);
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html
            .contains("SCRIPT https://example.com/assets/bundle.js"),
        "the running script knows where it came from: {}",
        out.html
    );
    assert!(
        out.html.contains("| timer: null"),
        "nothing is the current script once the scripts have run: {}",
        out.html
    );
}

#[test]
fn intl_formats_rather_than_throwing() {
    // QuickJS ships no Intl. A page that formats one price through it throws
    // inside its render and paints nothing at all.
    let out = render(
        r#"<body><div id="out"></div><script>
          const parts = [
            new Intl.NumberFormat('en-US').format(1234567.5),
            new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' }).format(9.9),
            new Intl.DateTimeFormat('en-US').format(new Date(2026, 0, 2)),
            new Intl.PluralRules('ru').select(3),
            // Written without `new` everywhere, because this is how a page
            // asks a browser what time zone it is in.
            typeof Intl.DateTimeFormat().resolvedOptions().timeZone,
            (1234.5).toLocaleString('en-US'),
          ];
          document.getElementById('out').textContent = parts.join(' | ');
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html
            .contains("1,234,567.5 | $9.90 | 1/2/2026 | few | string | 1,234.5"),
        "formatting produces something a reader recognises: {}",
        out.html
    );
}

#[test]
fn element_interfaces_exist_for_instanceof() {
    // React's scheduler alone tests HTMLDivElement, HTMLBRElement,
    // HTMLBodyElement and ShadowRoot. An undefined right-hand operand of
    // `instanceof` is a TypeError, and one of those inside a render takes the
    // whole application down.
    let out = render(
        r#"<body><div id="out"></div><script>
          const div = document.createElement('div');
          const results = [
            div instanceof HTMLDivElement,
            div instanceof HTMLElement,
            div instanceof HTMLBRElement,
            document.body instanceof HTMLBodyElement,
            document.createElement('h3') instanceof HTMLHeadingElement,
            document.createElement('td') instanceof HTMLTableCellElement,
            div instanceof ShadowRoot,
            new Image(16, 16) instanceof HTMLImageElement,
            new Image().tagName === 'IMG',
            localStorage instanceof Storage,
          ];
          document.getElementById('out').textContent = results.join(',');
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html
            .contains("true,true,false,true,true,true,false,true,true,true"),
        "each interface answers for its own tag and no other: {}",
        out.html
    );
}

#[test]
fn a_link_is_also_a_parsed_url() {
    // `isURLSameOrigin` in axios builds an anchor, assigns href and reads the
    // pieces back. An element that reflects only `href` returns undefined,
    // and the next line calls `.charAt(0)` on it.
    let out = render(
        r#"<body><div id="out"></div><script>
          const a = document.createElement('a');
          a.href = '/deep/path?q=1#frag';
          document.getElementById('out').textContent =
            [a.protocol, a.host, a.pathname, a.search, a.hash, a.origin].join(' ');
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html
            .contains("https: example.com /deep/path ?q=1 #frag https://example.com"),
        "a link exposes its URL in pieces: {}",
        out.html
    );
}

#[test]
fn adjacent_insertion_works_by_markup_and_by_node() {
    // `insertAdjacentHTML` is spelled with the acronym in capitals, which
    // camelCase renaming on the Rust side does not know. `insertAdjacentElement`
    // has to move the node it was handed, not a copy of its markup: the caller
    // keeps the reference and expects that one to be in the tree.
    let out = render(
        r#"<body><div id="host"><span id="anchor">x</span></div><div id="out"></div><script>
          const host = document.getElementById('host');
          host.insertAdjacentHTML('beforeend', '<b class="markup">B</b>');
          const made = document.createElement('i');
          made.textContent = 'I';
          host.insertAdjacentElement('afterbegin', made);
          document.getElementById('out').textContent =
            String(made.parentNode === null) + ' ' + host.firstChild.tagName + ' ' + host.lastChild.tagName;
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains(r#"<b class="markup">B</b>"#),
        "markup is parsed in place: {}",
        out.html
    );
    assert!(
        out.html.contains("false I B"),
        "the node handed in is the node inserted: {}",
        out.html
    );
}

#[test]
fn text_codecs_and_a_readable_body_round_trip() {
    // Anything that decodes a payload reaches for these, and a sanitiser
    // starts by asking for a blank document to parse into.
    let out = render(
        r#"<body><div id="out"></div><script>
          const bytes = new TextEncoder().encode('привет — ok');
          const back = new TextDecoder().decode(bytes);
          const doc = document.implementation.createHTMLDocument('t');
          doc.body.innerHTML = '<p>sanitised</p>';
          document.getElementById('out').textContent =
            [back, bytes.length, doc.body.textContent].join(' | ');
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains("привет — ok | 19 | sanitised"),
        "UTF-8 survives the round trip and a detached document parses: {}",
        out.html
    );
}

#[test]
fn an_iterator_helper_does_not_leak_the_items_it_rejected() {
    // `Iterator.prototype.find` in this build of QuickJS keeps a reference to
    // every item the predicate turned down. A leaked object is still live when
    // the runtime is freed, which aborts the process — so this test is really
    // checking that the page renders at all, and that the process survives to
    // report it. astro.build is one real page that does exactly this.
    let out = render(
        r#"<body><p><span>a</span><span>b</span></p><div id="out"></div><script>
          const found = document.querySelectorAll('span').values().find(e => e.textContent === 'b');
          const plain = [{ a: 1 }, { a: 2 }].values().find(x => x.a === 2);
          document.getElementById('out').textContent =
            (found ? found.textContent : 'none') + ' ' + plain.a + ' ' +
            String([1, 2, 3].values().find(x => x > 5));
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains("b 2 undefined"),
        "find returns the match, and undefined when there is none: {}",
        out.html
    );
}

#[test]
fn dom_string_arguments_are_coerced_the_way_the_dom_coerces() {
    // Every DOM method that takes a string takes ToString(value). Refusing a
    // number was the third most common script error across a 603-site corpus,
    // and it does not just lose the call — it throws out of the caller.
    let out = render(
        r#"<body><div id="d"></div><div id="out"></div><script>
          const d = document.getElementById('d');
          d.setAttribute('data-a', true);
          d.setAttribute('data-b', 42);
          d.setAttribute('data-c', null);
          d.className = 1;
          document.getElementById('out').textContent =
            [d.getAttribute('data-a'), d.getAttribute('data-b'),
             d.getAttribute('data-c'), d.className].join(' ');
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains("true 42 null 1"),
        "each value is stringified, not refused: {}",
        out.html
    );
}

#[test]
fn an_element_measures_what_its_css_says() {
    // Nothing is laid out. But a page that measures itself and finds zero
    // concludes it has no room and renders the collapsed branch, so the box
    // comes from the cascade — including rules in a stylesheet, which is more
    // than the inline attribute other engines in this class read.
    let out = render(
        r#"<html><head><style>
             .card { width: 320px; height: 180px }
             @media (min-width: 700px) { .wide { width: 640px } }
           </style></head><body>
           <div class="card" id="a">sheet</div>
           <div id="b" style="width:210px">inline</div>
           <div class="wide" id="c">media</div>
           <img id="d" width="300" height="150">
           <div id="e" style="display:none">hidden</div>
           <div id="out"></div><script>
             const w = (id) => Math.round(document.getElementById(id).getBoundingClientRect().width);
             document.getElementById('out').textContent =
               [w('a'), w('b'), w('c'), w('d'), w('e'),
                document.getElementById('a').offsetWidth,
                getComputedStyle(document.getElementById('a')).height].join(' ');
           </script></body></html>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains("320 210 640 300 0 320 180px"),
        "the sheet, the inline style, a matching media rule and the width \
         attribute all count; a hidden element measures zero: {}",
        out.html
    );
}

#[test]
fn the_names_a_framework_reaches_for_are_there() {
    // React's scheduler posts its work through a MessageChannel; a sanitiser
    // walks with a TreeWalker; everything tests with `instanceof`.
    let out = render(
        r#"<body><p><span>a</span><b>bee</b></p><div id="out"></div><script>
          const channel = new MessageChannel();
          const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
          let texts = 0;
          while (walker.nextNode()) texts++;
          const rect = new DOMRect(10, 20, 30, 40);
          document.getElementById('out').textContent = [
            document.querySelectorAll('span') instanceof NodeList,
            !!(channel.port1 && channel.port2),
            texts >= 2,
            rect.right === 40 && rect.bottom === 60,
            navigator instanceof Navigator,
            new DragEvent('drag') instanceof MouseEvent,
            getSelection() instanceof Selection,
          ].join(' ');
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains("true true true true true true true"),
        "every name answers for what it should: {}",
        out.html
    );
}

#[test]
fn a_page_waiting_on_the_network_is_not_a_settled_page() {
    // `fetch` no longer blocks, so the loop has to know the difference between
    // "nothing left to do" and "nothing left to do yet". Without that the page
    // is serialized the moment its last timer fires and the responses are lost.
    let net = StaticNetwork::new()
        .route("/one", 200, "application/json", "1")
        .route("/two", 200, "application/json", "2")
        .route("/three", 200, "application/json", "3");
    let out = render_with(
        net,
        r#"<body><div id="out">pending</div><script>
             Promise.all(['/one', '/two', '/three'].map((u) => fetch(u).then((r) => r.text())))
               .then((all) => { document.getElementById('out').textContent = 'got ' + all.join(''); });
           </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains("got 123"),
        "all three resolve before the page is declared settled: {}",
        out.html
    );
    assert_eq!(out.requests, 3);
}

#[test]
fn an_import_graph_is_fetched_and_run() {
    let net = StaticNetwork::new()
        .route(
            "/lib/greet.js",
            200,
            "text/javascript",
            "import { punct } from './punct.js';\
             export const greet = (who) => 'hello ' + who + punct;",
        )
        .route(
            "/lib/punct.js",
            200,
            "text/javascript",
            "export const punct = '!';",
        );

    let out = render_with(
        net,
        r#"<body><div id="out">empty</div><script type="module">
             import { greet } from '/lib/greet.js';
             document.getElementById('out').textContent = greet('world');
           </script></body>"#,
    );
    assert!(
        out.html.contains(">hello world!<"),
        "a module's own imports resolve against its URL: {}",
        out.html
    );
}

#[test]
fn a_bare_specifier_says_what_is_missing() {
    let out = render_with(
        StaticNetwork::new(),
        r#"<body><script type="module">import _ from 'lodash';</script></body>"#,
    );
    let reported = out
        .errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<String>();
    assert!(
        reported.contains("import map"),
        "a bare specifier is a missing feature, not a 404: {reported}"
    );
}

#[test]
fn a_module_that_throws_on_its_first_line_is_reported() {
    let out = render_with(
        StaticNetwork::new(),
        r#"<body><script type="module">throw new Error('module died');</script></body>"#,
    );
    let reported = out
        .errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<String>();
    assert!(
        reported.contains("module died"),
        "a module body runs inside a promise, and its rejection must not be silent: {reported}"
    );
}

#[test]
fn a_module_served_as_html_is_named_as_such() {
    let net = StaticNetwork::new().route("/app.js", 200, "text/html", "<!doctype html><h1>404");
    let out = render_with(
        net,
        r#"<body><script type="module">import '/app.js';</script></body>"#,
    );
    let reported = out
        .errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<String>();
    assert!(
        reported.contains("HTML"),
        "a soft 404 should say so rather than produce a wall of syntax errors: {reported}"
    );
}

#[test]
fn the_import_graph_draws_on_the_page_budget() {
    let net = StaticNetwork::new().route("/step", 200, "text/javascript", "export const x = 1;");
    let limits = Limits {
        max_requests: 2,
        ..Limits::default()
    };
    // Each import is a distinct URL, so none of them are deduplicated.
    let mut page = Page::new(
        r#"<body><script type="module">
             import '/step/1'; import '/step/2'; import '/step/3'; import '/step/4';
           </script></body>"#,
        "https://example.com/page",
        limits,
        net,
    )
    .expect("page builds");
    let out = page.run();
    let reported = out
        .errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<String>();
    assert!(
        reported.contains("budget"),
        "a page cannot spend more through imports than through fetch: {reported}"
    );
}

#[test]
fn a_script_the_page_inserts_runs_and_reports_load() {
    // Every webpack chunk and every tag manager arrives this way.
    let net = StaticNetwork::new().route(
        "/chunk.js",
        200,
        "text/javascript",
        "document.getElementById('out').textContent = 'chunk:' + document.currentScript.getAttribute('src');",
    );
    let out = render_with(
        net,
        r#"<body><div id="out"></div><script>
          const s = document.createElement('script');
          s.src = '/chunk.js';
          s.onload = () => { document.getElementById('out').textContent += ' loaded'; };
          document.head.appendChild(s);
          const inline = document.createElement('script');
          inline.textContent = 'window.__inline = 1';
          document.body.appendChild(inline);
          const markup = document.createElement('div');
          markup.innerHTML = '<script>window.__markup = 1<\/script>';
          document.body.appendChild(markup);
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains("chunk:/chunk.js loaded"),
        "the inserted script ran with currentScript set, then fired load: {}",
        out.html
    );
}

#[test]
fn a_submitted_form_is_a_post_navigation() {
    let out = render(
        r#"<body><form method="post" action="/login">
          <input name="user" value="ann"><input name="remember" type="checkbox" checked>
          <input name="skip" type="checkbox"><select name="lang"><option value="ru" selected>ru</option></select>
        </form><script>document.querySelector('form').submit();</script></body>"#,
    );
    let nav = out
        .requested_navigation
        .expect("the submit asked to navigate");
    assert_eq!(nav.method, "POST");
    assert_eq!(nav.url, "https://example.com/login");
    assert_eq!(nav.body.as_deref(), Some("user=ann&remember=on&lang=ru"));
}

#[test]
fn a_node_is_one_object_and_attributes_are_a_named_map() {
    let out = render(
        r#"<body><div id="out"></div><p id="a"></p><p id="b"></p><script>
          const a = document.getElementById('a'), b = document.getElementById('b');
          a.__mine = 'kept';
          const facts = [
            a.parentNode === b.parentNode,
            document.querySelector('#a').__mine === 'kept',
            a.getRootNode() === document,
            a.constructor.name,
            document.body instanceof HTMLBodyElement && !(a instanceof HTMLBodyElement),
          ];
          a.setAttribute('onsubmit', 't');
          facts.push(a.attributes.onsubmit.value, a.attributes.length, a.attributes[0].name);
          a.removeAttributeNode(a.getAttributeNode('onsubmit'));
          facts.push(a.hasAttribute('onsubmit'));
          const frag = document.createDocumentFragment();
          frag.appendChild(document.createElement('i'));
          frag.appendChild(document.createElement('u'));
          b.appendChild(frag);
          facts.push(frag.nodeType, b.childNodes.length, frag.childNodes.length);
          document.getElementById('out').textContent = facts.join('|');
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html
            .contains("true|true|true|HTMLParagraphElement|true|t|2|id|false|11|2|0"),
        "identity, prototypes, attributes and fragments behave: {}",
        out.html
    );
}

#[test]
fn an_import_map_gives_a_bare_specifier_a_url() {
    let net = StaticNetwork::new().route(
        "/vendor/greet.js",
        200,
        "text/javascript",
        "export const greet = (n) => 'hello ' + n;",
    );
    let out = render_with(
        net,
        r#"<head><script type="importmap">{"imports": {"greet": "/vendor/greet.js"}}</script></head>
        <body><div id="out"></div><script type="module">
          import { greet } from 'greet';
          document.getElementById('out').textContent = greet(import.meta.url);
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains("hello https://example.com/page"),
        "the map resolved the specifier and import.meta.url is the page: {}",
        out.html
    );
}

#[test]
fn a_template_has_content_and_a_parsed_document_has_a_body() {
    let out = render(
        r#"<body><template id="t"><p class="x">hi</p></template><div id="out"></div><script>
          const t = document.getElementById('t');
          const parsed = new DOMParser().parseFromString('<p>parsed</p>', 'text/html');
          document.getElementById('out').textContent = [
            t.content instanceof DocumentFragment,
            t.content.cloneNode(true).querySelector('.x').textContent,
            parsed.body.textContent,
            document.createElement('canvas').getContext('2d') !== null,
            typeof document.createElement('iframe').contentWindow.document.write,
          ].join('|');
        </script></body>"#,
    );
    assert!(out.errors.is_empty(), "no script errors: {:?}", out.errors);
    assert!(
        out.html.contains("true|hi|parsed|true|function"),
        "templates, DOMParser, canvas and frames answer: {}",
        out.html
    );
}

#[test]
fn a_call_as_a_for_in_target_parses_and_throws_only_when_assigned() {
    // servicepipe's bot check does `for (f() in [])` inside try/catch to
    // tell a browser from another engine. V8 parses it, runs the call, and
    // throws only when there is a key to assign; QuickJS refused to parse it
    // and the whole script died. The vendored QuickJS carries the fix.
    let out = render(
        r#"<body><div id="out"></div><script>
          var calls = 0;
          function f() { calls++; }
          var facts = [];
          try { for (f() in []); facts.push('empty ok'); } catch (e) { facts.push('empty threw ' + e.name); }
          try { for (f() in {a: 1}); facts.push('keyed ok'); } catch (e) { facts.push('keyed ' + e.name); }
          try { for (f() of [1]); facts.push('of ok'); } catch (e) { facts.push('of ' + e.name); }
          facts.push('calls ' + calls);
          document.getElementById('out').textContent = facts.join('|');
        </script></body>"#,
    );
    assert!(
        out.errors.is_empty(),
        "the script itself parses: {:?}",
        out.errors
    );
    assert!(
        out.html
            .contains("empty ok|keyed ReferenceError|of ReferenceError|calls 2"),
        "V8's behaviour, exactly: {}",
        out.html
    );
}

#[test]
fn a_worker_runs_on_this_thread_and_answers() {
    // A bot check hashes in a worker and posts the result back; a page keeps
    // its search index in one. Both need the reply, on a later turn.
    let net = StaticNetwork::new()
        .route("/helper.js", 200, "text/javascript", "function twice(n) { return n * 2; }")
        .route(
            "/worker.js",
            200,
            "text/javascript",
            "importScripts('/helper.js'); onmessage = (e) => postMessage({ n: twice(e.data), win: typeof window });",
        );
    let out = render_with(
        net,
        r#"<body><div id="out"></div><script>
          const facts = [];
          const blob = new Blob(['self.onmessage = (e) => { let h = 0; for (let i = 0; i < e.data; i++) h = (h * 31 + i) >>> 0; postMessage(h); }']);
          const w = new Worker(URL.createObjectURL(blob));
          w.onmessage = (e) => { facts.push('blob:' + e.data); done(); };
          w.postMessage(100);
          const w2 = new Worker('/worker.js');
          w2.addEventListener('message', (e) => { facts.push('url:' + e.data.n + ':' + e.data.win); done(); });
          w2.postMessage(21);
          const w3 = new Worker(URL.createObjectURL(new Blob(['onmessage = () => { throw new Error("boom"); }'])));
          w3.onerror = (e) => { facts.push('error:' + e.message); done(); };
          w3.postMessage(0);
          function done() { if (facts.length === 3) document.getElementById('out').textContent = facts.sort().join('|'); }
        </script></body>"#,
    );
    assert!(
        out.html
            .contains("blob:58926130|error:boom|url:42:undefined"),
        "workers compute, import scripts, hide window, and report errors: {}",
        out.html
    );
}

#[test]
fn a_component_hears_its_attributes_change_and_keeps_the_prototype_it_chose() {
    let out = render(
        r#"<!doctype html><html><body>
        <x-counter count="1"></x-counter><x-shimmed></x-shimmed>
        <script>
          const seen = [];
          class Counter extends HTMLElement {
            static get observedAttributes() { return ['count']; }
            attributeChangedCallback(name, before, after) { seen.push(`${name}:${before}>${after}`); }
          }
          customElements.define('x-counter', Counter);
          const el = document.querySelector('x-counter');
          el.setAttribute('count', '2');
          el.setAttribute('other', 'x');
          el.removeAttribute('count');
          el.toggleAttribute('count');
          // An ES5 shim's stand-in: super() puts the element on the stand-in's
          // prototype, the stand-in moves it to the transpiled class's, and
          // that choice has to survive the upgrade.
          function Legacy() {}
          Legacy.prototype = Object.create(HTMLElement.prototype);
          Legacy.prototype.constructor = Legacy;
          Legacy.prototype.greet = function () { return 'hi from ' + this.tagName.toLowerCase(); };
          Legacy.prototype.connectedCallback = function () { seen.push(this.greet()); };
          const proto = Legacy.prototype;
          class StandIn extends HTMLElement {
            constructor() { super(); Object.setPrototypeOf(this, proto); Legacy.call(this); }
          }
          StandIn.prototype.connectedCallback = proto.connectedCallback;
          customElements.define('x-shimmed', StandIn);
          const shimmed = document.querySelector('x-shimmed');
          seen.push('proto:' + (Object.getPrototypeOf(shimmed) === proto));
          document.body.setAttribute('data-seen', seen.join('|'));
        </script></body></html>"#,
    );
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert!(
        out.html.contains(
            r#"data-seen="count:null>1|count:1>2|count:2>null|count:null>|hi from x-shimmed|proto:true""#
        ),
        "{}",
        out.html
    );
}

#[test]
fn members_live_on_the_prototypes_a_browser_keeps_them_on() {
    // Polyfills read descriptors off Element.prototype and Node.prototype
    // and patch them there; ShadyDOM does, and YouTube forces it on.
    let out = render(
        r#"<!doctype html><html><body><p id="p">text</p>
        <script>
          const own = (c, n) => Object.prototype.hasOwnProperty.call(c.prototype, n);
          const p = document.getElementById('p');
          const text = p.firstChild;
          const facts = [
            own(Element, 'innerHTML'), !own(Node, 'innerHTML'), own(Node, 'firstChild'),
            own(HTMLElement, 'innerText'), own(Document, 'querySelector'), own(Element, 'setAttribute'),
            !('innerHTML' in text), typeof text.remove === 'function',
            document.querySelector('p') === p, typeof document.createDocumentFragment().querySelectorAll === 'function',
            p.getAttribute('nope') === null, p.nodeValue === null, text.nodeValue === 'text',
            document.hidden === false, typeof document.onclick === 'object',
          ];
          facts.push(
            !('value' in document.createElement('div')), 'value' in document.createElement('input'),
            'src' in document.createElement('img'), !('src' in document.createElement('a')),
            typeof Object.getOwnPropertyDescriptor(HTMLScriptElement.prototype, 'src').get === 'function',
            typeof document.createElement('video').play === 'function',
            typeof document.createElement('audio').canPlayType === 'function',
            document.createElement('template').content.nodeType === 11,
            document.createElement('meta').content === '',
            document.createElement('a').host === '',
          );
          const d = Object.getOwnPropertyDescriptor(Element.prototype, 'innerHTML');
          Object.defineProperty(Element.prototype, 'innerHTML', {
            get() { return 'patched:' + d.get.call(this); }, set: d.set, configurable: true,
          });
          facts.push(p.innerHTML === 'patched:text');
          p.setAttribute('data-facts', facts.map((f, i) => f ? '' : i).filter(Boolean).join(',') || 'all');
        </script></body></html>"#,
    );
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert!(
        out.html.contains(r#"data-facts="all""#),
        "failed facts: {}",
        out.html
    );
}

#[test]
fn a_template_parsed_from_markup_owns_its_content() {
    // Polymer walks the templates a component's markup contains, and a
    // template that arrived through `innerHTML` used to name a fragment in
    // the scratch document it was parsed in — an id that meant some other
    // node in the live one.
    let out = render(
        r#"<!doctype html><html><body><div id="host"></div>
        <script>
          const host = document.getElementById('host');
          host.innerHTML = '<template id="t"><i>z</i></template><b>after</b>';
          const t = host.firstChild;
          const clone = t.cloneNode(true);
          clone.content.firstChild.textContent = 'changed';
          // A fragment has no `innerHTML` in a browser either; read the child.
          const facts = [
            t.content.nodeType === 11, t.content.firstChild.outerHTML === '<i>z</i>',
            clone.content !== t.content, clone.content.firstChild.outerHTML === '<i>changed</i>',
            t.content.firstChild.outerHTML === '<i>z</i>', !('innerHTML' in t.content),
          ];
          host.setAttribute('data-facts', facts.map((f, i) => f ? '' : i).filter(Boolean).join(',') || 'all');
        </script></body></html>"#,
    );
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert!(
        out.html.contains(r#"data-facts="all""#),
        "failed facts: {}",
        out.html
    );
}
