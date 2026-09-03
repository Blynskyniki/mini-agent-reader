//! `mar` — read a page the way an agent wants it.

mod pipeline;
mod server;

use clap::{Parser, Subcommand, ValueEnum};
use mar_dom::LocalName;
use mar_extract::MarkdownOptions;
use mar_js::Limits;
use mar_net::Policy;
use pipeline::{RenderOptions, Rendered, Renderer};
use std::io::Write;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "mar",
    about = "A headless browser for reading pages, without the renderer",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Allow requests to private and loopback addresses. Off by default so a
    /// caller-supplied URL cannot reach internal services.
    #[arg(long, global = true)]
    allow_private: bool,

    /// Only these hosts may be fetched. Repeatable.
    #[arg(long, global = true, value_name = "HOST")]
    allow_host: Vec<String>,

    /// Which root certificates to verify against.
    ///
    /// The default verifies against the public roots and, only if that fails,
    /// retries against a set that also includes the bundled Russian Trusted
    /// Root CA. Sites such as gosuslugi.ru and mos.ru cannot be reached
    /// without it, and the ordering means it can never override a site whose
    /// standard chain already works.
    #[arg(long, global = true, value_enum, default_value_t = TrustArg::PublicThenExtra)]
    trust: TrustArg,

    /// Additional root certificates, as a PEM bundle. Repeatable.
    #[arg(long, global = true, value_name = "FILE")]
    ca_bundle: Vec<std::path::PathBuf>,

    /// Send everything through an HTTP or SOCKS proxy, e.g.
    /// `http://127.0.0.1:8080` or `socks5://user:pass@host:1080`.
    ///
    /// This is also the answer to TLS fingerprinting: the handshake here is
    /// rustls, whose JA3 and JA4 are not Chrome's, so a site that checks
    /// exactly that wants a fingerprint-impersonating proxy in front.
    #[arg(long, global = true, env = "MAR_PROXY", value_name = "URL")]
    proxy: Option<String>,

    /// Ask each site's robots.txt before fetching a page from it.
    #[arg(long, global = true, env = "MAR_OBEY_ROBOTS")]
    obey_robots: bool,

    #[arg(long, global = true, default_value = "warn")]
    log: String,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum TrustArg {
    /// Public roots, then the bundled extras on a certificate failure.
    PublicThenExtra,
    /// Public roots only.
    PublicOnly,
    /// Public roots and the bundled extras in one trust set.
    Combined,
    /// Verify nothing. Debugging only.
    None,
}

impl From<TrustArg> for mar_net::TrustMode {
    fn from(value: TrustArg) -> Self {
        match value {
            TrustArg::PublicThenExtra => mar_net::TrustMode::PublicThenExtra,
            TrustArg::PublicOnly => mar_net::TrustMode::PublicOnly,
            TrustArg::Combined => mar_net::TrustMode::Combined,
            TrustArg::None => mar_net::TrustMode::None,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Fetch a page, run its scripts, and print the article as Markdown.
    Read {
        url: String,
        #[command(flatten)]
        render: RenderArgs,
        /// Output shape.
        #[arg(long, value_enum, default_value_t = ReadFormat::Markdown)]
        format: ReadFormat,
        /// Cap the Markdown at this many characters.
        #[arg(long)]
        max_chars: Option<usize>,
        /// Leave images out of the Markdown.
        #[arg(long)]
        no_images: bool,
        /// Keep link text but drop the URLs.
        #[arg(long)]
        no_links: bool,
    },

    /// Fetch a page, run its scripts, and print the resulting HTML.
    Fetch {
        url: String,
        #[command(flatten)]
        render: RenderArgs,
        /// What to print.
        #[arg(long, value_enum, default_value_t = Dump::Html)]
        dump: Dump,
        /// Write to this file instead of standard output.
        #[arg(long, short = 'o', value_name = "FILE")]
        output: Option<std::path::PathBuf>,
        /// Print the timing and cost report to stderr.
        #[arg(long)]
        report: bool,
    },

    /// Read many pages at once, one JSON object per line.
    ///
    /// The per-page cost of starting a process disappears here: the engine
    /// starts once and every worker reuses its connections and cookie jar.
    Scrape {
        /// URLs to read. `-` reads them from standard input, one per line.
        #[arg(required = true, value_name = "URL")]
        urls: Vec<String>,
        #[command(flatten)]
        render: RenderArgs,
        /// Pages rendered at once. Each worker holds one page in memory.
        #[arg(long, short = 'c', default_value_t = 8)]
        concurrency: usize,
        /// What to produce for each page.
        #[arg(long, value_enum, default_value_t = ScrapeShape::Read)]
        shape: ScrapeShape,
        /// With `--shape eval`, the expression to evaluate on every page.
        #[arg(long, value_name = "JS")]
        eval: Option<String>,
        /// Cap each page's Markdown at this many characters.
        #[arg(long)]
        max_chars: Option<usize>,
        /// Do not report progress on stderr.
        #[arg(long, short)]
        quiet: bool,
    },

    /// Render a page, then evaluate an expression in it and print the JSON.
    Eval {
        url: String,
        /// The expression, e.g. "[...document.querySelectorAll('h2')].map(h => h.textContent)".
        expression: String,
        #[command(flatten)]
        render: RenderArgs,
    },

    /// Serve a Chrome DevTools Protocol endpoint, so Puppeteer, Playwright and
    /// chrome-remote-interface can drive this browser unchanged.
    Cdp {
        #[arg(long, default_value = "127.0.0.1:9222")]
        bind: String,
        /// Require this token, as `?token=` on the WebSocket URL or a bearer
        /// header on the HTTP endpoints.
        #[arg(long, env = "MAR_TOKEN")]
        token: Option<String>,
        /// Connections accepted at once. Each owns its own pages.
        #[arg(long, default_value_t = 16)]
        max_connections: usize,
        #[command(flatten)]
        render: RenderArgs,
    },

    /// Show the root certificates bundled into this binary.
    Certs {
        /// Print them as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Serve the reader over HTTP.
    Serve {
        #[arg(long, default_value = "127.0.0.1:3000")]
        bind: String,
        /// Worker threads. Each renders one page at a time.
        #[arg(long, default_value_t = 4)]
        workers: usize,
        /// Require this bearer token on every request.
        #[arg(long, env = "MAR_TOKEN")]
        token: Option<String>,
    },
}

#[derive(clap::Args, Clone)]
struct RenderArgs {
    /// Do not run scripts. Much faster, and enough for server-rendered pages.
    #[arg(long)]
    no_js: bool,

    /// Do not fetch `<script src=...>`; run only inline scripts.
    #[arg(long)]
    no_external_scripts: bool,

    /// Wall-clock budget for the page, in milliseconds.
    ///
    /// Raise it for a page that is genuinely slow rather than stuck: a heavy
    /// SPA on a slow network, or a proof-of-work bot check.
    #[arg(long, default_value_t = 15_000, env = "MAR_TIMEOUT_MS")]
    timeout_ms: u64,

    /// How far the page's virtual clock may run, in milliseconds. Timers
    /// scheduled beyond this never fire.
    #[arg(long, default_value_t = 10_000, env = "MAR_HORIZON_MS")]
    horizon_ms: i64,

    /// Memory ceiling for the JavaScript heap, in megabytes.
    #[arg(long, default_value_t = 64, env = "MAR_MEMORY_MB")]
    memory_mb: usize,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum Dump {
    /// The document after scripts ran.
    Html,
    /// Its visible text.
    Text,
    /// Every link, as `text<TAB>href`, resolved absolute.
    Links,
    /// Every subresource the settled page refers to, one JSON object per line.
    Assets,
    /// The response body byte for byte, with no parsing at all.
    ///
    /// The only mode that works on something that is not a page: an image, a
    /// JSON API, a script. Nothing is decoded, so redirect it to a file.
    Original,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum ScrapeShape {
    /// The article as Markdown, with its metadata.
    Read,
    /// The document after scripts ran.
    Html,
    /// The result of `--eval` on each page.
    Eval,
    /// Every link on the page.
    Links,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum ReadFormat {
    /// Just the Markdown.
    Markdown,
    /// Markdown with a YAML front-matter block of metadata.
    Full,
    /// One JSON object: metadata, content and the cost report.
    Json,
}

impl RenderArgs {
    fn to_options(&self) -> RenderOptions {
        RenderOptions {
            javascript: !self.no_js,
            external_scripts: !self.no_external_scripts,
            limits: Limits {
                wall_ms: self.timeout_ms,
                virtual_horizon_ms: self.horizon_ms,
                memory_bytes: self.memory_mb * 1024 * 1024,
                ..Limits::default()
            },
            ..RenderOptions::default()
        }
    }
}

fn main() {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| cli.log.clone().into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let policy = Policy {
        block_private_addresses: !cli.allow_private,
        allow_hosts: cli.allow_host.clone(),
        deny_hosts: Vec::new(),
    };

    let mut ca_bundle = Vec::new();
    for path in &cli.ca_bundle {
        match mar_net::load_pem_bundle(path) {
            Ok(certs) => ca_bundle.extend(certs),
            Err(e) => {
                eprintln!("error: cannot read {}: {e}", path.display());
                std::process::exit(1);
            }
        }
    }

    let egress = Egress {
        mode: cli.trust.into(),
        extra_roots: ca_bundle,
        proxy: cli.proxy.clone(),
        obey_robots: cli.obey_robots,
    };

    if let Err(e) = run(cli.command, policy, egress) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// How this process reaches the network, gathered from the global flags.
#[derive(Clone)]
struct Egress {
    mode: mar_net::TrustMode,
    extra_roots: Vec<ureq::tls::Certificate<'static>>,
    proxy: Option<String>,
    obey_robots: bool,
}

impl Egress {
    fn client_config(&self, policy: Policy) -> mar_net::ClientConfig {
        mar_net::ClientConfig {
            policy,
            trust: self.mode,
            extra_roots: self.extra_roots.clone(),
            proxy: self.proxy.clone(),
            obey_robots: self.obey_robots,
            ..Default::default()
        }
    }
}

fn run(command: Command, policy: Policy, egress: Egress) -> anyhow::Result<()> {
    match command {
        Command::Read {
            url,
            render,
            format,
            max_chars,
            no_images,
            no_links,
        } => {
            let mut options = render.to_options();
            options.markdown = MarkdownOptions {
                base_url: None,
                include_images: !no_images,
                include_links: !no_links,
                max_chars,
            };
            let (reading, report) =
                Renderer::new(egress.client_config(policy)).read(&url, &options)?;

            let mut out = std::io::stdout().lock();
            match format {
                ReadFormat::Markdown => write!(out, "{}", reading.content)?,
                ReadFormat::Full => {
                    write!(out, "{}", front_matter(&reading))?;
                    write!(out, "{}", reading.content)?;
                }
                ReadFormat::Json => {
                    let body = serde_json::json!({
                        "reading": reading,
                        "report": report,
                    });
                    writeln!(out, "{}", serde_json::to_string_pretty(&body)?)?;
                }
            }
            Ok(())
        }

        Command::Fetch {
            url,
            render,
            dump,
            output,
            report: want_report,
        } => {
            // `original` is the one mode that never parses anything, so it is
            // also the only one that works on a URL that is not a page.
            if dump == Dump::Original {
                let mut config = egress.client_config(policy);
                config.keep_raw = true;
                let fetched = mar_net::HttpClient::new(config).get_document(&url)?;
                let bytes = fetched.raw.unwrap_or_else(|| fetched.body.into_bytes());
                return write_out(output.as_deref(), &bytes);
            }

            let rendered =
                Renderer::new(egress.client_config(policy)).render(&url, &render.to_options())?;
            let text = match dump {
                Dump::Html => rendered.html.clone(),
                Dump::Text => visible_text(&rendered),
                Dump::Links => links_of(&rendered),
                Dump::Assets => assets_of(&rendered),
                Dump::Original => unreachable!("handled above"),
            };
            write_out(output.as_deref(), text.as_bytes())?;
            if want_report {
                eprintln!("{}", serde_json::to_string_pretty(&rendered.report)?);
            }
            Ok(())
        }

        Command::Scrape {
            urls,
            render,
            concurrency,
            shape,
            eval,
            max_chars,
            quiet,
        } => {
            let mut options = render.to_options();
            options.markdown = MarkdownOptions {
                max_chars,
                ..MarkdownOptions::default()
            };
            scrape(
                collect_urls(urls)?,
                Renderer::new(egress.client_config(policy)),
                options,
                concurrency.max(1),
                shape,
                eval.as_deref(),
                quiet,
            )
        }

        Command::Eval {
            url,
            expression,
            render,
        } => {
            let json = Renderer::new(egress.client_config(policy)).eval(
                &url,
                &expression,
                &render.to_options(),
            )?;
            println!("{json}");
            Ok(())
        }

        Command::Serve {
            bind,
            workers,
            token,
        } => server::serve(&bind, workers, token, egress.client_config(policy)),

        Command::Cdp {
            bind,
            token,
            max_connections,
            render,
        } => {
            let client = mar_net::HttpClient::new(egress.client_config(policy));
            mar_cdp::serve(
                mar_cdp::CdpConfig {
                    bind,
                    token,
                    limits: render.to_options().limits,
                    max_connections,
                },
                client,
            )?;
            Ok(())
        }

        Command::Certs { json } => {
            let bundled = mar_net::bundled_certs();
            if json {
                let rows: Vec<_> = bundled
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "name": c.name, "subject": c.subject,
                            "not_after": c.not_after, "source": c.source,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                println!(
                    "Bundled root certificates, used only when the public roots reject a chain:\n"
                );
                for cert in &bundled {
                    println!("  {}", cert.subject);
                    println!("    expires {}", cert.not_after);
                    println!("    from    {}\n", cert.source);
                }
                println!("Trust order: public roots first, these only on a certificate failure.");
                println!("Change it with --trust public-only | combined | none.");
            }
            Ok(())
        }
    }
}

/// YAML front matter, so a Markdown file keeps its metadata.
fn front_matter(reading: &mar_extract::Reading) -> String {
    let mut out = String::from("---\n");
    let mut field = |name: &str, value: &Option<String>| {
        if let Some(v) = value {
            // Quote and escape: titles routinely contain colons and quotes.
            out.push_str(&format!(
                "{name}: \"{}\"\n",
                v.replace('\\', "\\\\").replace('"', "\\\"")
            ));
        }
    };
    field("title", &reading.metadata.title);
    field("author", &reading.metadata.author);
    field("site", &reading.metadata.site_name);
    field("published", &reading.metadata.published);
    field("url", &reading.metadata.canonical_url);
    field("description", &reading.metadata.description);
    out.push_str(&format!("length: {}\n", reading.length));
    if reading.low_confidence {
        out.push_str("low_confidence: true\n");
    }
    out.push_str("---\n\n");
    out
}

/// The URL list, with `-` meaning "read them from standard input".
fn collect_urls(args: Vec<String>) -> anyhow::Result<Vec<String>> {
    if args.iter().all(|u| u != "-") {
        return Ok(args);
    }
    let mut out = Vec::new();
    for arg in args {
        if arg != "-" {
            out.push(arg);
            continue;
        }
        for line in std::io::read_to_string(std::io::stdin())?.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                out.push(line.to_owned());
            }
        }
    }
    Ok(out)
}

/// Read many pages in parallel, one JSON object per line.
///
/// Lines come out as pages finish rather than in the order they were given, so
/// a slow page never holds up the ones behind it. Each line carries its URL.
fn scrape(
    urls: Vec<String>,
    renderer: Renderer,
    options: RenderOptions,
    concurrency: usize,
    shape: ScrapeShape,
    eval: Option<&str>,
    quiet: bool,
) -> anyhow::Result<()> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let total = urls.len();
    let queue = Mutex::new(urls.into_iter());
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    // One lock for stdout: a line must not be interleaved with another's.
    let out = Mutex::new(std::io::stdout());
    let started = std::time::Instant::now();

    std::thread::scope(|scope| {
        for _ in 0..concurrency.min(total.max(1)) {
            scope.spawn(|| {
                loop {
                    let Some(url) = queue.lock().expect("queue lock").next() else {
                        return;
                    };
                    let record = scrape_one(&renderer, &url, &options, shape, eval);
                    if record.get("error").is_some() {
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                    let line = record.to_string();
                    if let Ok(mut out) = out.lock() {
                        let _ = writeln!(out, "{line}");
                        let _ = out.flush();
                    }
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if !quiet {
                        eprint!("\r{n}/{total} pages");
                    }
                }
            });
        }
    });

    if !quiet {
        let failed = failed.load(Ordering::Relaxed);
        let ms = started.elapsed().as_millis().max(1);
        eprintln!(
            "\r{total} pages in {ms} ms, {:.0}/s, {failed} failed",
            total as f64 * 1000.0 / ms as f64
        );
    }
    Ok(())
}

/// One page's line. A failure is a record with an `error`, not a dropped line:
/// a caller diffing input against output should find every URL it asked for.
fn scrape_one(
    renderer: &Renderer,
    url: &str,
    options: &RenderOptions,
    shape: ScrapeShape,
    eval: Option<&str>,
) -> serde_json::Value {
    let outcome = match shape {
        ScrapeShape::Read => renderer.read(url, options).map(|(reading, report)| {
            serde_json::json!({
                "title": reading.metadata.title,
                "content": reading.content,
                "length": reading.length,
                "low_confidence": reading.low_confidence,
                "report": report,
            })
        }),
        ScrapeShape::Eval => match eval {
            Some(expression) => renderer.eval(url, expression, options).map(|json| {
                let value: serde_json::Value =
                    serde_json::from_str(&json).unwrap_or(serde_json::Value::Null);
                serde_json::json!({ "value": value })
            }),
            None => Err(anyhow::anyhow!("--shape eval needs --eval <expression>")),
        },
        ScrapeShape::Html => renderer.render(url, options).map(
            |rendered| serde_json::json!({ "html": rendered.html, "report": rendered.report }),
        ),
        ScrapeShape::Links => renderer.render(url, options).map(|rendered| {
            let links: Vec<serde_json::Value> = links_of(&rendered)
                .lines()
                .filter_map(|line| line.split_once('\t'))
                .map(|(text, href)| serde_json::json!({ "text": text, "href": href }))
                .collect();
            serde_json::json!({ "links": links, "report": rendered.report })
        }),
    };

    match outcome {
        Ok(mut value) => {
            value["url"] = serde_json::Value::String(url.to_owned());
            value
        }
        Err(e) => serde_json::json!({ "url": url, "error": format!("{e:#}") }),
    }
}

/// Write to a file, or to standard output when there is none.
///
/// Bytes rather than text: `--dump original` hands back whatever the server
/// sent, and that is frequently not UTF-8 and frequently not a page.
fn write_out(path: Option<&std::path::Path>, bytes: &[u8]) -> anyhow::Result<()> {
    match path {
        Some(path) => Ok(std::fs::write(path, bytes)?),
        None => {
            let mut out = std::io::stdout().lock();
            out.write_all(bytes)?;
            Ok(out.flush()?)
        }
    }
}

/// The settled page's visible text.
fn visible_text(rendered: &Rendered) -> String {
    let doc = &rendered.document;
    let root = doc.body().unwrap_or_else(|| doc.root());
    let mut text = doc.text_content(root);
    text.push('\n');
    text
}

/// Every link, as `text<TAB>href`.
///
/// Tab separated because a title may contain anything else, and this is meant
/// to be piped into `cut` as much as read.
fn links_of(rendered: &Rendered) -> String {
    let doc = &rendered.document;
    let base = url::Url::parse(&rendered.report.final_url).ok();
    let mut out = String::new();
    let mut seen = std::collections::HashSet::new();
    for node in mar_dom::query_selector_all(doc, doc.root(), "a[href]").unwrap_or_default() {
        let Some(href) = doc
            .element(node)
            .and_then(|e| e.attr(&LocalName::from("href")))
        else {
            continue;
        };
        let resolved = match &base {
            Some(base) => base.join(href).map(|u| u.to_string()).unwrap_or_default(),
            None => href.to_owned(),
        };
        if resolved.is_empty() || !seen.insert(resolved.clone()) {
            continue;
        }
        let text = doc.text_content(node);
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push_str(&format!("{text}\t{resolved}\n"));
    }
    out
}

/// Every subresource the settled page refers to, one JSON object per line.
///
/// Read off the document rather than off the requests we made: most of these
/// are images, fonts and stylesheets this browser never fetches, and knowing
/// what a page would pull in is the point.
fn assets_of(rendered: &Rendered) -> String {
    // Attribute per element, in the order a browser would discover them.
    const SOURCES: [(&str, &str, &str); 7] = [
        ("script[src]", "src", "script"),
        ("link[href]", "href", "link"),
        ("img[src]", "src", "image"),
        ("source[src]", "src", "media"),
        ("video[src]", "src", "media"),
        ("audio[src]", "src", "media"),
        ("iframe[src]", "src", "frame"),
    ];

    let doc = &rendered.document;
    let base = url::Url::parse(&rendered.report.final_url).ok();
    let mut out = String::new();
    let mut seen = std::collections::HashSet::new();
    for (selector, attribute, kind) in SOURCES {
        for node in mar_dom::query_selector_all(doc, doc.root(), selector).unwrap_or_default() {
            let Some(element) = doc.element(node) else {
                continue;
            };
            let Some(value) = element.attr(&LocalName::from(attribute)) else {
                continue;
            };
            let resolved = match &base {
                Some(base) => base.join(value).map(|u| u.to_string()).unwrap_or_default(),
                None => value.to_owned(),
            };
            if resolved.is_empty() || !seen.insert(resolved.clone()) {
                continue;
            }
            // `rel` distinguishes a stylesheet from a preload or an icon, and
            // that is the whole reason a link is worth listing separately.
            let rel = element.attr(&LocalName::from("rel")).unwrap_or_default();
            let record = serde_json::json!({
                "url": resolved,
                "kind": kind,
                "rel": (!rel.is_empty()).then_some(rel),
            });
            out.push_str(&record.to_string());
            out.push('\n');
        }
    }
    out
}

/// Default request timeout for the server, exposed for tests.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
