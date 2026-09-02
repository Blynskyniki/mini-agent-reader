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

    #[arg(long, global = true, default_value = "warn")]
    log: String,
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
    #[arg(long, default_value_t = 15_000)]
    timeout_ms: u64,

    /// How far the page's virtual clock may run, in milliseconds. Timers
    /// scheduled beyond this never fire.
    #[arg(long, default_value_t = 10_000)]
    horizon_ms: i64,

    /// Memory ceiling for the JavaScript heap, in megabytes.
    #[arg(long, default_value_t = 64)]
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

    if let Err(e) = run(cli.command, policy) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(command: Command, policy: Policy) -> anyhow::Result<()> {
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
            let (reading, report) = Renderer::new(policy).read(&url, &options)?;

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
            let rendered = Renderer::new(policy).render(&url, &render.to_options())?;
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
            let json = Renderer::new(policy).eval(&url, &expression, &render.to_options())?;
            println!("{json}");
            Ok(())
        }

        Command::Serve {
            bind,
            workers,
            token,
        } => server::serve(&bind, workers, token, policy),
    }
}

/// YAML front matter, so a Markdown file keeps its metadata.
fn front_matter(reading: &mar_extract::Reading) -> String {
    let mut out = String::from("---\n");
    let mut field = |name: &str, value: &Option<String>| {
        if let Some(v) = value {
            // Quote and escape: titles routinely contain colons and quotes.
            out.push_str(&format!("{name}: \"{}\"\n", v.replace('\\', "\\\\").replace('"', "\\\"")));
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
