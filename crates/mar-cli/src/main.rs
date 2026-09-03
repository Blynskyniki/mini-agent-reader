//! `mar` — read a page the way an agent wants it.

mod pipeline;
mod server;

use clap::{Parser, Subcommand, ValueEnum};
use mar_extract::MarkdownOptions;
use mar_js::Limits;
use mar_net::Policy;
use pipeline::{RenderOptions, Renderer};
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
        /// Print the timing and cost report to stderr.
        #[arg(long)]
        report: bool,
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
            report: want_report,
        } => {
            let rendered =
                Renderer::new(egress.client_config(policy)).render(&url, &render.to_options())?;
            print!("{}", rendered.html);
            if want_report {
                eprintln!("{}", serde_json::to_string_pretty(&rendered.report)?);
            }
            Ok(())
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

/// Default request timeout for the server, exposed for tests.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
