//! The HTTP API.
//!
//! A small pool of worker threads, each rendering one page at a time. The JS
//! engine is single-threaded per page and blocking, so threads are the natural
//! unit of concurrency here; there is no async runtime to schedule around.

use crate::pipeline::{RenderOptions, Renderer};
use mar_extract::MarkdownOptions;
use mar_js::Limits;
use mar_net::ClientConfig;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tiny_http::{Header, Request, Response, Server};

/// The request body, shared by every endpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderRequest {
    url: String,
    #[serde(default = "yes")]
    javascript: bool,
    #[serde(default = "yes")]
    external_scripts: bool,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default = "yes")]
    images: bool,
    #[serde(default = "yes")]
    links: bool,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_horizon_ms")]
    horizon_ms: i64,
    /// For `/eval` only: the expression to evaluate in the settled page.
    #[serde(default)]
    expression: Option<String>,
    /// Include console output in the response.
    #[serde(default)]
    console: bool,
}

fn yes() -> bool {
    true
}
fn default_timeout_ms() -> u64 {
    15_000
}
fn default_horizon_ms() -> i64 {
    10_000
}

impl RenderRequest {
    fn to_options(&self) -> RenderOptions {
        RenderOptions {
            javascript: self.javascript,
            external_scripts: self.external_scripts,
            limits: Limits {
                // Clamp: a caller must not be able to pin a worker forever.
                wall_ms: self.timeout_ms.clamp(1_000, 60_000),
                virtual_horizon_ms: self.horizon_ms.clamp(0, 60_000),
                ..Limits::default()
            },
            markdown: MarkdownOptions {
                base_url: None,
                include_images: self.images,
                include_links: self.links,
                max_chars: self.max_chars,
            },
            ..RenderOptions::default()
        }
    }
}

struct Shared {
    renderer: Renderer,
    token: Option<String>,
    served: AtomicU64,
}

pub fn serve(
    bind: &str,
    workers: usize,
    token: Option<String>,
    client_config: ClientConfig,
) -> anyhow::Result<()> {
    let server = Server::http(bind).map_err(|e| anyhow::anyhow!("cannot bind {bind}: {e}"))?;
    let server = Arc::new(server);
    let shared = Arc::new(Shared {
        renderer: Renderer::new(client_config),
        token,
        served: AtomicU64::new(0),
    });

    let workers = workers.max(1);
    eprintln!("mar listening on http://{bind} with {workers} workers");
    eprintln!("  POST /read    {{\"url\": \"...\"}} -> markdown + metadata");
    eprintln!("  POST /html    {{\"url\": \"...\"}} -> rendered html");
    eprintln!("  POST /eval    {{\"url\": \"...\", \"expression\": \"...\"}} -> json");
    eprintln!("  GET  /health");

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let server = server.clone();
        let shared = shared.clone();
        handles.push(std::thread::spawn(move || {
            // recv_timeout returning None just means idle; only a hard error
            // ends a worker.
            while let Ok(request) = server.recv() {
                handle(request, &shared);
            }
        }));
    }
    for handle in handles {
        // A panicking worker must not take the whole server down silently.
        if let Err(e) = handle.join() {
            eprintln!("worker thread ended unexpectedly: {e:?}");
        }
    }
    Ok(())
}

fn handle(mut request: Request, shared: &Shared) {
    let method = request.method().as_str().to_owned();
    let path = request
        .url()
        .split('?')
        .next()
        .unwrap_or("/")
        .trim_end_matches('/')
        .to_owned();
    let path = if path.is_empty() { "/".to_owned() } else { path };

    if method == "GET" && path == "/health" {
        let served = shared.served.load(Ordering::Relaxed);
        respond_json(
            request,
            200,
            &serde_json::json!({"status": "ok", "served": served}),
        );
        return;
    }

    if let Some(expected) = &shared.token {
        let presented = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("authorization"))
            .map(|h| h.value.as_str().to_owned())
            .unwrap_or_default();
        // Compare the whole "Bearer <token>" form so a bare token is rejected.
        if presented.strip_prefix("Bearer ") != Some(expected.as_str()) {
            respond_json(request, 401, &serde_json::json!({"error": "unauthorized"}));
            return;
        }
    }

    if method != "POST" {
        respond_json(
            request,
            405,
            &serde_json::json!({"error": format!("{method} not allowed on {path}")}),
        );
        return;
    }

    let mut body = String::new();
    if let Err(e) = std::io::Read::read_to_string(request.as_reader(), &mut body) {
        respond_json(
            request,
            400,
            &serde_json::json!({"error": format!("cannot read body: {e}")}),
        );
        return;
    }

    let parsed: RenderRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            respond_json(
                request,
                400,
                &serde_json::json!({"error": format!("invalid request: {e}")}),
            );
            return;
        }
    };

    shared.served.fetch_add(1, Ordering::Relaxed);
    let options = parsed.to_options();

    match path.as_str() {
        "/read" => match shared.renderer.read(&parsed.url, &options) {
            Ok((reading, mut report)) => {
                if !parsed.console {
                    report.console = None;
                }
                respond_json(
                    request,
                    200,
                    &serde_json::json!({"reading": reading, "report": report}),
                );
            }
            Err(e) => respond_error(request, &e),
        },

        "/html" => match shared.renderer.render(&parsed.url, &options) {
            Ok(rendered) => {
                let mut report = rendered.report;
                if !parsed.console {
                    report.console = None;
                }
                respond_json(
                    request,
                    200,
                    &serde_json::json!({"html": rendered.html, "report": report}),
                );
            }
            Err(e) => respond_error(request, &e),
        },

        "/eval" => {
            let Some(expression) = parsed.expression.as_deref() else {
                respond_json(
                    request,
                    400,
                    &serde_json::json!({"error": "expression is required for /eval"}),
                );
                return;
            };
            match shared.renderer.eval(&parsed.url, expression, &options) {
                Ok(json) => {
                    // The expression's result is already JSON; splice it in as
                    // a value rather than a string.
                    let value: serde_json::Value =
                        serde_json::from_str(&json).unwrap_or(serde_json::Value::Null);
                    respond_json(request, 200, &serde_json::json!({"result": value}));
                }
                Err(e) => respond_error(request, &e),
            }
        }

        other => respond_json(
            request,
            404,
            &serde_json::json!({"error": format!("no such endpoint: {other}")}),
        ),
    }
}

fn respond_error(request: Request, error: &anyhow::Error) {
    let message = format!("{error:#}");
    // A blocked URL is the caller's mistake; anything else is ours to explain.
    let status = if message.contains("blocked by policy") || message.contains("invalid URL") {
        400
    } else {
        502
    };
    respond_json(request, status, &serde_json::json!({"error": message}));
}

fn respond_json(request: Request, status: u16, body: &serde_json::Value) {
    let text = serde_json::to_string(body).unwrap_or_else(|_| r#"{"error":"serialization"}"#.into());
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
        .expect("static header is valid");
    let response = Response::from_string(text)
        .with_status_code(status)
        .with_header(header);
    // A client that hung up mid-response is normal and not worth reporting.
    let _ = request.respond(response);
}
