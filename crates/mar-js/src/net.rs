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
/// Blocking by design: the settle loop is single-threaded and drives a virtual
/// clock, so a blocking call here is simply "time does not advance while the
/// network works". Hosts that want concurrency run whole pages in parallel.
pub trait NetworkProvider {
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
