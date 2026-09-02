//! Chrome DevTools Protocol message shapes.
//!
//! CDP is JSON-RPC with two additions that matter here: a message may carry a
//! `sessionId` routing it to one attached target, and the server pushes events
//! that have no `id`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A command from the client.
#[derive(Debug, Deserialize)]
pub struct Command {
    pub id: i64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    /// Present when the command is aimed at an attached target rather than the
    /// browser itself.
    #[serde(default, rename = "sessionId")]
    pub session_id: Option<String>,
}

impl Command {
    /// Read a string parameter.
    pub fn str_param(&self, name: &str) -> Option<&str> {
        self.params.get(name)?.as_str()
    }

    /// Read an integer parameter.
    pub fn int_param(&self, name: &str) -> Option<i64> {
        self.params.get(name)?.as_i64()
    }

    pub fn bool_param(&self, name: &str) -> Option<bool> {
        self.params.get(name)?.as_bool()
    }
}

/// A reply to a command, or an event.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Outgoing {
    Result {
        id: i64,
        result: Value,
        #[serde(skip_serializing_if = "Option::is_none", rename = "sessionId")]
        session_id: Option<String>,
    },
    Error {
        id: i64,
        error: ProtocolError,
        #[serde(skip_serializing_if = "Option::is_none", rename = "sessionId")]
        session_id: Option<String>,
    },
    Event {
        method: String,
        params: Value,
        #[serde(skip_serializing_if = "Option::is_none", rename = "sessionId")]
        session_id: Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub struct ProtocolError {
    pub code: i32,
    pub message: String,
}

impl Outgoing {
    pub fn ok(id: i64, session: Option<String>, result: Value) -> Self {
        Outgoing::Result {
            id,
            result,
            session_id: session,
        }
    }

    /// An empty successful reply, which most CDP setters return.
    pub fn empty(id: i64, session: Option<String>) -> Self {
        Outgoing::ok(id, session, json!({}))
    }

    pub fn error(id: i64, session: Option<String>, message: impl Into<String>) -> Self {
        Outgoing::Error {
            id,
            // -32601 is JSON-RPC "method not found"; CDP reuses the codes.
            error: ProtocolError {
                code: -32000,
                message: message.into(),
            },
            session_id: session,
        }
    }

    pub fn not_found(id: i64, session: Option<String>, method: &str) -> Self {
        Outgoing::Error {
            id,
            error: ProtocolError {
                code: -32601,
                message: format!("'{method}' wasn't found"),
            },
            session_id: session,
        }
    }

    pub fn event(method: impl Into<String>, params: Value) -> Self {
        Outgoing::Event {
            method: method.into(),
            params,
            session_id: None,
        }
    }

    pub fn session_event(session: &str, method: impl Into<String>, params: Value) -> Self {
        Outgoing::Event {
            method: method.into(),
            params,
            session_id: Some(session.to_owned()),
        }
    }
}

/// What `GET /json/version` returns. Puppeteer reads this before connecting.
pub fn version_payload(ws_url: &str) -> Value {
    json!({
        // Puppeteer parses the Chrome version out of this string to decide
        // which protocol quirks apply, so it has to look like a Chrome UA.
        "Browser": concat!("mini-agent-reader/", env!("CARGO_PKG_VERSION")),
        "Protocol-Version": "1.3",
        "User-Agent": mar_js::default_user_agent(),
        "V8-Version": "quickjs-ng",
        "WebKit-Version": "0",
        "webSocketDebuggerUrl": ws_url,
    })
}
