//! The HTTP client the engine fetches through.
//!
//! Three jobs beyond "download bytes": look like a browser at the header level,
//! decode whatever charset the server actually sent, and enforce policy so a
//! page cannot drag the host somewhere it should not go.

mod charset;
mod policy;

pub use charset::{decode_body, sniff_charset};
pub use policy::{Policy, PolicyError, ResourceKind};

use mar_js::{HttpRequest, HttpResponse, NetworkProvider};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// How the client presents itself and what it is allowed to reach.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub user_agent: String,
    pub accept_language: String,
    pub timeout: Duration,
    /// Redirects followed before giving up.
    pub max_redirects: u32,
    /// Response bodies larger than this are truncated.
    pub max_body_bytes: usize,
    pub policy: Policy,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            user_agent: mar_js::state::default_user_agent().to_owned(),
            accept_language: "en-US,en;q=0.9".to_owned(),
            timeout: Duration::from_secs(15),
            max_redirects: 5,
            max_body_bytes: 8 * 1024 * 1024,
            policy: Policy::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("blocked by policy")]
    Policy(#[from] PolicyError),
    #[error("request failed: {0}")]
    Transport(String),
    #[error("invalid URL {url}: {source}")]
    Url {
        url: String,
        #[source]
        source: url::ParseError,
    },
}

/// A blocking HTTP client with browser-shaped headers.
///
/// One agent is shared across requests so connections and the cookie jar are
/// reused. Cloning is cheap and thread-safe.
#[derive(Clone)]
pub struct HttpClient {
    agent: ureq::Agent,
    config: Arc<ClientConfig>,
    requests: Arc<AtomicUsize>,
}

impl HttpClient {
    pub fn new(config: ClientConfig) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(config.timeout))
            // Redirects are followed here rather than in the engine so the
            // page only ever sees the final URL.
            .max_redirects(config.max_redirects)
            .save_redirect_history(true)
            .user_agent(&config.user_agent)
            .build()
            .into();
        HttpClient {
            agent,
            config: Arc::new(config),
            requests: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Requests issued so far, across every clone of this client.
    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    /// Fetch a top-level document.
    pub fn get_document(&self, url: &str) -> Result<Fetched, NetError> {
        self.execute("GET", url, &[], None, ResourceKind::Document)
    }

    /// Fetch a script the page referenced, so it can be inlined and run.
    pub fn get_script(&self, url: &str) -> Result<Fetched, NetError> {
        self.execute("GET", url, &[], None, ResourceKind::Script)
    }

    /// The header set a browser sends, minus anything that would be a lie.
    ///
    /// Servers do branch on these: a request without `Accept` or `Sec-Fetch-*`
    /// is frequently served a different page, or none at all.
    fn browser_headers(&self, kind: ResourceKind, url: &url::Url) -> Vec<(&'static str, String)> {
        let mut headers = vec![
            ("Accept-Language", self.config.accept_language.clone()),
            ("Accept-Encoding", "gzip, deflate, br".to_owned()),
            ("Upgrade-Insecure-Requests", "1".to_owned()),
            ("Sec-CH-UA", r#""Chromium";v="140", "Not=A?Brand";v="24""#.to_owned()),
            ("Sec-CH-UA-Mobile", "?0".to_owned()),
            ("Sec-CH-UA-Platform", "\"macOS\"".to_owned()),
        ];
        match kind {
            ResourceKind::Document => {
                headers.push((
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,\
                     image/webp,*/*;q=0.8"
                        .to_owned(),
                ));
                headers.push(("Sec-Fetch-Dest", "document".to_owned()));
                headers.push(("Sec-Fetch-Mode", "navigate".to_owned()));
                headers.push(("Sec-Fetch-Site", "none".to_owned()));
                headers.push(("Sec-Fetch-User", "?1".to_owned()));
            }
            ResourceKind::Script => {
                headers.push(("Accept", "*/*".to_owned()));
                headers.push(("Sec-Fetch-Dest", "script".to_owned()));
                headers.push(("Sec-Fetch-Mode", "no-cors".to_owned()));
                headers.push(("Sec-Fetch-Site", "same-origin".to_owned()));
                headers.push(("Referer", url.origin().ascii_serialization() + "/"));
            }
            ResourceKind::Xhr => {
                headers.push(("Accept", "*/*".to_owned()));
                headers.push(("Sec-Fetch-Dest", "empty".to_owned()));
                headers.push(("Sec-Fetch-Mode", "cors".to_owned()));
                headers.push(("Sec-Fetch-Site", "same-origin".to_owned()));
                headers.push(("Referer", url.origin().ascii_serialization() + "/"));
            }
        }
        headers
    }

    fn execute(
        &self,
        method: &str,
        url: &str,
        extra_headers: &[(String, String)],
        body: Option<&str>,
        kind: ResourceKind,
    ) -> Result<Fetched, NetError> {
        let parsed = url::Url::parse(url).map_err(|source| NetError::Url {
            url: url.to_owned(),
            source,
        })?;
        self.config.policy.check(&parsed)?;
        self.requests.fetch_add(1, Ordering::Relaxed);

        // ureq 3 exposes typed per-method builders; going through the http
        // crate keeps one code path for every method, body or not.
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(parsed.as_str());
        for (name, value) in self.browser_headers(kind, &parsed) {
            builder = builder.header(name, value);
        }
        // Caller headers win: a page's own Content-Type or Authorization must
        // not be shadowed by the defaults above.
        for (name, value) in extra_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let request = builder
            .body(body.unwrap_or(""))
            .map_err(|e| NetError::Transport(e.to_string()))?;

        let mut response = match self.agent.run(request) {
            Ok(r) => r,
            // A 4xx/5xx is a real response and the body often matters, so it is
            // returned rather than treated as a transport failure.
            Err(ureq::Error::StatusCode(code)) => {
                return Ok(Fetched {
                    status: code,
                    status_text: String::new(),
                    final_url: parsed.to_string(),
                    headers: Vec::new(),
                    body: String::new(),
                    charset: "utf-8".into(),
                    truncated: false,
                });
            }
            Err(e) => return Err(NetError::Transport(e.to_string())),
        };

        let status = response.status().as_u16();
        let status_text = response
            .status()
            .canonical_reason()
            .unwrap_or_default()
            .to_owned();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_owned())))
            .collect();
        let final_url = {
            use ureq::ResponseExt;
            response.get_uri().to_string()
        };
        let content_type = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();

        let limit = self.config.max_body_bytes;
        let raw = response
            .body_mut()
            .with_config()
            // +1 so a body exactly at the limit is not reported as truncated.
            .limit((limit + 1) as u64)
            .read_to_vec()
            .map_err(|e| NetError::Transport(e.to_string()))?;
        let truncated = raw.len() > limit;
        let raw = if truncated { &raw[..limit] } else { &raw[..] };

        let (body, charset) = decode_body(raw, &content_type);

        Ok(Fetched {
            status,
            status_text,
            final_url,
            headers,
            body,
            charset,
            truncated,
        })
    }
}

/// A fetched resource, decoded to text.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub status: u16,
    pub status_text: String,
    /// URL after redirects. Relative links must resolve against this.
    pub final_url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// Charset the body was decoded from.
    pub charset: String,
    /// True when the body hit the size limit and was cut short.
    pub truncated: bool,
}

impl Fetched {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_type(&self) -> &str {
        self.header("content-type").unwrap_or("")
    }

    pub fn is_html(&self) -> bool {
        let ct = self.content_type().to_ascii_lowercase();
        ct.contains("text/html") || ct.contains("application/xhtml")
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Adapter that lets a page's `fetch`/XHR reach the network under policy.
pub struct PageNetwork {
    client: HttpClient,
    /// Origin of the page, used to decide same-origin.
    page_origin: String,
}

impl PageNetwork {
    pub fn new(client: HttpClient, page_url: &url::Url) -> Self {
        PageNetwork {
            page_origin: page_url.origin().ascii_serialization(),
            client,
        }
    }
}

impl NetworkProvider for PageNetwork {
    fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, String> {
        let parsed = url::Url::parse(&request.url).map_err(|e| e.to_string())?;
        // Same-origin subresource requests are the ones worth serving: they are
        // how a page loads its own data. Cross-origin ones are usually
        // analytics and ads, which cost time and change nothing readable.
        if parsed.origin().ascii_serialization() != self.page_origin {
            return Err(format!("cross-origin request blocked: {}", request.url));
        }
        let headers: Vec<(String, String)> = request.headers.clone();
        let fetched = self
            .client
            .execute(
                &request.method,
                &request.url,
                &headers,
                request.body.as_deref(),
                ResourceKind::Xhr,
            )
            .map_err(|e| e.to_string())?;

        Ok(HttpResponse {
            status: fetched.status,
            status_text: fetched.status_text,
            url: fetched.final_url,
            headers: fetched.headers,
            body: fetched.body,
        })
    }
}
