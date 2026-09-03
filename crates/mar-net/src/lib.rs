//! The HTTP client the engine fetches through.
//!
//! Three jobs beyond "download bytes": look like a browser at the header level,
//! decode whatever charset the server actually sent, and enforce policy so a
//! page cannot drag the host somewhere it should not go.

mod charset;
mod policy;
pub mod tls;

pub use charset::{decode_body, sniff_charset};
pub use policy::{Policy, PolicyError, ResourceKind};
pub use tls::{BundledCert, TrustMode, bundled_certs, load_pem_bundle};

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
    /// Which roots to verify against, and in what order.
    pub trust: TrustMode,
    /// Extra roots beyond the bundled ones, from a caller-supplied PEM bundle.
    pub extra_roots: Vec<ureq::tls::Certificate<'static>>,
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
            trust: TrustMode::default(),
            extra_roots: Vec::new(),
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
    /// Used only to retry a request the primary agent rejected on certificate
    /// grounds. See [`tls::TrustMode`].
    fallback_agent: Option<ureq::Agent>,
    config: Arc<ClientConfig>,
    requests: Arc<AtomicUsize>,
    /// Hosts already known to need the fallback, so the second and later
    /// requests to one skip the handshake that is going to fail.
    needs_fallback: Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
}

impl HttpClient {
    pub fn new(config: ClientConfig) -> Self {
        // The bundled roots plus anything the caller added.
        let mut extra = tls::extra_roots();
        extra.extend(config.extra_roots.iter().cloned());

        let build = |tls_config: ureq::tls::TlsConfig| -> ureq::Agent {
            ureq::Agent::config_builder()
                .timeout_global(Some(config.timeout))
                // Redirects are followed here rather than in the engine so the
                // page only ever sees the final URL.
                .max_redirects(config.max_redirects)
                .save_redirect_history(true)
                .user_agent(&config.user_agent)
                .tls_config(tls_config)
                .build()
                .into()
        };

        let agent = build(tls::primary_config(config.trust, &extra));
        let fallback_agent = tls::fallback_config(config.trust, &extra).map(build);

        HttpClient {
            agent,
            fallback_agent,
            config: Arc::new(config),
            requests: Arc::new(AtomicUsize::new(0)),
            needs_fallback: Arc::new(std::sync::RwLock::new(Default::default())),
        }
    }

    /// Does this client carry a second trust set to fall back on?
    pub fn has_extended_trust(&self) -> bool {
        self.fallback_agent.is_some()
    }

    /// Hosts that needed the extended trust set during this client's life.
    pub fn hosts_using_extended_trust(&self) -> Vec<String> {
        self.needs_fallback
            .read()
            .map(|set| {
                let mut hosts: Vec<String> = set.iter().cloned().collect();
                hosts.sort();
                hosts
            })
            .unwrap_or_default()
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Requests issued so far, across every clone of this client.
    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    /// Every agent that could serve a request, primary first.
    ///
    /// Each carries its own jar, and which one a host lands on is decided by
    /// its certificate chain, so a cookie has to be written to both to survive
    /// a switch between them.
    fn agents(&self) -> impl Iterator<Item = &ureq::Agent> {
        std::iter::once(&self.agent).chain(self.fallback_agent.iter())
    }

    /// What `document.cookie` should read as on `url`.
    pub fn cookies_for(&self, url: &str) -> String {
        let Ok(parsed) = url::Url::parse(url) else {
            return String::new();
        };
        // The jar can only be queried by an exact domain, path and name triple,
        // so the candidates a browser would match against are enumerated and
        // the jar decides which of them it actually holds.
        let domains = domain_candidates(parsed.host_str().unwrap_or_default());
        let paths = path_candidates(parsed.path());

        let mut pairs: Vec<(String, String)> = Vec::new();
        for agent in self.agents() {
            let jar = agent.cookie_jar_lock();
            let names: Vec<String> = jar.iter().map(|c| c.name().to_owned()).collect();
            for name in names {
                if pairs.iter().any(|(existing, _)| *existing == name) {
                    continue;
                }
                let found = domains.iter().find_map(|domain| {
                    paths
                        .iter()
                        .find_map(|path| jar.get(domain, path, &name))
                        .map(|c| c.value().to_owned())
                });
                if let Some(value) = found {
                    pairs.push((name, value));
                }
            }
        }

        pairs
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Apply a `document.cookie = ...` a script performed on `url`.
    ///
    /// Kept verbatim rather than as a name and value, so `path`, `domain` and
    /// `expires` are honoured by the same rules a response's `Set-Cookie` gets.
    pub fn apply_script_cookie(&self, url: &str, raw: &str) {
        let Ok(uri) = url.parse::<ureq::http::Uri>() else {
            return;
        };
        for agent in self.agents() {
            let Ok(cookie) = ureq::Cookie::parse(raw.to_owned(), &uri) else {
                continue;
            };
            let _ = agent.cookie_jar_lock().insert(cookie, &uri);
        }
    }

    /// A fingerprint of the cookies that apply to `url`.
    ///
    /// Two fetches of one URL are the same fetch unless something changed in
    /// between; this is how a caller tells those apart.
    pub fn cookie_fingerprint(&self, url: &str) -> String {
        let mut pairs: Vec<String> = self
            .cookies_for(url)
            .split("; ")
            .map(|s| s.to_owned())
            .collect();
        pairs.sort();
        pairs.join("; ")
    }

    /// Fetch a top-level document.
    pub fn get_document(&self, url: &str) -> Result<Fetched, NetError> {
        self.execute("GET", url, &[], None, ResourceKind::Document, None)
    }

    /// Fetch a script the page referenced, so it can be inlined and run.
    pub fn get_script(&self, url: &str, referer: &url::Url) -> Result<Fetched, NetError> {
        self.execute("GET", url, &[], None, ResourceKind::Script, Some(referer))
    }

    /// The header set a browser sends, minus anything that would be a lie.
    ///
    /// Servers do branch on these: a request without `Accept` or `Sec-Fetch-*`
    /// is frequently served a different page, or none at all.
    fn browser_headers(
        &self,
        kind: ResourceKind,
        url: &url::Url,
        referer: Option<&url::Url>,
    ) -> Vec<(&'static str, String)> {
        let mut headers = vec![
            ("Accept-Language", self.config.accept_language.clone()),
            ("Accept-Encoding", "gzip, deflate, br".to_owned()),
            ("Upgrade-Insecure-Requests", "1".to_owned()),
            (
                "Sec-CH-UA",
                r#""Chromium";v="140", "Not=A?Brand";v="24""#.to_owned(),
            ),
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
                headers.push(("Sec-Fetch-Site", site_relation(referer, url)));
                // The referring page, not just its origin. A site that checks
                // where a script was requested from sees the real page here,
                // which is what a browser sends.
                if let Some(referer) = referer {
                    headers.push(("Referer", referer.to_string()));
                }
            }
            ResourceKind::Xhr => {
                headers.push(("Accept", "*/*".to_owned()));
                headers.push(("Sec-Fetch-Dest", "empty".to_owned()));
                headers.push(("Sec-Fetch-Mode", "cors".to_owned()));
                headers.push(("Sec-Fetch-Site", site_relation(referer, url)));
                if let Some(referer) = referer {
                    headers.push(("Referer", referer.to_string()));
                    // A browser sends Origin on any request a script made,
                    // except a plain same-origin GET. Its absence is one of the
                    // cheapest ways for a server to spot a non-browser client.
                    headers.push(("Origin", referer.origin().ascii_serialization()));
                }
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
        referer: Option<&url::Url>,
    ) -> Result<Fetched, NetError> {
        let parsed = url::Url::parse(url).map_err(|source| NetError::Url {
            url: url.to_owned(),
            source,
        })?;
        self.config.policy.check(&parsed)?;
        self.requests.fetch_add(1, Ordering::Relaxed);

        // ureq 3 exposes typed per-method builders; going through the http
        // crate keeps one code path for every method, body or not.
        let mut header_pairs: Vec<(String, String)> = self
            .browser_headers(kind, &parsed, referer)
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect();
        // Caller headers win: a page's own Content-Type or Authorization must
        // not be shadowed by the defaults above.
        for (name, value) in extra_headers {
            header_pairs.retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
            header_pairs.push((name.clone(), value.clone()));
        }

        let body = body.filter(|b| !b.is_empty());
        let uri = parsed.as_str().to_owned();
        let method = method.to_owned();

        // Built fresh for each attempt: `Agent::run` consumes the request, and
        // a certificate retry needs an identical second copy.
        let build = || -> Result<ureq::http::request::Builder, NetError> {
            let mut builder = ureq::http::Request::builder()
                .method(method.as_str())
                .uri(&uri);
            for (name, value) in &header_pairs {
                builder = builder.header(name.as_str(), value.as_str());
            }
            Ok(builder)
        };

        // A bodyless request must carry no body at all. Handing ureq an empty
        // string instead produces `Content-Length: 0` on a GET, which no
        // browser sends and which a bot check reads as an automated client.
        let send = |agent: &ureq::Agent| -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
            let builder = match build() {
                Ok(b) => b,
                Err(_) => return Err(ureq::Error::BadUri(uri.clone())),
            };
            match body {
                Some(text) => agent.run(builder.body(text).map_err(ureq::Error::Http)?),
                None => agent.run(builder.body(()).map_err(ureq::Error::Http)?),
            }
        };

        // A host already known to need the extended trust set skips straight
        // to it rather than repeating a handshake that is going to fail.
        let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
        let known_fallback = self
            .needs_fallback
            .read()
            .map(|set| set.contains(&host))
            .unwrap_or(false);

        let first_agent = match (known_fallback, &self.fallback_agent) {
            (true, Some(agent)) => agent,
            _ => &self.agent,
        };

        let mut outcome = send(first_agent);

        // Retry once against the extended trust set when the public roots
        // rejected the chain. Only a certificate failure qualifies: a refused
        // connection or a timeout would fail the same way twice.
        if !known_fallback
            && let Some(fallback) = &self.fallback_agent
            && let Err(e) = &outcome
            && tls::is_certificate_error(&e.to_string())
        {
            tracing::debug!(host = %host, "retrying with the extended trust set");
            let retried = send(fallback);
            if retried.is_ok()
                && let Ok(mut set) = self.needs_fallback.write()
            {
                set.insert(host.clone());
            }
            outcome = retried;
        }

        let mut response = match outcome {
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
                    raw_prefix: Vec::new(),
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

        // Bodies come back as raw bytes: ureq's own charset support is not
        // enabled, because it transcodes from the Content-Type header alone and
        // would decode a body this crate then decodes again. `decode_body`
        // below follows the full sniffing order instead.
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

        // Keep a small window of the original bytes: the decoded text cannot
        // be used to recognise a binary format.
        let raw_prefix = raw[..raw.len().min(1024)].to_vec();
        let (body, charset) = decode_body(raw, &content_type);

        Ok(Fetched {
            status,
            status_text,
            final_url,
            headers,
            body,
            charset,
            truncated,
            raw_prefix,
        })
    }
}

/// The `Sec-Fetch-Site` value for a request made from `referer` to `url`.
///
/// Servers do read this. Sending "same-origin" for a cross-site request, or
/// "none" for one a script made, is a mismatch a bot check can see.
fn site_relation(referer: Option<&url::Url>, url: &url::Url) -> String {
    let Some(referer) = referer else {
        return "none".to_owned();
    };
    if referer.origin() == url.origin() {
        return "same-origin".to_owned();
    }
    // Same registrable domain but a different subdomain or scheme is
    // "same-site"; anything else is "cross-site".
    match (referer.host_str(), url.host_str()) {
        (Some(a), Some(b)) if registrable(a) == registrable(b) => "same-site".to_owned(),
        _ => "cross-site".to_owned(),
    }
}

/// The last two labels of a host, as a rough registrable domain. Good enough
/// to tell "api.example.com" from "example.org" without a public-suffix list.
fn registrable(host: &str) -> String {
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 {
        return host.to_ascii_lowercase();
    }
    labels[labels.len() - 2..].join(".").to_ascii_lowercase()
}

/// A host and every parent a cookie could have been scoped to.
///
/// `www.gosuslugi.ru` yields itself and `gosuslugi.ru`, which is what a
/// `Domain=` attribute one level up produces.
fn domain_candidates(host: &str) -> Vec<String> {
    let host = host.trim_start_matches('.').to_ascii_lowercase();
    let mut out = vec![host.clone()];
    let labels: Vec<&str> = host.split('.').collect();
    for cut in 1..labels.len().saturating_sub(1) {
        out.push(labels[cut..].join("."));
    }
    out
}

/// A path and every prefix directory of it, longest first, as cookie matching
/// walks upward from the request path to the root.
fn path_candidates(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = if path.is_empty() { "/" } else { path };
    loop {
        out.push(current.to_owned());
        match current.rfind('/') {
            Some(0) | None => break,
            Some(cut) => current = &current[..cut],
        }
    }
    if !out.iter().any(|p| p == "/") {
        out.push("/".to_owned());
    }
    out
}

/// What kind of resource a response holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseKind {
    Html,
    Json,
    Xml,
    Text,
    Pdf,
    Image,
    Media,
    Other,
}

impl ResponseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ResponseKind::Html => "html",
            ResponseKind::Json => "json",
            ResponseKind::Xml => "xml",
            ResponseKind::Text => "text",
            ResponseKind::Pdf => "pdf",
            ResponseKind::Image => "image",
            ResponseKind::Media => "media",
            ResponseKind::Other => "other",
        }
    }
}

/// Does this look like HTML, for a server that declared nothing?
fn looks_like_html(prefix: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&prefix[..prefix.len().min(512)]).to_ascii_lowercase();
    let head = head.trim_start();
    head.starts_with("<!doctype html")
        || head.starts_with("<html")
        || (head.starts_with('<') && head.contains("<body"))
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
    /// First bytes of the undecoded body, for sniffing the real type.
    raw_prefix: Vec<u8>,
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
        matches!(self.kind(), ResponseKind::Html)
    }

    /// What kind of resource came back.
    ///
    /// A URL is not a promise about content type. Feeding a PDF or an image to
    /// an HTML parser produces a document of mojibake that looks like text and
    /// is not, so the caller has to be told what it actually received.
    pub fn kind(&self) -> ResponseKind {
        let ct = self.content_type().to_ascii_lowercase();
        let ct = ct.split(';').next().unwrap_or("").trim().to_owned();

        // A declared type is usually right, but not always: servers send
        // application/octet-stream for everything. The magic bytes settle it.
        if self.raw_prefix.starts_with(b"%PDF-") {
            return ResponseKind::Pdf;
        }

        match ct.as_str() {
            "application/pdf" => ResponseKind::Pdf,
            "text/html" | "application/xhtml+xml" => ResponseKind::Html,
            "application/json" | "text/json" | "application/ld+json" => ResponseKind::Json,
            "application/xml" | "text/xml" | "application/rss+xml" | "application/atom+xml" => {
                ResponseKind::Xml
            }
            _ if ct.starts_with("text/") => ResponseKind::Text,
            _ if ct.starts_with("image/") => ResponseKind::Image,
            _ if ct.starts_with("audio/") || ct.starts_with("video/") => ResponseKind::Media,
            "" => {
                // No declaration at all: guess from the bytes.
                if looks_like_html(&self.raw_prefix) {
                    ResponseKind::Html
                } else {
                    ResponseKind::Other
                }
            }
            _ => ResponseKind::Other,
        }
    }

    /// Is this something the HTML pipeline can meaningfully read?
    pub fn is_readable_as_html(&self) -> bool {
        matches!(
            self.kind(),
            ResponseKind::Html | ResponseKind::Xml | ResponseKind::Text
        )
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Adapter that lets a page's `fetch`/XHR reach the network under policy.
pub struct PageNetwork {
    client: HttpClient,
    /// The page making the request. Used to decide same-origin and to fill in
    /// `Referer` and `Origin`, exactly as a browser would.
    page_url: url::Url,
}

impl PageNetwork {
    pub fn new(client: HttpClient, page_url: &url::Url) -> Self {
        PageNetwork {
            page_url: page_url.clone(),
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
        if parsed.origin() != self.page_url.origin() {
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
                Some(&self.page_url),
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

#[cfg(test)]
mod cookie_tests {
    use super::*;

    fn client() -> HttpClient {
        HttpClient::new(ClientConfig::default())
    }

    #[test]
    fn a_script_cookie_is_readable_back_on_the_same_site() {
        let c = client();
        c.apply_script_cookie("https://www.gosuslugi.ru/", "jsch=solved; path=/");
        assert_eq!(c.cookies_for("https://www.gosuslugi.ru/"), "jsch=solved");
        assert_eq!(
            c.cookies_for("https://www.gosuslugi.ru/deep/page"),
            "jsch=solved",
            "a path=/ cookie applies further down the site"
        );
    }

    #[test]
    fn a_cookie_does_not_leak_to_another_site() {
        let c = client();
        c.apply_script_cookie("https://a.example/", "token=secret; path=/");
        assert_eq!(c.cookies_for("https://b.example/"), "");
    }

    #[test]
    fn a_domain_cookie_reaches_a_subdomain() {
        let c = client();
        c.apply_script_cookie("https://www.example.com/", "id=7; domain=example.com; path=/");
        assert_eq!(c.cookies_for("https://api.example.com/"), "id=7");
    }

    #[test]
    fn the_fingerprint_changes_when_a_cookie_is_added() {
        let c = client();
        let before = c.cookie_fingerprint("https://example.com/");
        c.apply_script_cookie("https://example.com/", "gate=passed; path=/");
        assert_ne!(
            before,
            c.cookie_fingerprint("https://example.com/"),
            "this is what tells a reload apart from a redirect loop"
        );
    }

    #[test]
    fn candidates_walk_up_the_host_and_the_path() {
        assert_eq!(domain_candidates("www.gosuslugi.ru"), ["www.gosuslugi.ru", "gosuslugi.ru"]);
        assert_eq!(domain_candidates("example.com"), ["example.com"]);
        assert_eq!(path_candidates("/a/b"), ["/a/b", "/a", "/"]);
        assert_eq!(path_candidates("/"), ["/"]);
    }
}
