//! Per-page state shared between Rust and the JS globals.

use crate::net::HttpResponse;
use crate::timers::TimerQueue;
use mar_dom::NodeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Log => "log",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
            LogLevel::Debug => "debug",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConsoleMessage {
    pub level: LogLevel,
    pub text: String,
    /// Virtual milliseconds since navigation.
    pub at_ms: i64,
}

/// A script error that did not stop the page.
#[derive(Debug, Clone)]
pub struct ScriptError {
    pub message: String,
    /// Where it came from: an inline script, a URL, or an event handler.
    pub source: String,
}

/// Where a page asked to go, and how.
///
/// A `location.href` assignment is a GET, but a `form.submit()` on a POST
/// form is a POST with a body, and the login and single-sign-on flows that
/// use one are not reachable any other way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Navigation {
    pub url: String,
    pub method: String,
    pub body: Option<String>,
}

impl Navigation {
    pub fn get(url: impl Into<String>) -> Self {
        Navigation {
            url: url.into(),
            method: "GET".to_owned(),
            body: None,
        }
    }
}

/// Limits that keep one page from consuming the process.
#[derive(Debug, Clone)]
pub struct Limits {
    /// QuickJS heap ceiling. Exceeding it throws inside JS rather than aborting.
    pub memory_bytes: usize,
    /// JS stack ceiling, to turn runaway recursion into an exception.
    pub stack_bytes: usize,
    /// Wall-clock budget for the whole page.
    pub wall_ms: u64,
    /// Virtual time a page is allowed to reach. Timers past it never run.
    pub virtual_horizon_ms: i64,
    /// Total timer callbacks, so a `setInterval` cannot spin forever.
    pub max_timer_callbacks: u64,
    /// Subresource requests a page may make, counting module loads.
    pub max_requests: usize,
    /// Bytes of console output retained.
    pub max_console_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            memory_bytes: 256 * 1024 * 1024,
            stack_bytes: 1024 * 1024,
            wall_ms: 15_000,
            virtual_horizon_ms: 10_000,
            max_timer_callbacks: 10_000,
            max_requests: 384,
            max_console_bytes: 256 * 1024,
        }
    }
}

/// Everything the JS globals read or mutate, outside the DOM itself.
pub struct PageState {
    pub url: Url,
    pub referrer: String,
    pub user_agent: String,
    pub timers: TimerQueue,
    pub console: Vec<ConsoleMessage>,
    pub console_bytes: usize,
    pub errors: Vec<ScriptError>,
    /// `document.cookie`, as a single `k=v; k2=v2` string.
    pub cookies: String,
    /// Every `document.cookie = ...` a script performed, kept verbatim.
    ///
    /// The flattened string above has already lost `path`, `domain` and
    /// `expires`; a host that owns a real cookie jar needs them, so the raw
    /// assignment is kept alongside it.
    pub cookie_writes: Vec<String>,
    pub local_storage: HashMap<String, String>,
    pub session_storage: HashMap<String, String>,
    /// Set when a script assigns `location.href` or calls `location.replace`.
    /// The caller decides whether to follow it.
    pub requested_navigation: Option<Navigation>,
    /// `document.readyState`.
    pub ready_state: &'static str,
    /// The `<script>` element currently executing, for `document.currentScript`.
    ///
    /// Bundlers read it, and its `src`, to work out where their own chunks
    /// live; webpack's "Automatic publicPath" and Next's chunk loader both
    /// throw outright when it is null outside a worker.
    pub current_script: Option<NodeId>,
    /// Subresource requests made so far, against `Limits::max_requests`.
    pub request_count: usize,
    /// Requests handed to a worker thread and not yet delivered back to JS.
    ///
    /// The page is not idle while any of these are outstanding, so the settle
    /// loop waits on them rather than declaring the page finished — the same
    /// rule a browser's event loop follows.
    pub inflight: usize,
    /// Where a finished request lands, tagged with the id JS is waiting on.
    pub responses: std::sync::mpsc::Receiver<(u32, Result<HttpResponse, String>)>,
    /// Handed to each worker thread.
    pub responses_tx: std::sync::mpsc::Sender<(u32, Result<HttpResponse, String>)>,
    /// Ids for requests JS is waiting on.
    pub next_request_id: u32,
    pub limits: Limits,
    /// When this page started. The wall-clock budget is measured from here,
    /// and everything that can block — a `fetch`, an `import` — checks it,
    /// so `wall_ms` bounds the page rather than only its script loop.
    pub started: Instant,
    /// Viewport reported to scripts. Nothing is laid out at this size; it is
    /// what `innerWidth`, `matchMedia` and `getBoundingClientRect` quote.
    pub viewport: (u32, u32),
}

impl PageState {
    pub fn new(url: Url, limits: Limits) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        PageState {
            url,
            referrer: String::new(),
            user_agent: default_user_agent().to_owned(),
            timers: TimerQueue::new(limits.virtual_horizon_ms, limits.max_timer_callbacks),
            console: Vec::new(),
            console_bytes: 0,
            errors: Vec::new(),
            cookies: String::new(),
            cookie_writes: Vec::new(),
            local_storage: HashMap::new(),
            session_storage: HashMap::new(),
            requested_navigation: None,
            ready_state: "loading",
            current_script: None,
            request_count: 0,
            inflight: 0,
            responses: rx,
            responses_tx: tx,
            next_request_id: 1,
            limits,
            started: Instant::now(),
            viewport: (1280, 800),
        }
    }

    /// Has this page spent its wall-clock budget?
    pub fn out_of_time(&self) -> bool {
        self.started.elapsed().as_millis() as u64 >= self.limits.wall_ms
    }

    pub fn log(&mut self, level: LogLevel, text: String) {
        if self.console_bytes >= self.limits.max_console_bytes {
            return;
        }
        self.console_bytes += text.len();
        let at_ms = self.timers.now_ms();
        self.console.push(ConsoleMessage { level, text, at_ms });
    }

    pub fn record_error(&mut self, source: impl Into<String>, message: impl Into<String>) {
        // Cap the list: a page in a failing render loop can throw thousands.
        if self.errors.len() < 100 {
            self.errors.push(ScriptError {
                message: message.into(),
                source: source.into(),
            });
        }
    }

    /// Parse `document.cookie` into pairs.
    pub fn cookie_pairs(&self) -> Vec<(String, String)> {
        self.cookies
            .split(';')
            .filter_map(|part| {
                let part = part.trim();
                let (k, v) = part.split_once('=')?;
                Some((k.trim().to_owned(), v.trim().to_owned()))
            })
            .collect()
    }

    /// Apply a `document.cookie = ...` assignment, which sets exactly one pair.
    pub fn set_cookie(&mut self, raw: &str) {
        let first = raw.split(';').next().unwrap_or("").trim();
        let Some((name, value)) = first.split_once('=') else {
            return;
        };
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.cookie_writes.push(raw.to_owned());
        let mut pairs = self.cookie_pairs();
        match pairs.iter_mut().find(|(k, _)| k == name) {
            Some(existing) => existing.1 = value.trim().to_owned(),
            None => pairs.push((name.to_owned(), value.trim().to_owned())),
        }
        self.cookies = pairs
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
    }
}

/// Chrome on macOS. Sites branch on this string, and a truthful one ("some Rust
/// crate") lands on unstyled or blocked paths far more often.
pub fn default_user_agent() -> &'static str {
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/140.0.0.0 Safari/537.36"
}

pub type Shared = Rc<RefCell<PageState>>;

pub fn shared(state: PageState) -> Shared {
    Rc::new(RefCell::new(state))
}
