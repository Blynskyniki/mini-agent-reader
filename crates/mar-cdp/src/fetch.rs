//! Request interception, spliced into the network seam.
//!
//! The engine never opens a socket: it calls a `NetworkProvider` the host
//! installs. That is the only place every request passes through, so it is
//! where a client's `Fetch` patterns are applied — a request that matches is
//! announced, and then nothing happens until the client says what to do with
//! it.
//!
//! Waiting is the awkward part. The settle loop is synchronous and runs inside
//! command dispatch, so while a page renders, nobody is reading the socket. A
//! paused request therefore borrows the connection: it pushes the event and
//! then reads commands itself until the answer arrives. Commands that are not
//! that answer are put aside for the dispatch loop to run afterwards, because
//! answering them would need the browser this call stack is already inside.
//! The wait is bounded by the page's own wall-clock budget: a client that never
//! answers costs one page, not the process.

use crate::protocol::{Command, Outgoing};
use mar_js::{HttpRequest, HttpResponse, NetworkProvider};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

/// Response bodies retained for `Network.getResponseBody`, and the ceiling on
/// each one. A client asks for a body it just saw go past, so a short window is
/// enough, and an unbounded one would hold every page a session ever loaded.
pub const MAX_BODIES: usize = 16;
const MAX_BODY_BYTES: usize = 256 * 1024;

/// How a paused request talks to the client while the settle loop is blocked.
pub trait PauseChannel {
    fn send(&mut self, message: &Outgoing);
    /// The next command from the client, or None once `deadline` has passed.
    fn next_command(&mut self, deadline: Instant) -> Option<Command>;
}

/// What the client decided about a paused request.
#[derive(Debug, Clone)]
pub enum Verdict {
    /// Send it, with any of the request rewritten.
    Continue {
        url: Option<String>,
        method: Option<String>,
        headers: Option<Vec<(String, String)>>,
        body: Option<String>,
    },
    /// Answer it here, without touching the network.
    Fulfill {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    /// Refuse it.
    Fail { reason: String },
}

impl Verdict {
    /// The verdict for a request nobody asked to see.
    fn untouched() -> Self {
        Verdict::Continue {
            url: None,
            method: None,
            headers: None,
            body: None,
        }
    }

    /// Rewrite a request the way a `continueRequest` asked for.
    pub fn apply(&self, request: &mut HttpRequest) {
        if let Verdict::Continue {
            url,
            method,
            headers,
            body,
        } = self
        {
            if let Some(url) = url {
                request.url = url.clone();
            }
            if let Some(method) = method {
                request.method = method.clone();
            }
            if let Some(headers) = headers {
                request.headers = headers.clone();
            }
            if let Some(body) = body {
                request.body = Some(body.clone());
            }
        }
    }
}

/// One `Fetch.RequestPattern`.
#[derive(Debug, Clone)]
pub struct Pattern {
    url: String,
    resource_type: Option<String>,
}

impl Pattern {
    fn matches(&self, url: &str, kind: &str) -> bool {
        match &self.resource_type {
            Some(wanted) if !wanted.eq_ignore_ascii_case(kind) => false,
            _ => glob_matches(&self.url, url),
        }
    }
}

/// Chrome's pattern syntax: `*` for any run of characters, `?` for one.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    // Where the last `*` was, and how much of the text it had eaten, so a
    // failed match can give it one more character and try again.
    let (mut star, mut eaten) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            eaten = ti;
            pi += 1;
        } else if let Some(s) = star {
            eaten += 1;
            ti = eaten;
            pi = s + 1;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// A request the client was told about and has not answered yet.
struct Paused {
    interception_id: String,
    verdict: Option<Verdict>,
}

/// Everything the interception surface holds for one connection.
#[derive(Default)]
pub struct Desk {
    pub enabled: bool,
    patterns: Vec<Pattern>,
    /// The session `Fetch.enable` arrived on. Events are routed back there.
    session: Option<String>,
    /// Whether the client subscribed to `Network`. Chrome only reports requests
    /// to a client that asked for them, and Puppeteer correlates a paused
    /// request with the `Network.requestWillBeSent` that preceded it.
    pub network_enabled: bool,
    counter: u64,
    paused: Option<Paused>,
    /// Commands that arrived while a request was paused, in arrival order.
    pub deferred: VecDeque<Command>,
    /// Response bodies, newest last.
    bodies: VecDeque<(String, String)>,
    channel: Option<Rc<RefCell<dyn PauseChannel>>>,
}

impl Desk {
    pub fn shared() -> Rc<RefCell<Desk>> {
        Rc::new(RefCell::new(Desk::default()))
    }

    /// Install the connection a paused request may borrow.
    pub fn attach(&mut self, channel: Rc<RefCell<dyn PauseChannel>>) {
        self.channel = Some(channel);
    }

    /// `Fetch.enable`. An empty pattern list means every request, as in Chrome.
    pub fn enable(&mut self, params: &Value, session: Option<String>) -> Result<(), String> {
        let mut patterns = Vec::new();
        if let Some(list) = params.get("patterns").and_then(Value::as_array) {
            for entry in list {
                // Pausing a response means holding its body until the client
                // has looked at it, which is a second interception point this
                // does not have. Saying so beats never pausing and never
                // explaining why.
                if entry
                    .get("requestStage")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.eq_ignore_ascii_case("Response"))
                {
                    return Err(
                        "Fetch.enable: requestStage 'Response' is not supported, only 'Request'"
                            .into(),
                    );
                }
                patterns.push(Pattern {
                    url: entry
                        .get("urlPattern")
                        .and_then(Value::as_str)
                        .unwrap_or("*")
                        .to_owned(),
                    resource_type: entry
                        .get("resourceType")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
        }
        if patterns.is_empty() {
            patterns.push(Pattern {
                url: "*".into(),
                resource_type: None,
            });
        }
        self.patterns = patterns;
        self.session = session;
        self.enabled = true;
        Ok(())
    }

    pub fn disable(&mut self) {
        self.enabled = false;
        self.patterns.clear();
        self.paused = None;
    }

    /// `Network.enable`, which is what turns the request reporting on.
    pub fn observe(&mut self, on: bool, session: Option<String>) {
        self.network_enabled = on;
        if on && session.is_some() {
            self.session = session;
        }
    }

    fn wants(&self, url: &str, kind: &str) -> bool {
        self.enabled && self.patterns.iter().any(|p| p.matches(url, kind))
    }

    fn next_ids(&mut self) -> (String, String) {
        self.counter += 1;
        (
            format!("mar-{}", self.counter),
            format!("interception-{}", self.counter),
        )
    }

    /// Keep a response body where `Network.getResponseBody` can find it.
    pub fn record_body(&mut self, request_id: &str, body: &str) {
        let mut body = body.to_owned();
        if body.len() > MAX_BODY_BYTES {
            // Cut on a character boundary; the string has to stay valid UTF-8.
            let end = (0..=MAX_BODY_BYTES)
                .rev()
                .find(|&i| body.is_char_boundary(i))
                .unwrap_or(0);
            body.truncate(end);
        }
        self.bodies.push_back((request_id.to_owned(), body));
        while self.bodies.len() > MAX_BODIES {
            self.bodies.pop_front();
        }
    }

    pub fn body(&self, request_id: &str) -> Option<&str> {
        self.bodies
            .iter()
            .find(|(id, _)| id == request_id)
            .map(|(_, body)| body.as_str())
    }

    /// Take a `Fetch.continueRequest`/`fulfillRequest`/`failRequest` as the
    /// answer to the request now paused. Returns the reply to send back.
    pub fn resolve(&mut self, command: &Command) -> Outgoing {
        let id = command.id;
        let session = command.session_id.clone();
        let wanted = command.str_param("requestId").unwrap_or_default();
        let Some(paused) = self.paused.as_mut() else {
            return Outgoing::error(id, session, "no request is paused");
        };
        if paused.interception_id != wanted {
            return Outgoing::error(
                id,
                session,
                format!("no paused request with id '{wanted}'"),
            );
        }

        let verdict = match command.method.as_str() {
            "Fetch.fulfillRequest" => Verdict::Fulfill {
                status: command.int_param("responseCode").unwrap_or(200) as u16,
                headers: header_list(command.params.get("responseHeaders")),
                body: command
                    .str_param("body")
                    .map(decode_body)
                    .unwrap_or_default(),
            },
            "Fetch.failRequest" => Verdict::Fail {
                reason: command
                    .str_param("errorReason")
                    .unwrap_or("Failed")
                    .to_owned(),
            },
            _ => Verdict::Continue {
                url: command.str_param("url").map(str::to_owned),
                method: command.str_param("method").map(str::to_owned),
                headers: command
                    .params
                    .get("headers")
                    .map(|h| header_list(Some(h)))
                    .filter(|h| !h.is_empty()),
                body: command.str_param("postData").map(decode_body),
            },
        };
        paused.verdict = Some(verdict);
        Outgoing::empty(id, session)
    }
}

/// A `fulfillRequest` body arrives base64 encoded, as CDP requires. A body that
/// is not valid base64 is taken literally: a client that sent plain text meant
/// plain text, and refusing it would only produce an empty page.
fn decode_body(raw: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(raw)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| raw.to_owned())
}

/// CDP carries headers as an object, and `fulfillRequest` as a list of
/// `{name, value}`. Both shapes reach here.
fn header_list(value: Option<&Value>) -> Vec<(String, String)> {
    match value {
        Some(Value::Object(map)) => map
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
            .collect(),
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(|entry| {
                Some((
                    entry.get("name")?.as_str()?.to_owned(),
                    entry.get("value")?.as_str()?.to_owned(),
                ))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Events go to the session that asked for them, or to the connection when the
/// client subscribed without one.
fn to_client(session: Option<&str>, method: &str, params: Value) -> Outgoing {
    match session {
        Some(s) if !s.is_empty() => Outgoing::session_event(s, method, params),
        _ => Outgoing::event(method, params),
    }
}

fn header_object(headers: &[(String, String)]) -> Value {
    let mut map = serde_json::Map::new();
    for (name, value) in headers {
        map.insert(name.clone(), Value::from(value.clone()));
    }
    Value::Object(map)
}

/// Which document a request belongs to, as the events have to report it.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub frame_id: String,
    pub loader_id: String,
}

/// Announce a request, pause it if the client asked to see requests like it,
/// and return what the client decided.
///
/// The returned id is the one the `Network` events carry, and the one a client
/// quotes back to `Network.getResponseBody`. A main resource passes its own —
/// the loader id — because that equality is how a client tells the document
/// apart from everything the page goes on to fetch.
pub fn arbitrate(
    desk: &Rc<RefCell<Desk>>,
    request: &HttpRequest,
    kind: &str,
    frame: &Frame,
    request_id: Option<&str>,
    deadline: Instant,
) -> (String, Verdict) {
    let (request_id, interception_id, session, channel, wanted, network) = {
        let mut d = desk.borrow_mut();
        let (generated, interception_id) = d.next_ids();
        (
            request_id.map(str::to_owned).unwrap_or(generated),
            interception_id,
            d.session.clone(),
            d.channel.clone(),
            d.wants(&request.url, kind),
            d.network_enabled,
        )
    };

    let Some(channel) = channel else {
        // No connection to ask: only a host driving the browser directly gets
        // here, and it did not ask for interception.
        return (request_id, Verdict::untouched());
    };

    if network {
        channel.borrow_mut().send(&to_client(
            session.as_deref(),
            "Network.requestWillBeSent",
            json!({
                "requestId": request_id,
                "loaderId": frame.loader_id,
                "documentURL": request.url,
                "request": {
                    "url": request.url,
                    "method": request.method,
                    "headers": header_object(&request.headers),
                    "hasPostData": request.body.is_some(),
                    "postData": request.body,
                    "initialPriority": "High",
                    "referrerPolicy": "strict-origin-when-cross-origin",
                    "mixedContentType": "none",
                },
                "timestamp": 0.0,
                "wallTime": 0.0,
                "initiator": {"type": "script"},
                "redirectHasExtraInfo": false,
                "hasUserGesture": false,
                "type": kind,
                "frameId": frame.frame_id,
            }),
        ));
    }

    if !wanted {
        return (request_id, Verdict::untouched());
    }

    desk.borrow_mut().paused = Some(Paused {
        interception_id: interception_id.clone(),
        verdict: None,
    });

    channel.borrow_mut().send(&to_client(
        session.as_deref(),
        "Fetch.requestPaused",
        json!({
            "requestId": interception_id,
            "networkId": request_id,
            "request": {
                "url": request.url,
                "method": request.method,
                "headers": header_object(&request.headers),
                "hasPostData": request.body.is_some(),
                "postData": request.body,
                "initialPriority": "High",
                "referrerPolicy": "strict-origin-when-cross-origin",
            },
            "frameId": frame.frame_id,
            "resourceType": kind,
        }),
    ));

    let verdict = pump(desk, &channel, deadline);
    desk.borrow_mut().paused = None;
    (request_id, verdict)
}

/// Read commands until the paused request is answered or the budget runs out.
fn pump(
    desk: &Rc<RefCell<Desk>>,
    channel: &Rc<RefCell<dyn PauseChannel>>,
    deadline: Instant,
) -> Verdict {
    loop {
        let Some(command) = channel.borrow_mut().next_command(deadline) else {
            tracing::debug!("no answer to Fetch.requestPaused within the page budget");
            return Verdict::Fail {
                reason: "TimedOut".into(),
            };
        };

        let answer = matches!(
            command.method.as_str(),
            "Fetch.continueRequest" | "Fetch.fulfillRequest" | "Fetch.failRequest"
        );
        if !answer {
            desk.borrow_mut().deferred.push_back(command);
            continue;
        }

        let reply = desk.borrow_mut().resolve(&command);
        channel.borrow_mut().send(&reply);
        if let Some(verdict) = desk.borrow_mut().paused.as_mut().and_then(|p| p.verdict.take()) {
            return verdict;
        }
    }
}

/// Report how a request ended, and keep its body for `getResponseBody`.
pub fn settled(
    desk: &Rc<RefCell<Desk>>,
    request_id: &str,
    kind: &str,
    frame: &Frame,
    outcome: Result<&HttpResponse, &str>,
) {
    let (session, channel, network) = {
        let d = desk.borrow();
        (d.session.clone(), d.channel.clone(), d.network_enabled)
    };
    if let Ok(response) = outcome {
        desk.borrow_mut().record_body(request_id, &response.body);
    }
    let (Some(channel), true) = (channel, network) else {
        return;
    };
    let session = session.as_deref();

    match outcome {
        Ok(response) => {
            channel.borrow_mut().send(&to_client(
                session,
                "Network.responseReceived",
                json!({
                    "requestId": request_id,
                    "loaderId": frame.loader_id,
                    "timestamp": 0.0,
                    "type": kind,
                    "hasExtraInfo": false,
                    "frameId": frame.frame_id,
                    "response": {
                        "url": response.url,
                        "status": response.status,
                        "statusText": response.status_text,
                        "headers": header_object(&response.headers),
                        "mimeType": response.header("content-type").unwrap_or("text/plain"),
                        "connectionReused": false,
                        "connectionId": 0,
                        "remoteIPAddress": "",
                        "remotePort": 0,
                        "fromDiskCache": false,
                        "fromServiceWorker": false,
                        "fromPrefetchCache": false,
                        "encodedDataLength": response.body.len(),
                        "responseTime": 0.0,
                        "protocol": "http/1.1",
                        "securityState": "secure",
                        "timing": null,
                    },
                }),
            ));
            channel.borrow_mut().send(&to_client(
                session,
                "Network.loadingFinished",
                json!({
                    "requestId": request_id,
                    "timestamp": 0.0,
                    "encodedDataLength": response.body.len(),
                }),
            ));
        }
        Err(error) => {
            channel.borrow_mut().send(&to_client(
                session,
                "Network.loadingFailed",
                json!({
                    "requestId": request_id,
                    "timestamp": 0.0,
                    "type": kind,
                    "errorText": error,
                    "canceled": false,
                }),
            ));
        }
    }
}

/// The page's network seam with the desk spliced in.
pub struct Intercepted {
    inner: Box<dyn NetworkProvider>,
    desk: Rc<RefCell<Desk>>,
    frame: Frame,
    /// When a paused request gives up. Set from the page's wall-clock budget,
    /// so an unanswered pause cannot outlive the page it belongs to.
    deadline: Instant,
}

impl Intercepted {
    pub fn new(
        inner: Box<dyn NetworkProvider>,
        desk: Rc<RefCell<Desk>>,
        frame: Frame,
        deadline: Instant,
    ) -> Self {
        Intercepted {
            inner,
            desk,
            frame,
            deadline,
        }
    }
}

/// Everything a page fetches for itself is script-initiated.
const SUBRESOURCE: &str = "XHR";

impl NetworkProvider for Intercepted {
    fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, String> {
        let (request_id, verdict) = arbitrate(
            &self.desk,
            &request,
            SUBRESOURCE,
            &self.frame,
            None,
            self.deadline,
        );

        let outcome = match &verdict {
            Verdict::Fail { reason } => Err(format!("request blocked by the client: {reason}")),
            Verdict::Fulfill {
                status,
                headers,
                body,
            } => Ok(HttpResponse {
                status: *status,
                status_text: if (200..300).contains(status) {
                    "OK".into()
                } else {
                    "Error".into()
                },
                url: request.url.clone(),
                headers: headers.clone(),
                body: body.clone(),
            }),
            Verdict::Continue { .. } => {
                let mut request = request;
                verdict.apply(&mut request);
                self.inner.fetch(request)
            }
        };

        settled(
            &self.desk,
            &request_id,
            SUBRESOURCE,
            &self.frame,
            outcome.as_ref().map_err(String::as_str),
        );
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_follow_chromes_glob() {
        assert!(glob_matches("*", "https://example.com/a"));
        assert!(glob_matches("*/api/*", "https://example.com/api/items"));
        assert!(glob_matches(
            "https://example.com/*.json",
            "https://example.com/data/a.json"
        ));
        assert!(!glob_matches("*/api/*", "https://example.com/items"));
        assert!(glob_matches("a?c", "abc"));
        assert!(!glob_matches("a?c", "ac"));
        // A trailing star may match nothing at all.
        assert!(glob_matches("https://example.com/*", "https://example.com/"));
    }

    #[test]
    fn a_resource_type_narrows_a_pattern() {
        let pattern = Pattern {
            url: "*".into(),
            resource_type: Some("Document".into()),
        };
        assert!(pattern.matches("https://example.com/", "Document"));
        assert!(!pattern.matches("https://example.com/", "XHR"));
    }

    #[test]
    fn bodies_are_bounded_in_count_and_size() {
        let mut desk = Desk::default();
        for i in 0..MAX_BODIES + 4 {
            desk.record_body(&format!("r{i}"), "body");
        }
        assert_eq!(desk.bodies.len(), MAX_BODIES);
        assert!(desk.body("r0").is_none(), "the oldest was evicted");
        assert_eq!(desk.body(&format!("r{}", MAX_BODIES + 3)), Some("body"));

        desk.record_body("big", &"x".repeat(MAX_BODY_BYTES * 2));
        assert_eq!(desk.body("big").map(str::len), Some(MAX_BODY_BYTES));
    }
}
