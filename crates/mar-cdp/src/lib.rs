//! A Chrome DevTools Protocol endpoint, so existing Puppeteer, Playwright and
//! chrome-remote-interface code can drive this browser unchanged.
//!
//! The shape follows Chrome's: HTTP endpoints under `/json` for discovery, and
//! a WebSocket carrying JSON-RPC. One thread per connection owns that
//! connection's pages, because the JS engine holds `Rc` internally and a CDP
//! client drives a single browser anyway.

pub mod browser;
pub mod domains;
pub mod protocol;

use browser::Browser;
use mar_js::Limits;
use mar_net::HttpClient;
use protocol::{Command, Outgoing, version_payload};
use serde_json::json;
use base64::Engine;
use sha1::Digest;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tungstenite::Message;

#[derive(Debug, Clone)]
pub struct CdpConfig {
    pub bind: String,
    /// Required as `?token=` on the WebSocket URL and as a bearer token on the
    /// HTTP endpoints. None disables authentication.
    pub token: Option<String>,
    pub limits: Limits,
    /// Connections accepted at once.
    pub max_connections: usize,
}

impl Default for CdpConfig {
    fn default() -> Self {
        CdpConfig {
            bind: "127.0.0.1:9222".into(),
            token: None,
            limits: Limits::default(),
            max_connections: 16,
        }
    }
}

/// Run the endpoint until the process is stopped.
pub fn serve(config: CdpConfig, client: HttpClient) -> std::io::Result<()> {
    let listener = TcpListener::bind(&config.bind)?;
    let local = listener.local_addr()?;
    let config = Arc::new(config);
    let live = Arc::new(AtomicU64::new(0));

    eprintln!("CDP endpoint on ws://{local}");
    eprintln!("  puppeteer.connect({{ browserWSEndpoint: 'ws://{local}' }})");
    eprintln!("  GET http://{local}/json/version");

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if live.load(Ordering::Relaxed) as usize >= config.max_connections {
            // Refuse politely rather than queueing: each connection owns pages,
            // and an unbounded queue would be an unbounded memory commitment.
            let _ = write_http(
                &stream,
                503,
                "application/json",
                br#"{"error":"too many connections"}"#,
            );
            continue;
        }
        let config = config.clone();
        let client = client.clone();
        let live = live.clone();
        live.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            if let Err(e) = handle_connection(stream, &config, client) {
                tracing::debug!("connection ended: {e}");
            }
            live.fetch_sub(1, Ordering::Relaxed);
        });
    }
    Ok(())
}

/// Read the request line and headers, then either serve HTTP or upgrade.
fn handle_connection(
    stream: TcpStream,
    config: &CdpConfig,
    client: HttpClient,
) -> std::io::Result<()> {
    stream.set_nodelay(true)?;

    // Read exactly the request head and not a byte more. A buffered reader
    // would pull the first WebSocket frames into its buffer, and those bytes
    // would then be lost when the raw socket is handed to the frame codec.
    let head = read_request_head(&stream)?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut path = request_line.split_whitespace().nth(1).unwrap_or("/").to_owned();

    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect();

    let is_upgrade = headers
        .iter()
        .any(|(k, v)| k == "upgrade" && v.eq_ignore_ascii_case("websocket"));

    // The token may arrive as a query parameter or an Authorization header:
    // Puppeteer can carry either, depending on how it was configured.
    if let Some(expected) = &config.token {
        let query_token = path
            .split_once('?')
            .and_then(|(_, q)| {
                q.split('&')
                    .find_map(|pair| pair.strip_prefix("token=").map(str::to_owned))
            });
        let header_token = headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .and_then(|(_, v)| v.strip_prefix("Bearer ").map(str::to_owned));
        if query_token.as_deref() != Some(expected.as_str())
            && header_token.as_deref() != Some(expected.as_str())
        {
            write_http(
                &stream,
                401,
                "application/json",
                br#"{"error":"unauthorized"}"#,
            )?;
            return Ok(());
        }
    }

    if let Some((base, _)) = path.split_once('?') {
        path = base.to_owned();
    }

    if !is_upgrade {
        return serve_http(&stream, &path, config);
    }

    // The handshake is completed by hand because the request head has already
    // been consumed above; tungstenite then takes over an established socket.
    let Some(key) = headers
        .iter()
        .find(|(k, _)| k == "sec-websocket-key")
        .map(|(_, v)| v.clone())
    else {
        return write_http(
            &stream,
            400,
            "application/json",
            br#"{"error":"missing Sec-WebSocket-Key"}"#,
        );
    };

    let accept = websocket_accept(&key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    (&stream).write_all(response.as_bytes())?;
    (&stream).flush()?;

    let websocket = tungstenite::WebSocket::from_raw_socket(
        stream,
        tungstenite::protocol::Role::Server,
        None,
    );
    run_session(websocket, config, client);
    Ok(())
}

/// Read up to and including the blank line that ends an HTTP request head.
fn read_request_head(mut stream: &TcpStream) -> std::io::Result<String> {
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        if stream.read(&mut byte)? == 0 {
            break;
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        // A head this long is not a real request; refuse rather than grow.
        if head.len() > 64 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
    }
    Ok(String::from_utf8_lossy(&head).trim_end().to_owned())
}

/// The `Sec-WebSocket-Accept` value: SHA-1 of the key plus the fixed GUID from
/// RFC 6455, base64 encoded.
fn websocket_accept(key: &str) -> String {
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = sha1::Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

/// Chrome's discovery endpoints, which clients read before connecting.
fn serve_http(stream: &TcpStream, path: &str, config: &CdpConfig) -> std::io::Result<()> {
    let ws_url = format!("ws://{}/devtools/browser/mar", config.bind);
    let body = match path {
        "/json/version" => version_payload(&ws_url).to_string(),
        "/json" | "/json/list" => json!([{
            "description": "",
            "id": "mar-page-0",
            "title": "mini-agent-reader",
            "type": "page",
            "url": "about:blank",
            "webSocketDebuggerUrl": format!("ws://{}/devtools/page/mar-page-0", config.bind),
        }])
        .to_string(),
        "/json/protocol" => json!({"domains": []}).to_string(),
        "/health" => json!({"status": "ok"}).to_string(),
        _ => {
            return write_http(
                stream,
                404,
                "application/json",
                json!({"error": format!("no such endpoint: {path}")})
                    .to_string()
                    .as_bytes(),
            );
        }
    };
    write_http(stream, 200, "application/json", body.as_bytes())
}

fn write_http(
    mut stream: &TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Drive one CDP session to its end.
fn run_session<S: Read + Write>(
    mut websocket: tungstenite::WebSocket<S>,
    config: &CdpConfig,
    client: HttpClient,
) {
    let salt = format!("{:p}", &websocket);
    let mut browser = Browser::new(client, config.limits.clone(), salt);
    // Chrome always has a page open; a client that calls `pages()` before
    // creating one expects to find it.
    let first = browser.create_target(None);
    let _ = first;

    loop {
        let message = match websocket.read() {
            Ok(m) => m,
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => break,
            Err(e) => {
                tracing::debug!("websocket read failed: {e}");
                break;
            }
        };

        let text = match message {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => match String::from_utf8(bytes.to_vec()) {
                Ok(t) => t,
                Err(_) => continue,
            },
            Message::Ping(payload) => {
                let _ = websocket.send(Message::Pong(payload));
                continue;
            }
            Message::Close(_) => break,
            _ => continue,
        };

        let command: Command = match serde_json::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                // A malformed frame gets an error rather than a dropped
                // connection: clients recover from the former.
                let _ = send(
                    &mut websocket,
                    &Outgoing::error(0, None, format!("invalid message: {e}")),
                );
                continue;
            }
        };

        tracing::debug!(method = %command.method, params = %command.params, "cdp command");
        let reply = domains::dispatch(&mut browser, &command);

        if send(&mut websocket, &reply.response).is_err() {
            break;
        }
        for event in &reply.events {
            if send(&mut websocket, event).is_err() {
                return;
            }
        }
    }
}

fn send<S: Read + Write>(
    websocket: &mut tungstenite::WebSocket<S>,
    message: &Outgoing,
) -> Result<(), tungstenite::Error> {
    let text = serde_json::to_string(message)
        .unwrap_or_else(|_| r#"{"error":{"code":-32603,"message":"serialization"}}"#.into());
    websocket.send(Message::Text(text.into()))
}
