//! Fetch, render, extract: the whole journey from a URL to Markdown.

use mar_dom::{Document, LocalName, NodeId, StrTendril};
use mar_extract::{MarkdownOptions, Reading};
use mar_js::{Limits, Page};
use mar_net::{ClientConfig, HttpClient, PageNetwork};
use serde::Serialize;
use std::time::Instant;
use url::Url;

/// How to handle one page.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Run scripts. Off is much faster and enough for server-rendered pages.
    pub javascript: bool,
    /// Fetch and run `<script src=...>`. Off runs only inline scripts.
    pub external_scripts: bool,
    /// External scripts fetched before giving up on the rest.
    pub max_external_scripts: usize,
    /// Follow a `location.href` assignment or `location.replace` call made by
    /// a script, the way a browser would.
    pub follow_client_navigation: bool,
    pub limits: Limits,
    pub markdown: MarkdownOptions,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            javascript: true,
            external_scripts: true,
            max_external_scripts: 12,
            follow_client_navigation: true,
            limits: Limits::default(),
            markdown: MarkdownOptions::default(),
        }
    }
}

/// What one page cost and produced.
#[derive(Debug, Serialize)]
pub struct RenderReport {
    pub url: String,
    pub final_url: String,
    pub status: u16,
    pub charset: String,
    /// What the response actually held: html, pdf, json, image and so on.
    pub content_kind: String,
    /// Whether scripts ran at all.
    pub javascript: bool,
    pub scripts_inlined: usize,
    pub scripts_run: usize,
    pub timer_callbacks: u64,
    pub subresource_requests: usize,
    /// Virtual milliseconds the page's own clock reached.
    pub virtual_ms: i64,
    pub fetch_ms: u128,
    pub render_ms: u128,
    pub extract_ms: u128,
    pub total_ms: u128,
    /// Script errors, capped. Present when something did not run cleanly.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// Console output, only when the caller asked for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub console: Option<Vec<String>>,
    /// A navigation the page requested and we did not follow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_navigation: Option<String>,
    /// True when the settle loop hit a limit instead of going quiet.
    pub truncated: bool,
    /// Set when the response looks like a refusal rather than a page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

/// A rendered page plus what it cost.
pub struct Rendered {
    pub document: Document,
    pub report: RenderReport,
    /// HTML after scripts ran.
    pub html: String,
}

pub struct Renderer {
    client: HttpClient,
}

impl Renderer {
    pub fn new(config: ClientConfig) -> Self {
        Renderer {
            client: HttpClient::new(config),
        }
    }

    /// Fetch `url`, run its scripts, and hand back the settled document.
    ///
    /// Follows redirects a browser would follow but HTTP does not express:
    /// `<meta http-equiv="refresh">` and a script assigning `location`. Sites
    /// that moved a URL, and consent or region gates, both work this way, so
    /// without this a caller frequently gets a stub page.
    pub fn render(&self, url: &str, options: &RenderOptions) -> anyhow::Result<Rendered> {
        let started = Instant::now();
        self.following_navigation(url, options, |target| {
            let mut rendered = self.render_once(target, options, started)?;
            rendered.report.url = url.to_owned();
            let next = rendered.report.requested_navigation.clone();
            Ok((rendered, next))
        })
    }

    /// Run `once` at `url`, then again wherever the page said to go.
    ///
    /// Shared by every entry point, because otherwise they disagree about what
    /// page they are looking at: a challenge or a consent gate serves one thing
    /// to whoever follows the navigation and another to whoever does not.
    fn following_navigation<T>(
        &self,
        url: &str,
        options: &RenderOptions,
        mut once: impl FnMut(&str) -> anyhow::Result<(T, Option<String>)>,
    ) -> anyhow::Result<T> {
        let mut target = url.to_owned();
        let mut hops = 0usize;
        let mut seen = vec![(target.clone(), self.client.cookie_fingerprint(&target))];

        loop {
            let (value, requested) = once(&target)?;

            if !options.follow_client_navigation || hops >= MAX_NAVIGATION_HOPS {
                return Ok(value);
            }
            let Some(next) = requested else {
                return Ok(value);
            };
            // A loop between two pages is common when a gate misfires; stop and
            // return what we have rather than bouncing until the hop limit.
            //
            // The same URL twice is not automatically a loop, though. A
            // challenge page computes something, sets a cookie and reloads, and
            // the second fetch is a different request carrying a different jar.
            // What makes a repeat pointless is repeating it unchanged.
            let fingerprint = self.client.cookie_fingerprint(&next);
            if seen
                .iter()
                .any(|(url, seen_at)| *url == next && *seen_at == fingerprint)
            {
                return Ok(value);
            }
            seen.push((next.clone(), fingerprint));
            target = next;
            hops += 1;
        }
    }

    fn render_once(
        &self,
        url: &str,
        options: &RenderOptions,
        started: Instant,
    ) -> anyhow::Result<Rendered> {
        let fetch_start = Instant::now();
        let mut fetched = self.client.get_document(url)?;
        let mut document = mar_dom::parse_html(&fetched.body).document;
        let mut hops = 0;
        while hops < MAX_META_REFRESH_HOPS {
            let base = Url::parse(&fetched.final_url)
                .unwrap_or_else(|_| Url::parse(url).expect("the client already parsed this URL"));
            let Some(target) = meta_refresh_target(&document, &base) else {
                break;
            };
            if target == fetched.final_url {
                break;
            }
            let Ok(next) = self.client.get_document(&target) else {
                break;
            };
            fetched = next;
            document = mar_dom::parse_html(&fetched.body).document;
            hops += 1;
        }
        let fetch_ms = fetch_start.elapsed().as_millis();

        // A URL is not a promise about content type. Handing a PDF or an image
        // to the HTML parser yields a document of mojibake that reads like text
        // and is not, so say plainly what arrived instead.
        if !fetched.is_readable_as_html() {
            return Err(anyhow::anyhow!(
                "{} is {}, not HTML ({}). This tool reads HTML pages; \
                 fetch the file directly, or use a converter for that format.",
                fetched.final_url,
                fetched.kind().as_str(),
                if fetched.content_type().is_empty() {
                    "no content-type"
                } else {
                    fetched.content_type()
                },
            ));
        }

        let base = Url::parse(&fetched.final_url)
            .unwrap_or_else(|_| Url::parse(url).expect("the client already parsed this URL"));

        let mut report = RenderReport {
            url: url.to_owned(),
            final_url: fetched.final_url.clone(),
            status: fetched.status,
            charset: fetched.charset.clone(),
            content_kind: fetched.kind().as_str().to_owned(),
            javascript: options.javascript,
            scripts_inlined: 0,
            scripts_run: 0,
            timer_callbacks: 0,
            subresource_requests: 0,
            virtual_ms: 0,
            fetch_ms,
            render_ms: 0,
            extract_ms: 0,
            total_ms: 0,
            errors: Vec::new(),
            console: None,
            requested_navigation: None,
            truncated: false,
            blocked: None,
        };

        if !options.javascript {
            report.total_ms = started.elapsed().as_millis();
            let html = mar_dom::document_html(&document);
            return Ok(Rendered {
                document,
                report,
                html,
            });
        }

        let render_start = Instant::now();
        if options.external_scripts {
            report.scripts_inlined =
                self.inline_external_scripts(&mut document, &base, options.max_external_scripts);
        }

        let net = PageNetwork::new(self.client.clone(), &base);
        let mut page = Page::with_document(document, base.clone(), options.limits.clone(), net)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // A script that reads `document.cookie` expects to find what the server
        // already set, and one that writes it expects the next request to carry
        // it. The jar lives in the client, so the page borrows it on the way in
        // and hands its writes back on the way out.
        page.state().borrow_mut().cookies = self.client.cookies_for(base.as_str());
        let outcome = page.run();
        for raw in &outcome.cookie_writes {
            self.client.apply_script_cookie(base.as_str(), raw);
        }
        report.render_ms = render_start.elapsed().as_millis();

        report.scripts_run = outcome.scripts_run;
        report.timer_callbacks = outcome.timer_callbacks;
        report.subresource_requests = outcome.requests;
        report.virtual_ms = outcome.virtual_ms;
        report.truncated = outcome.truncated;
        report.requested_navigation = outcome.requested_navigation.clone();
        report.errors = outcome
            .errors
            .iter()
            .take(20)
            .map(|e| format!("{}: {}", e.source, e.message))
            .collect();
        report.console = Some(
            outcome
                .console
                .iter()
                .map(|m| format!("[{}] {}", m.level.as_str(), m.text))
                .collect(),
        );

        let document = page.document().snapshot();
        report.total_ms = started.elapsed().as_millis();

        Ok(Rendered {
            document,
            report,
            html: outcome.html,
        })
    }

    /// Statuses that mean "we know who you are and you are not welcome".
    ///
    /// Returning an empty article for these is misleading: nothing was
    /// extracted because nothing was served.
    fn describe_block(status: u16, body_len: usize) -> Option<&'static str> {
        match status {
            403 => Some("403: the server refused the request"),
            429 => Some("429: rate limited"),
            // Several Russian sites use 439 for their bot check, and Cloudflare
            // uses 503 with a challenge page.
            439 => Some("439: the site's bot check refused the request"),
            503 if body_len < 20_000 => Some("503: likely a bot-check interstitial"),
            _ => None,
        }
    }

    /// Fetch, render and extract in one call.
    pub fn read(
        &self,
        url: &str,
        options: &RenderOptions,
    ) -> anyhow::Result<(Reading, RenderReport)> {
        let rendered = self.render(url, options)?;
        let mut report = rendered.report;

        let extract_start = Instant::now();
        // Resolve relative URLs against where the page actually came from.
        let mut markdown = options.markdown.clone();
        if markdown.base_url.is_none() {
            markdown.base_url = Url::parse(&report.final_url).ok();
        }
        let mut reading = mar_extract::read(&rendered.document, &markdown);
        report.extract_ms = extract_start.elapsed().as_millis();
        report.total_ms += report.extract_ms;

        // An empty article after a refusal status is not an empty page; it is
        // a page we were not shown. Say which.
        if reading.length < 200
            && let Some(reason) = Self::describe_block(report.status, rendered.html.len())
        {
            report.blocked = Some(reason.to_owned());
            if reading.content.trim().is_empty() {
                reading.content = format!("_Blocked: {reason}._\n");
            }
        }

        Ok((reading, report))
    }

    /// Render a page, then evaluate an expression in the settled page and
    /// return its JSON encoding.
    pub fn eval(
        &self,
        url: &str,
        expression: &str,
        options: &RenderOptions,
    ) -> anyhow::Result<String> {
        self.following_navigation(url, options, |target| {
            let fetched = self.client.get_document(target)?;
            let base = Url::parse(&fetched.final_url)
                .or_else(|_| Url::parse(target))
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let mut document = mar_dom::parse_html(&fetched.body).document;
            if options.javascript && options.external_scripts {
                self.inline_external_scripts(&mut document, &base, options.max_external_scripts);
            }

            let net = PageNetwork::new(self.client.clone(), &base);
            let mut page = Page::with_document(document, base.clone(), options.limits.clone(), net)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            page.state().borrow_mut().cookies = self.client.cookies_for(base.as_str());

            let mut requested = None;
            if options.javascript {
                let outcome = page.run();
                for raw in &outcome.cookie_writes {
                    self.client.apply_script_cookie(base.as_str(), raw);
                }
                requested = outcome.requested_navigation.clone();
            }

            // A page that is on its way somewhere else has not been asked the
            // question yet. Evaluate only where it comes to rest.
            if requested.is_some() {
                return Ok((String::new(), requested));
            }
            let value = page
                .eval_json(expression)
                .map_err(|e| anyhow::anyhow!("evaluation failed: {e}"))?;
            Ok((value, None))
        })
    }

    /// Replace `<script src=...>` with the script's own source.
    ///
    /// The engine deliberately does not fetch anything itself, so this is where
    /// the decision to load a page's own code is made and bounded. Returns how
    /// many scripts were inlined.
    fn inline_external_scripts(&self, doc: &mut Document, base: &Url, max: usize) -> usize {
        let targets: Vec<(NodeId, String, bool)> = doc
            .descendants(doc.root())
            .filter_map(|id| {
                let el = doc.element(id)?;
                if el.local_name().as_ref() != "script" {
                    return None;
                }
                let src = el.attr(&LocalName::from("src"))?;
                // A script with a body as well as a src is unusual; leave it.
                if !doc.text_content(id).trim().is_empty() {
                    return None;
                }
                let module = el
                    .attr(&LocalName::from("type"))
                    .is_some_and(|t| t.eq_ignore_ascii_case("module"));
                Some((id, src.to_owned(), module))
            })
            .take(max)
            .collect();

        let mut inlined = 0;
        for (id, src, module) in targets {
            let Ok(absolute) = base.join(&src) else {
                continue;
            };
            // Third-party classic scripts are almost always analytics and tag
            // managers: slow to fetch, and they add nothing a reader wants.
            //
            // A cross-origin *module* is a different animal. An application
            // shipping native modules keeps its bundle on a CDN, so the same
            // rule that skips the tag manager also skips the whole
            // application, and the page renders as an empty shell.
            if absolute.origin() != base.origin() && !module {
                continue;
            }
            let Ok(fetched) = self.client_execute(&absolute, base) else {
                continue;
            };
            if !fetched.ok() || fetched.body.trim().is_empty() {
                continue;
            }
            // The body goes in beside the `src` rather than replacing it: a
            // module's own imports resolve against its URL, so the engine has
            // to be able to see where this came from.
            if !module && let Some(el) = doc.element_mut(id) {
                el.remove_attr(&LocalName::from("src"));
            }
            let text = doc.create(mar_dom::NodeData::Text(StrTendril::from(fetched.body)));
            doc.append(id, text);
            inlined += 1;
        }
        inlined
    }

    fn client_execute(&self, url: &Url, referer: &Url) -> anyhow::Result<mar_net::Fetched> {
        Ok(self.client.get_script(url.as_str(), referer)?)
    }
}

/// A page may chain refreshes; a couple of hops is generous and bounds the work.
const MAX_META_REFRESH_HOPS: usize = 3;

/// Script-driven navigations followed before giving up.
const MAX_NAVIGATION_HOPS: usize = 3;

/// The URL a `<meta http-equiv="refresh">` points at, when it fires promptly.
///
/// A long delay is a "you will be redirected in 30 seconds" notice on a page
/// worth reading in its own right, so only short ones count as a redirect.
fn meta_refresh_target(doc: &Document, base: &Url) -> Option<String> {
    for id in doc.descendants(doc.root()) {
        let Some(el) = doc.element(id) else { continue };
        if el.local_name().as_ref() != "meta" {
            continue;
        }
        if el
            .attr(&LocalName::from("http-equiv"))
            .is_none_or(|v| !v.eq_ignore_ascii_case("refresh"))
        {
            continue;
        }
        let Some(content) = el.attr(&LocalName::from("content")) else {
            continue;
        };
        // The value is "<seconds>" or "<seconds>; url=<target>".
        let (delay, rest) = content.split_once(';').unwrap_or((content, ""));
        if delay.trim().parse::<f64>().unwrap_or(f64::MAX) > 5.0 {
            continue;
        }
        let target = rest
            .trim()
            .strip_prefix("url")
            .or_else(|| rest.trim().strip_prefix("URL"))
            .and_then(|r| r.trim_start().strip_prefix('='))
            .map(|r| r.trim().trim_matches(['"', '\'']))?;
        if target.is_empty() {
            continue;
        }
        return base.join(target).ok().map(|u| u.to_string());
    }
    None
}
