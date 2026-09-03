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
fn navigation_is_reported_not_followed() {
    let out = render(r#"<body><script>location.href = '/next?a=1';</script></body>"#);
    assert_eq!(
        out.requested_navigation.as_deref(),
        Some("https://example.com/next?a=1"),
        "resolved against the page URL, and not acted on"
    );
    assert_eq!(out.requests, 0);
}

#[test]
fn reload_is_a_navigation_to_the_same_url() {
    let out = render(r#"<body><script>location.reload();</script></body>"#);
    assert_eq!(
        out.requested_navigation.as_deref(),
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
