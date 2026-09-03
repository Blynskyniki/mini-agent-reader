//! A CDP session, driven the way a client drives one, without a network.
//!
//! Documents are loaded with `Page.setDocumentContent` and every request the
//! page makes is either intercepted by the fake client below or refused for
//! being cross-origin, so nothing here opens a socket.

use mar_cdp::browser::Browser;
use mar_cdp::domains::dispatch;
use mar_cdp::fetch::PauseChannel;
use mar_cdp::protocol::{Command, Outgoing};
use mar_js::Limits;
use mar_net::{ClientConfig, HttpClient};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

/// A client that has already decided everything it will say.
struct Scripted {
    answers: VecDeque<Command>,
    sent: Rc<RefCell<Vec<Value>>>,
}

impl PauseChannel for Scripted {
    fn send(&mut self, message: &Outgoing) {
        self.sent
            .borrow_mut()
            .push(serde_json::to_value(message).expect("outgoing serializes"));
    }

    fn next_command(&mut self, _deadline: Instant) -> Option<Command> {
        self.answers.pop_front()
    }
}

fn command(method: &str, params: Value) -> Command {
    Command {
        id: 1,
        method: method.to_owned(),
        params,
        session_id: None,
    }
}

/// A browser with one page, sitting at `url` and answering with `answers`.
fn session(url: &str, answers: Vec<Command>) -> (Browser, Rc<RefCell<Vec<Value>>>) {
    // The HTTP client is built but never reached: every test either loads its
    // document directly or answers the request itself.
    let client = HttpClient::new(ClientConfig::default());
    let mut browser = Browser::new(client, Limits::default(), "test".into());
    browser.create_target(None);
    browser.targets[0].url = url.to_owned();

    let sent = Rc::new(RefCell::new(Vec::new()));
    browser
        .fetch
        .borrow_mut()
        .attach(Rc::new(RefCell::new(Scripted {
            answers: answers.into(),
            sent: sent.clone(),
        })));
    (browser, sent)
}

fn result(reply: &Outgoing) -> Value {
    match reply {
        Outgoing::Result { result, .. } => result.clone(),
        other => panic!("expected a result, got {other:?}"),
    }
}

fn error_text(reply: &Outgoing) -> String {
    match reply {
        Outgoing::Error { error, .. } => error.message.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn load(browser: &mut Browser, html: &str) {
    let reply = dispatch(
        browser,
        &command("Page.setDocumentContent", json!({"html": html})),
    );
    result(&reply.response);
}

/// Evaluate an expression in the page and return it by value.
fn evaluate(browser: &mut Browser, expression: &str) -> Value {
    let reply = dispatch(
        browser,
        &command(
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true}),
        ),
    );
    result(&reply.response)
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn events(sent: &Rc<RefCell<Vec<Value>>>, method: &str) -> Vec<Value> {
    sent.borrow()
        .iter()
        .filter(|m| m.get("method").and_then(Value::as_str) == Some(method))
        .cloned()
        .collect()
}

const FETCHING_PAGE: &str = r#"<body><main>waiting</main><script>
    fetch('/api/items')
      .then(r => r.text())
      .then(t => { document.querySelector('main').textContent = t; })
      .catch(e => { document.querySelector('main').textContent = 'ERR ' + e.message; });
  </script></body>"#;

fn enable_interception(browser: &mut Browser, pattern: &str) {
    dispatch(browser, &command("Network.enable", json!({})));
    let reply = dispatch(
        browser,
        &command(
            "Fetch.enable",
            json!({"patterns": [{"urlPattern": pattern}]}),
        ),
    );
    result(&reply.response);
}

#[test]
fn a_fulfilled_request_never_reaches_the_network() {
    let answer = command(
        "Fetch.fulfillRequest",
        json!({
            "requestId": "interception-1",
            "responseCode": 200,
            "body": "bW9ja2Vk",
        }),
    );
    let (mut browser, sent) = session("https://example.com/", vec![answer]);
    enable_interception(&mut browser, "*");
    load(&mut browser, FETCHING_PAGE);

    assert_eq!(
        evaluate(&mut browser, "document.querySelector('main').textContent"),
        "mocked"
    );
    let paused = events(&sent, "Fetch.requestPaused");
    assert_eq!(paused.len(), 1, "one request was announced");
    assert_eq!(
        paused[0]["params"]["request"]["url"],
        "https://example.com/api/items"
    );
    assert_eq!(paused[0]["params"]["resourceType"], "XHR");
}

#[test]
fn a_failed_request_reports_the_reason_to_the_page() {
    let answer = command(
        "Fetch.failRequest",
        json!({"requestId": "interception-1", "errorReason": "BlockedByClient"}),
    );
    let (mut browser, _sent) = session("https://example.com/", vec![answer]);
    enable_interception(&mut browser, "*");
    load(&mut browser, FETCHING_PAGE);

    let text = evaluate(&mut browser, "document.querySelector('main').textContent");
    assert!(
        text.as_str().is_some_and(|t| t.contains("BlockedByClient")),
        "the page saw why: {text}"
    );
}

#[test]
fn a_pause_nobody_answers_fails_the_request_and_the_page_carries_on() {
    // No answers at all: the client enabled interception and then went quiet.
    let (mut browser, sent) = session("https://example.com/", Vec::new());
    enable_interception(&mut browser, "*");

    let started = Instant::now();
    load(&mut browser, FETCHING_PAGE);

    assert_eq!(events(&sent, "Fetch.requestPaused").len(), 1);
    let text = evaluate(&mut browser, "document.querySelector('main').textContent");
    assert!(
        text.as_str().is_some_and(|t| t.contains("TimedOut")),
        "the request was failed, not left hanging: {text}"
    );
    assert!(
        started.elapsed().as_millis() < Limits::default().wall_ms as u128 + 2_000,
        "and the page finished inside its budget"
    );
}

#[test]
fn a_pattern_chooses_which_requests_pause() {
    let answer = command(
        "Fetch.fulfillRequest",
        json!({"requestId": "interception-1", "responseCode": 200, "body": "b2s="}),
    );
    let (mut browser, sent) = session("https://example.com/", vec![answer]);
    enable_interception(&mut browser, "*/api/*");
    load(
        &mut browser,
        r#"<body><main></main><script>
             const log = t => { document.querySelector('main').textContent += t; };
             fetch('/api/items').then(r => r.text()).then(t => log('api:' + t + ';'));
             fetch('https://cdn.example.com/other')
               .then(() => log('cdn:ok;'))
               .catch(() => log('cdn:refused;'));
           </script></body>"#,
    );

    // The refusal resolves first: it never left the process.
    let text = evaluate(&mut browser, "document.querySelector('main').textContent");
    assert_eq!(text, "cdn:refused;api:ok;");
    let paused = events(&sent, "Fetch.requestPaused");
    assert_eq!(paused.len(), 1, "only the matching request paused");
    assert_eq!(
        paused[0]["params"]["request"]["url"],
        "https://example.com/api/items"
    );
}

#[test]
fn commands_that_arrive_during_a_pause_are_put_aside() {
    let (mut browser, _sent) = session(
        "https://example.com/",
        vec![
            command("Runtime.evaluate", json!({"expression": "1 + 1"})),
            command(
                "Fetch.fulfillRequest",
                json!({"requestId": "interception-1", "responseCode": 200, "body": "b2s="}),
            ),
        ],
    );
    enable_interception(&mut browser, "*");
    load(&mut browser, FETCHING_PAGE);

    let deferred = &browser.fetch.borrow().deferred;
    assert_eq!(
        deferred.len(),
        1,
        "the evaluate waits for the page to settle"
    );
    assert_eq!(deferred[0].method, "Runtime.evaluate");
}

#[test]
fn a_verdict_for_a_request_that_is_not_paused_is_refused() {
    let (mut browser, _sent) = session("https://example.com/", Vec::new());
    let reply = dispatch(
        &mut browser,
        &command(
            "Fetch.continueRequest",
            json!({"requestId": "interception-9"}),
        ),
    );
    assert_eq!(error_text(&reply.response), "no request is paused");
}

#[test]
fn intercepting_a_response_says_so_rather_than_doing_nothing() {
    let (mut browser, _sent) = session("https://example.com/", Vec::new());
    let reply = dispatch(
        &mut browser,
        &command(
            "Fetch.enable",
            json!({"patterns": [{"urlPattern": "*", "requestStage": "Response"}]}),
        ),
    );
    assert!(
        error_text(&reply.response).contains("requestStage"),
        "the client is told which half is missing"
    );
}

#[test]
fn a_response_body_is_kept_for_the_client_to_read() {
    let answer = command(
        "Fetch.fulfillRequest",
        json!({"requestId": "interception-1", "responseCode": 200, "body": "bW9ja2Vk"}),
    );
    let (mut browser, sent) = session("https://example.com/", vec![answer]);
    enable_interception(&mut browser, "*");
    load(&mut browser, FETCHING_PAGE);

    // The client learns the request id from the events, as it would in Chrome.
    let announced = events(&sent, "Network.requestWillBeSent");
    assert_eq!(announced.len(), 1);
    let request_id = announced[0]["params"]["requestId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(events(&sent, "Network.loadingFinished").len(), 1);

    let reply = dispatch(
        &mut browser,
        &command("Network.getResponseBody", json!({"requestId": request_id})),
    );
    assert_eq!(
        result(&reply.response),
        json!({"body": "mocked", "base64Encoded": false})
    );

    let missing = dispatch(
        &mut browser,
        &command("Network.getResponseBody", json!({"requestId": "nothing"})),
    );
    assert!(error_text(&missing.response).contains("no body retained"));
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

const CLICKABLE: &str = r#"<body>
    <button id="one">one</button>
    <button id="two">two</button>
    <form id="form"><input id="field" value=""></form>
    <div id="log"></div>
    <script>
      const log = document.getElementById('log');
      for (const id of ['one', 'two']) {
        document.getElementById(id).addEventListener('click', e => {
          log.textContent += 'click:' + e.target.id + '@' + e.clientX + ',' + e.clientY + ';';
        });
      }
      document.getElementById('field').addEventListener('input', e => {
        log.textContent += 'input:' + e.target.value + ';';
      });
      document.getElementById('form').addEventListener('submit', e => {
        e.preventDefault();
        log.textContent += 'submit:' + document.getElementById('field').value + ';';
      });
    </script></body>"#;

/// The node id a client would hold for `selector`.
fn query(browser: &mut Browser, selector: &str) -> i64 {
    let reply = dispatch(browser, &command("DOM.getDocument", json!({})));
    result(&reply.response);
    let reply = dispatch(
        browser,
        &command(
            "DOM.querySelector",
            json!({"nodeId": 1, "selector": selector}),
        ),
    );
    result(&reply.response)["nodeId"]
        .as_i64()
        .expect("a node id")
}

/// The centre of a node's box, the way a client works it out.
fn centre(browser: &mut Browser, node_id: i64) -> (f64, f64) {
    let reply = dispatch(
        browser,
        &command("DOM.getContentQuads", json!({"nodeId": node_id})),
    );
    let quads = result(&reply.response);
    let quad: Vec<f64> = quads["quads"][0]
        .as_array()
        .expect("one quad")
        .iter()
        .map(|n| n.as_f64().unwrap())
        .collect();
    let x = (quad[0] + quad[2] + quad[4] + quad[6]) / 4.0;
    let y = (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0;
    (x, y)
}

fn click(browser: &mut Browser, x: f64, y: f64) {
    for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
        let reply = dispatch(
            browser,
            &command(
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1}),
            ),
        );
        result(&reply.response);
    }
}

#[test]
fn a_click_lands_on_the_element_the_client_measured() {
    let (mut browser, _sent) = session("https://example.com/", Vec::new());
    load(&mut browser, CLICKABLE);

    let two = query(&mut browser, "#two");
    let (x, y) = centre(&mut browser, two);
    click(&mut browser, x, y);

    let log = evaluate(&mut browser, "document.getElementById('log').textContent");
    assert_eq!(
        log,
        format!("click:two@{x},{y};"),
        "the second button, and it saw the coordinates"
    );
}

#[test]
fn every_element_gets_a_box_of_its_own() {
    let (mut browser, _sent) = session("https://example.com/", Vec::new());
    load(&mut browser, CLICKABLE);

    let one = query(&mut browser, "#one");
    let two = query(&mut browser, "#two");
    assert_ne!(
        centre(&mut browser, one),
        centre(&mut browser, two),
        "two elements, two boxes"
    );
    // Asking twice gives the same answer, or a client's second click would
    // land somewhere else.
    assert_eq!(centre(&mut browser, one), centre(&mut browser, one));
}

#[test]
fn the_page_itself_still_sees_zero_sized_boxes() {
    let (mut browser, _sent) = session("https://example.com/", Vec::new());
    load(
        &mut browser,
        r#"<body><p id="p">x</p><script>
             window.measured = document.getElementById('p').getBoundingClientRect().width;
             window.rects = document.getElementById('p').getClientRects().length;
           </script></body>"#,
    );
    assert_eq!(evaluate(&mut browser, "window.measured"), 0.0);
    assert_eq!(evaluate(&mut browser, "window.rects"), 0.0);

    // The client asking the same question gets a box it can click.
    assert_ne!(
        evaluate(
            &mut browser,
            "document.getElementById('p').getBoundingClientRect().width"
        ),
        0.0,
        "the ruler exists in the client's hands only"
    );
}

#[test]
fn the_layout_viewport_covers_the_boxes_handed_out() {
    let (mut browser, _sent) = session("https://example.com/", Vec::new());
    load(&mut browser, CLICKABLE);
    let reply = dispatch(&mut browser, &command("Page.getLayoutMetrics", json!({})));
    let metrics = result(&reply.response);
    let (w, h) = (
        metrics["cssLayoutViewport"]["clientWidth"]
            .as_f64()
            .unwrap(),
        metrics["cssLayoutViewport"]["clientHeight"]
            .as_f64()
            .unwrap(),
    );

    let one = query(&mut browser, "#one");
    let (x, y) = centre(&mut browser, one);
    assert!(
        x < w && y < h,
        "a click point a client computes is inside it"
    );
}

#[test]
fn typing_reaches_the_focused_element() {
    let (mut browser, _sent) = session("https://example.com/", Vec::new());
    load(&mut browser, CLICKABLE);

    let field = query(&mut browser, "#field");
    let reply = dispatch(
        &mut browser,
        &command("DOM.focus", json!({"nodeId": field})),
    );
    result(&reply.response);

    for key in ["h", "i"] {
        for kind in ["keyDown", "keyUp"] {
            let reply = dispatch(
                &mut browser,
                &command(
                    "Input.dispatchKeyEvent",
                    json!({"type": kind, "key": key, "text": key}),
                ),
            );
            result(&reply.response);
        }
    }
    for key in ["Backspace", "Enter"] {
        let reply = dispatch(
            &mut browser,
            &command(
                "Input.dispatchKeyEvent",
                json!({"type": "keyDown", "key": key}),
            ),
        );
        result(&reply.response);
    }

    assert_eq!(
        evaluate(&mut browser, "document.getElementById('field').value"),
        "h"
    );
    assert_eq!(
        evaluate(&mut browser, "document.getElementById('log').textContent"),
        // Enter in a single-line field submits, as it does in a browser.
        "input:h;input:hi;input:h;submit:h;"
    );
}

#[test]
fn a_click_that_lands_on_nothing_is_not_an_error() {
    let (mut browser, _sent) = session("https://example.com/", Vec::new());
    load(&mut browser, CLICKABLE);
    click(&mut browser, 9_000.0, 9_000.0);
    assert_eq!(
        evaluate(&mut browser, "document.getElementById('log').textContent"),
        ""
    );
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

#[test]
fn cookies_round_trip_through_the_storage_domain() {
    let (mut browser, _sent) = session("https://example.com/", Vec::new());
    let reply = dispatch(
        &mut browser,
        &command(
            "Storage.setCookies",
            json!({"cookies": [{"name": "sid", "value": "42", "domain": "example.com"}]}),
        ),
    );
    result(&reply.response);

    // A cookie set before the page exists is waiting for it when it loads.
    load(
        &mut browser,
        r#"<body><script>document.cookie = 'theme=dark';</script></body>"#,
    );
    assert_eq!(
        evaluate(&mut browser, "document.cookie"),
        "sid=42; theme=dark"
    );

    let reply = dispatch(&mut browser, &command("Storage.getCookies", json!({})));
    let cookies = result(&reply.response);
    let names: Vec<&str> = cookies["cookies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["sid", "theme"],
        "including what the page set itself"
    );
    assert_eq!(cookies["cookies"][0]["domain"], "example.com");
    assert_eq!(cookies["cookies"][0]["value"], "42");

    let reply = dispatch(
        &mut browser,
        &command(
            "Storage.deleteCookies",
            json!({"cookies": [{"name": "sid"}]}),
        ),
    );
    result(&reply.response);
    assert_eq!(evaluate(&mut browser, "document.cookie"), "theme=dark");

    let reply = dispatch(&mut browser, &command("Storage.clearCookies", json!({})));
    result(&reply.response);
    assert_eq!(evaluate(&mut browser, "document.cookie"), "");
}

#[test]
fn methods_that_need_the_renderer_still_refuse() {
    let (mut browser, _sent) = session("https://example.com/", Vec::new());
    let reply = dispatch(&mut browser, &command("Page.captureScreenshot", json!({})));
    assert!(error_text(&reply.response).contains("no renderer"));
}

// -- teardown ---------------------------------------------------------------

/// What a client's `page.evaluate` looks like when it sets up an observer.
const OBSERVING: &str = "(() => { const mo = new MutationObserver(() => {}); \
     mo.observe(document.body, { childList: true }); return 1 })()";

#[test]
fn closing_a_page_that_evaluated_an_observer_keeps_the_server_up() {
    // The evaluation runs after the page settled, so the timer the observer
    // queues is still queued when the target is closed and the page torn
    // down. That teardown used to abort the whole process.
    let (mut browser, _sent) = session("https://example.com/", vec![]);
    load(&mut browser, "<body><p>hello</p></body>");
    assert_eq!(evaluate(&mut browser, OBSERVING), json!(1));

    let target_id = browser.targets[0].id.clone();
    let reply = dispatch(
        &mut browser,
        &command("Target.closeTarget", json!({"targetId": target_id})),
    );
    assert_eq!(result(&reply.response), json!({"success": true}));
    assert!(browser.targets.is_empty());
}

#[test]
fn a_new_document_and_a_dropped_connection_release_the_page_the_same_way() {
    let (mut browser, _sent) = session("https://example.com/", vec![]);
    load(&mut browser, "<body><p>one</p></body>");
    assert_eq!(evaluate(&mut browser, OBSERVING), json!(1));
    // A new document replaces the page that was observing.
    load(&mut browser, "<body><p>two</p></body>");
    assert_eq!(evaluate(&mut browser, OBSERVING), json!(1));
    // The connection goes away with the page still open.
    drop(browser);
}
