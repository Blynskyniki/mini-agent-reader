//! The wire: what actually opens sockets and speaks TLS.
//!
//! Two implementations behind one small trait. `ureq` with rustls is the
//! default: pure Rust, no system dependencies, a 6 MB binary. `browser-tls`
//! swaps in BoringSSL through `wreq` and speaks TLS and HTTP/2 exactly as a
//! current Chrome does — cipher order, extension set, GREASE, ALPS, the
//! SETTINGS frame — because a growing share of sites decide what to serve from
//! the handshake alone, and rustls' handshake is not a browser's.

use crate::tls;

/// A request as the transport sees it: policy, prefetching and header
/// shaping have already happened.
pub(crate) struct Outgoing<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub body: Option<&'a str>,
    /// Bytes past this are dropped and the response marked truncated.
    pub limit: usize,
}

/// A response as the transport hands it back: bytes, undecoded.
pub(crate) struct Incoming {
    pub status: u16,
    pub status_text: String,
    pub final_url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub truncated: bool,
}

pub(crate) trait Transport: Send + Sync {
    /// Send one request, following redirects, retrying once against the
    /// extended trust set when the public roots rejected the chain.
    fn send(&self, request: Outgoing<'_>) -> Result<Incoming, String>;

    /// What `document.cookie` reads as on `url`.
    fn cookies_for(&self, url: &str) -> String;

    /// A `document.cookie = ...` a script performed on `url`, verbatim.
    fn apply_script_cookie(&self, url: &str, raw: &str);

    fn has_extended_trust(&self) -> bool;

    fn hosts_using_extended_trust(&self) -> Vec<String>;

    /// Headers this transport sets itself and the caller must leave alone.
    fn owns_header(&self, _name: &str) -> bool {
        false
    }

    /// The user agent this transport presents, when it presents its own.
    fn user_agent(&self) -> Option<String> {
        None
    }
}

/// The certificate chain was rejected by the roots in use: the one failure
/// worth a second handshake against the extended set.
pub(crate) fn certificate_failure(message: &str) -> bool {
    tls::is_certificate_error(message)
}

pub(crate) mod ureq_transport {
    use super::{Incoming, Outgoing, Transport, certificate_failure};
    use crate::{ClientConfig, tls};
    use std::collections::HashSet;
    use std::sync::RwLock;

    /// rustls through ureq: the default, and the one with no build
    /// dependencies.
    pub struct UreqTransport {
        agent: ureq::Agent,
        /// Used only to retry a request the primary agent rejected on
        /// certificate grounds. See [`tls::TrustMode`].
        fallback_agent: Option<ureq::Agent>,
        /// Hosts already known to need the fallback, so the second and
        /// later requests to one skip the handshake that is going to fail.
        needs_fallback: RwLock<HashSet<String>>,
    }

    impl UreqTransport {
        pub fn new(config: &ClientConfig) -> Result<Self, String> {
            // The bundled roots plus anything the caller added.
            let mut extra = tls::extra_roots();
            extra.extend(config.extra_roots.iter().cloned());

            // A proxy that will not parse must not degrade into going
            // direct. The caller asked for egress through somewhere
            // specific, and silently ignoring that is how a scrape leaks
            // its real address.
            let proxy = match config.proxy.as_deref().map(ureq::Proxy::new) {
                Some(Ok(p)) => Some(p),
                Some(Err(e)) => {
                    return Err(format!("proxy {:?} is not usable: {e}", config.proxy));
                }
                // Nothing asked for explicitly: honour `HTTPS_PROXY` and
                // friends the way curl and every browser do. Passing `None`
                // here would tell ureq to ignore the environment, and on a
                // machine that reaches the outside world only through a
                // proxy every foreign site then hangs in the handshake until
                // the budget runs out.
                None => ureq::Proxy::try_from_env(),
            };

            let build = |tls_config: ureq::tls::TlsConfig| -> ureq::Agent {
                ureq::Agent::config_builder()
                    .timeout_global(Some(config.timeout))
                    // Redirects are followed here rather than in the engine
                    // so the page only ever sees the final URL.
                    .max_redirects(config.max_redirects)
                    .save_redirect_history(true)
                    .user_agent(&config.user_agent)
                    .tls_config(tls_config)
                    .proxy(proxy.clone())
                    // A 403 or a 503 is where the interesting bodies live: a
                    // bot check, a consent gate, a "you are not welcome"
                    // page. Treating the status as a transport error throws
                    // that body away.
                    .http_status_as_error(false)
                    .build()
                    .into()
            };

            Ok(UreqTransport {
                agent: build(tls::primary_config(config.trust, &extra)),
                fallback_agent: tls::fallback_config(config.trust, &extra).map(build),
                needs_fallback: RwLock::new(HashSet::new()),
            })
        }

        /// Every agent that could serve a request, primary first.
        ///
        /// Each carries its own jar, and which one a host lands on is
        /// decided by its certificate chain, so a cookie has to be written
        /// to both to survive a switch between them.
        fn agents(&self) -> impl Iterator<Item = &ureq::Agent> {
            std::iter::once(&self.agent).chain(self.fallback_agent.iter())
        }

        /// The cookies for `url` that ureq will not send.
        ///
        /// ureq withholds any cookie whose value is not RFC 6265 clean, and
        /// a bot check's cookie is routinely `972,2700,0,0,0`. A browser
        /// sends it regardless, and the check only passes when it comes
        /// back, so these go out in a `Cookie` header of their own
        /// alongside ureq's.
        fn withheld_cookies(&self, url: &str) -> String {
            let compliant = |value: &str| {
                let inner = value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(value);
                inner.bytes().all(|b| {
                    b.is_ascii()
                        && !b.is_ascii_control()
                        && !b.is_ascii_whitespace()
                        && !matches!(b, b'"' | b',' | b';' | b'\\')
                })
            };
            self.cookies_for(url)
                .split("; ")
                .filter(|pair| !pair.is_empty())
                .filter(|pair| pair.split_once('=').is_some_and(|(_, v)| !compliant(v)))
                .collect::<Vec<_>>()
                .join("; ")
        }
    }

    impl Transport for UreqTransport {
        fn send(&self, request: Outgoing<'_>) -> Result<Incoming, String> {
            let parsed = url::Url::parse(request.url).map_err(|e| e.to_string())?;
            let mut header_pairs: Vec<(String, String)> = request.headers.to_vec();
            if !header_pairs
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("cookie"))
            {
                let withheld = self.withheld_cookies(request.url);
                if !withheld.is_empty() {
                    header_pairs.push(("Cookie".to_owned(), withheld));
                }
            }

            let body = request.body.filter(|b| !b.is_empty());
            let uri = request.url.to_owned();
            let method = request.method.to_owned();

            // Built fresh for each attempt: `Agent::run` consumes the
            // request, and a certificate retry needs an identical copy.
            let build = || -> Result<ureq::http::request::Builder, String> {
                let mut builder = ureq::http::Request::builder()
                    .method(method.as_str())
                    .uri(&uri);
                for (name, value) in &header_pairs {
                    builder = builder.header(name.as_str(), value.as_str());
                }
                Ok(builder)
            };

            // A bodyless request must carry no body at all. Handing ureq an
            // empty string instead produces `Content-Length: 0` on a GET,
            // which no browser sends and which a bot check reads as an
            // automated client.
            let send =
                |agent: &ureq::Agent| -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
                    let builder = match build() {
                        Ok(b) => b,
                        Err(_) => return Err(ureq::Error::BadUri(uri.clone())),
                    };
                    match body {
                        Some(text) => agent.run(builder.body(text).map_err(ureq::Error::Http)?),
                        None => agent.run(builder.body(()).map_err(ureq::Error::Http)?),
                    }
                };

            // A host already known to need the extended trust set skips
            // straight to it rather than repeating a handshake that is going
            // to fail.
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

            // Retry once against the extended trust set when the public
            // roots rejected the chain. Only a certificate failure
            // qualifies: a refused connection or a timeout would fail the
            // same way twice.
            if !known_fallback
                && let Some(fallback) = &self.fallback_agent
                && let Err(e) = &outcome
                && certificate_failure(&e.to_string())
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

            let mut response = outcome.map_err(|e| e.to_string())?;
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
            // +1 so a body exactly at the limit is not reported as truncated.
            let mut raw = response
                .body_mut()
                .with_config()
                .limit((request.limit + 1) as u64)
                .read_to_vec()
                .map_err(|e| e.to_string())?;
            let truncated = raw.len() > request.limit;
            raw.truncate(request.limit);
            Ok(Incoming {
                status,
                status_text,
                final_url,
                headers,
                body: raw,
                truncated,
            })
        }

        fn cookies_for(&self, url: &str) -> String {
            let Ok(parsed) = url::Url::parse(url) else {
                return String::new();
            };
            // The jar can only be queried by an exact domain, path and name
            // triple, so the candidates a browser would match against are
            // enumerated and the jar decides which of them it holds.
            let domains = super::domain_candidates(parsed.host_str().unwrap_or_default());
            let paths = super::path_candidates(parsed.path());

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

        fn apply_script_cookie(&self, url: &str, raw: &str) {
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

        fn has_extended_trust(&self) -> bool {
            self.fallback_agent.is_some()
        }

        fn hosts_using_extended_trust(&self) -> Vec<String> {
            self.needs_fallback
                .read()
                .map(|set| {
                    let mut hosts: Vec<String> = set.iter().cloned().collect();
                    hosts.sort();
                    hosts
                })
                .unwrap_or_default()
        }
    }
}

/// A host and every parent a cookie could have been scoped to.
///
/// `www.gosuslugi.ru` yields itself and `gosuslugi.ru`, which is what a
/// `Domain=` attribute one level up produces.
pub(crate) fn domain_candidates(host: &str) -> Vec<String> {
    let host = host.trim_start_matches('.').to_ascii_lowercase();
    let mut out = vec![host.clone()];
    let labels: Vec<&str> = host.split('.').collect();
    for cut in 1..labels.len().saturating_sub(1) {
        out.push(labels[cut..].join("."));
    }
    out
}

/// A path and every prefix directory of it, longest first, as cookie
/// matching walks upward from the request path to the root.
pub(crate) fn path_candidates(path: &str) -> Vec<String> {
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

#[cfg(feature = "browser-tls")]
pub(crate) mod browser {
    //! Chrome's handshake and HTTP/2, through BoringSSL.
    //!
    //! `wreq` emulates a named Chrome release end to end: the ClientHello
    //! Chrome sends (its JA3 and JA4), its HTTP/2 SETTINGS and priority
    //! frames (Akamai's fingerprint), and its default header set and order.
    //! What is left to this engine is to run the page. The client is
    //! asynchronous; the engine is not, so every request is `block_on` a
    //! runtime kept for the purpose.

    use super::{Incoming, Outgoing, Transport, certificate_failure};
    use crate::{ClientConfig, tls};
    use futures_util::StreamExt;
    use std::collections::HashSet;
    use std::sync::{Arc, RwLock};
    use wreq::cookie::{CookieStore, Cookies, Jar};
    use wreq::tls::trust::{CertStore, CertificateInput};
    use wreq_util::Profile;

    /// The Chrome release presented. Its user agent is what the page sees
    /// in `navigator.userAgent`, so the two cannot disagree.
    const EMULATION: Profile = Profile::Chrome149;
    const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                              (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

    /// The whole chain: wreq's top-level message is "client error
    /// (Connect)", and the part that says *why* — the certificate verify
    /// that failed, the connection that was refused — is in its sources.
    /// The trust fallback decides on that part.
    fn describe(error: wreq::Error) -> String {
        let mut message = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(inner) = source {
            let text = inner.to_string();
            if !message.contains(&text) {
                message.push_str(": ");
                message.push_str(&text);
            }
            source = inner.source();
        }
        message
    }

    /// The jar, serialising its cookies into one `cookie` field.
    ///
    /// wreq splits them into one field per cookie over HTTP/2, as RFC 9113
    /// permits. Chrome does not, and a bot check that reads the first field
    /// and compares it with what its script set sees a different client —
    /// nic.ru and reg.ru both loop on their challenge page over that.
    struct OneField(Arc<Jar>);

    impl CookieStore for OneField {
        fn set_cookies(
            &self,
            cookie_headers: &mut dyn Iterator<Item = &wreq::header::HeaderValue>,
            uri: &wreq::Uri,
        ) {
            self.0.set_cookies(cookie_headers, uri);
        }

        fn cookies(&self, uri: &wreq::Uri, _version: wreq::Version) -> Cookies {
            self.0.cookies(uri, wreq::Version::HTTP_11)
        }
    }

    pub struct BrowserTransport {
        runtime: tokio::runtime::Runtime,
        primary: wreq::Client,
        extended: Option<wreq::Client>,
        jar: Arc<Jar>,
        needs_fallback: RwLock<HashSet<String>>,
    }

    impl BrowserTransport {
        pub fn new(config: &ClientConfig) -> Result<Self, String> {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("mar-net")
                .enable_all()
                .build()
                .map_err(|e| format!("tokio runtime: {e}"))?;
            let jar = Arc::new(Jar::default());

            let proxy = match config.proxy.as_deref() {
                Some(p) => Some(
                    wreq::Proxy::all(p)
                        .map_err(|e| format!("proxy {:?} is not usable: {e}", config.proxy))?,
                ),
                // Left unset, wreq reads `HTTPS_PROXY` and friends itself.
                None => None,
            };

            // Mozilla's roots plus the bundled extras and the caller's, in
            // one store, for the modes that want them together.
            let combined = || -> Result<CertStore, String> {
                let mut pem: Vec<&[u8]> = tls::extra_roots_pem();
                pem.extend(config.extra_roots_pem.iter().map(|p| p.as_slice()));
                CertStore::builder()
                    .add_der_certs(
                        webpki_root_certs::TLS_SERVER_ROOT_CERTS
                            .iter()
                            .map(|c| CertificateInput::Raw(c.as_ref())),
                    )
                    .add_pem_certs(pem.into_iter().map(CertificateInput::Raw))
                    .build()
                    .map_err(|e| format!("certificate store: {e}"))
            };

            let build = |store: Option<CertStore>| -> Result<wreq::Client, String> {
                let mut builder = wreq::Client::builder()
                    .emulation(EMULATION)
                    .cookie_provider(Arc::new(OneField(jar.clone())))
                    .redirect(wreq::redirect::Policy::limited(
                        config.max_redirects as usize,
                    ))
                    .timeout(config.timeout);
                if let Some(proxy) = proxy.clone() {
                    builder = builder.proxy(proxy);
                }
                if let Some(store) = store {
                    builder = builder.tls_cert_store(store);
                }
                if matches!(config.trust, tls::TrustMode::None) {
                    builder = builder.tls_cert_verification(false);
                }
                builder.build().map_err(|e| format!("http client: {e}"))
            };

            let (primary, extended) = match config.trust {
                tls::TrustMode::Combined => (build(Some(combined()?))?, None),
                tls::TrustMode::PublicThenExtra => (build(None)?, Some(build(Some(combined()?))?)),
                tls::TrustMode::PublicOnly | tls::TrustMode::None => (build(None)?, None),
            };

            Ok(BrowserTransport {
                runtime,
                primary,
                extended,
                jar,
                needs_fallback: RwLock::new(HashSet::new()),
            })
        }

        async fn request(
            client: &wreq::Client,
            request: &Outgoing<'_>,
        ) -> Result<Incoming, String> {
            let method =
                wreq::Method::from_bytes(request.method.as_bytes()).map_err(|e| e.to_string())?;
            let mut builder = client.request(method, request.url);
            for (name, value) in request.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            if let Some(body) = request.body.filter(|b| !b.is_empty()) {
                builder = builder.body(body.to_owned());
            }
            let response = builder.send().await.map_err(describe)?;
            let status = response.status().as_u16();
            let status_text = response
                .status()
                .canonical_reason()
                .unwrap_or_default()
                .to_owned();
            let final_url = response.uri().to_string();
            let headers: Vec<(String, String)> = response
                .headers()
                .iter()
                .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_owned())))
                .collect();

            let mut body = Vec::new();
            let mut truncated = false;
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(describe)?;
                let room = request.limit.saturating_sub(body.len());
                if chunk.len() > room {
                    body.extend_from_slice(&chunk[..room]);
                    truncated = true;
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Incoming {
                status,
                status_text,
                final_url,
                headers,
                body,
                truncated,
            })
        }
    }

    impl Transport for BrowserTransport {
        fn send(&self, request: Outgoing<'_>) -> Result<Incoming, String> {
            let host = url::Url::parse(request.url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
                .unwrap_or_default();
            let known_fallback = self
                .needs_fallback
                .read()
                .map(|set| set.contains(&host))
                .unwrap_or(false);
            let first = match (known_fallback, &self.extended) {
                (true, Some(client)) => client,
                _ => &self.primary,
            };
            let mut outcome = self.runtime.block_on(Self::request(first, &request));
            if !known_fallback
                && let Some(extended) = &self.extended
                && let Err(e) = &outcome
                && certificate_failure(e)
            {
                tracing::debug!(host = %host, "retrying with the extended trust set");
                let retried = self.runtime.block_on(Self::request(extended, &request));
                if retried.is_ok()
                    && let Ok(mut set) = self.needs_fallback.write()
                {
                    set.insert(host);
                }
                outcome = retried;
            }
            outcome
        }

        fn cookies_for(&self, url: &str) -> String {
            self.jar
                .matches(url)
                .map(|c| format!("{}={}", c.name(), c.value()))
                .collect::<Vec<_>>()
                .join("; ")
        }

        fn apply_script_cookie(&self, url: &str, raw: &str) {
            self.jar.add(raw, url);
        }

        fn has_extended_trust(&self) -> bool {
            self.extended.is_some()
        }

        fn hosts_using_extended_trust(&self) -> Vec<String> {
            self.needs_fallback
                .read()
                .map(|set| {
                    let mut hosts: Vec<String> = set.iter().cloned().collect();
                    hosts.sort();
                    hosts
                })
                .unwrap_or_default()
        }

        /// Chrome's own values for these come with the emulation, in
        /// Chrome's order; a second copy in a different order is a tell.
        fn owns_header(&self, name: &str) -> bool {
            let lower = name.to_ascii_lowercase();
            matches!(
                lower.as_str(),
                "user-agent"
                    | "accept-encoding"
                    | "sec-ch-ua"
                    | "sec-ch-ua-mobile"
                    | "sec-ch-ua-platform"
            )
        }

        fn user_agent(&self) -> Option<String> {
            Some(USER_AGENT.to_owned())
        }
    }
}
