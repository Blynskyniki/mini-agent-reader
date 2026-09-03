//! The network seam.
//!
//! The JS engine never opens a socket itself. It hands requests to whatever the
//! host installs, which is what makes the engine testable offline and lets the
//! host enforce policy: allowed hosts, per-page budgets, cookie handling and
//! blocking of images, fonts and analytics.

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub status_text: String,
    /// Final URL after redirects.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Fetches a subresource on behalf of a page.
///
/// Blocking: the settle loop is single-threaded and drives a virtual clock, so
/// a call here is simply "time does not advance while the network works". This
/// is the path an `import` and a synchronous XHR take, both of which block in
/// the page as well.
pub trait NetworkProvider {
    fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, String>;

    /// Start fetching these URLs now; the page will ask for them shortly.
    ///
    /// Fire and forget: a host that cannot prefetch ignores it, and a URL that
    /// turns out not to be wanted costs one request nobody reads. Correctness
    /// never depends on this having happened.
    fn prefetch(&self, _urls: Vec<String>) {}

    /// The same seam, callable from another thread, when the host has one.
    ///
    /// A page that issues ten requests expects ten round trips to overlap —
    /// `Promise.all([fetch(a), fetch(b)])` is two requests in flight, not two
    /// in a row. Serialising them turns a page with a hundred of them into a
    /// hundred latencies added up, which on a news site is most of the wall
    /// clock.
    ///
    /// Optional because not every host can: the CDP interceptor keeps
    /// per-session state that is not `Send`. Returning `None` costs
    /// correctness nothing — requests are simply issued one at a time.
    fn concurrent(&self) -> Option<std::sync::Arc<dyn ConcurrentNetwork>> {
        None
    }
}

/// A network seam that may be called from a worker thread.
pub trait ConcurrentNetwork: Send + Sync + 'static {
    fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, String>;
}

/// Refuses everything. The default for evaluating a page offline, and what the
/// tests use so they never touch the network.
pub struct NoNetwork;

impl NetworkProvider for NoNetwork {
    fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, String> {
        Err(format!(
            "network disabled: refused {} {}",
            request.method, request.url
        ))
    }
}

/// Serves canned responses by URL, for tests and for replaying a captured page.
pub struct StaticNetwork {
    routes: Vec<(String, HttpResponse)>,
}

impl StaticNetwork {
    pub fn new() -> Self {
        StaticNetwork { routes: Vec::new() }
    }

    /// Respond to any URL containing `pattern` with this body.
    pub fn route(mut self, pattern: &str, status: u16, content_type: &str, body: &str) -> Self {
        self.routes.push((
            pattern.to_owned(),
            HttpResponse {
                status,
                status_text: if (200..300).contains(&status) {
                    "OK".into()
                } else {
                    "Error".into()
                },
                url: pattern.to_owned(),
                headers: vec![("content-type".into(), content_type.into())],
                body: body.to_owned(),
            },
        ));
        self
    }
}

impl Default for StaticNetwork {
    fn default() -> Self {
        StaticNetwork::new()
    }
}

impl NetworkProvider for StaticNetwork {
    fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, String> {
        self.routes
            .iter()
            .find(|(pattern, _)| request.url.contains(pattern.as_str()))
            .map(|(_, response)| {
                let mut r = response.clone();
                r.url = request.url.clone();
                r
            })
            .ok_or_else(|| format!("no route for {}", request.url))
    }
}
