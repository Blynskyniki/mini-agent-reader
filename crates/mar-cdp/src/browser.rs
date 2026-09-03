//! Browser and target state behind the CDP endpoint.
//!
//! A CDP client sees a browser holding page targets. Here a page target owns a
//! rendered document and, once navigated, the JS context that produced it.
//!
//! Everything is single-threaded per connection. The JS engine holds `Rc`
//! internally and is deliberately not `Send`, and a CDP client drives one
//! browser, so a thread per connection is the natural fit.

use crate::fetch::{Desk, Intercepted};
use mar_js::{Limits, Page};
use mar_net::HttpClient;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use url::Url;

/// Identifier handed to the client for a target. CDP clients treat these as
/// opaque, but Chrome uses 32 uppercase hex characters and some tooling
/// assumes that shape.
pub fn new_id(counter: u64, salt: &str) -> String {
    let mut out = String::with_capacity(32);
    // A hash keeps ids stable per connection without pulling in a uuid crate.
    let mut h: u64 = 0xcbf29ce484222325;
    for byte in salt.as_bytes().iter().chain(&counter.to_le_bytes()) {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    for i in 0..4 {
        out.push_str(&format!("{:08X}", (h >> (i * 8)) as u32 ^ counter as u32));
    }
    out
}

/// One page the client can drive.
pub struct Target {
    pub id: String,
    pub url: String,
    pub title: String,
    /// Session id once the client has attached. CDP routes by this.
    pub session_id: Option<String>,
    /// The rendered page. Absent until the first navigation.
    pub page: Option<Page<Intercepted>>,
    /// The document as it stood after the last navigation, for DOM queries
    /// that do not need a live JS context.
    pub document: Option<mar_dom::Document>,
    /// Nodes handed to the client, so `DOM.getOuterHTML` and friends can
    /// resolve a `nodeId` back to the tree.
    pub node_handles: HashMap<i64, mar_dom::NodeId>,
    pub next_node_id: i64,
    /// Scripts the client asked to run before every document.
    pub init_scripts: Vec<String>,
    /// Isolated worlds the client asked for, by name. Everything here shares
    /// one real JS context; a separate world would only matter if the page
    /// could observe the client's helper globals, and nothing renders here for
    /// it to observe them with.
    pub worlds: Vec<String>,
    pub next_context_id: i64,
    pub loader_id: String,
    pub frame_id: String,
}

impl Target {
    pub fn new(id: String, frame_id: String, loader_id: String) -> Self {
        Target {
            url: "about:blank".into(),
            title: String::new(),
            session_id: None,
            page: None,
            document: None,
            node_handles: HashMap::new(),
            next_node_id: 1,
            init_scripts: Vec::new(),
            worlds: Vec::new(),
            // 1 is the main world; isolated worlds count up from there.
            next_context_id: 2,
            id,
            loader_id,
            frame_id,
        }
    }

    /// `Target.TargetInfo`, as several methods and events must report it.
    pub fn info(&self) -> Value {
        json!({
            "targetId": self.id,
            "type": "page",
            "title": self.title,
            "url": self.url,
            "attached": self.session_id.is_some(),
            "canAccessOpener": false,
            "browserContextId": DEFAULT_CONTEXT,
        })
    }

    /// `Page.FrameTree` for a single-frame page. Nothing here loads iframes,
    /// so the tree is always one node deep.
    pub fn frame_tree(&self) -> Value {
        json!({
            "frameTree": {
                "frame": {
                    "id": self.frame_id,
                    "loaderId": self.loader_id,
                    "url": self.url,
                    "domainAndRegistry": "",
                    "securityOrigin": origin_of(&self.url),
                    "mimeType": "text/html",
                    "adFrameStatus": {"adFrameType": "none"},
                    "secureContextType": "Secure",
                    "crossOriginIsolatedContextType": "NotIsolated",
                    "gatedAPIFeatures": [],
                },
                "childFrames": [],
            }
        })
    }

    /// Register a node so the client can address it later.
    pub fn handle(&mut self, node: mar_dom::NodeId) -> i64 {
        let id = self.next_node_id;
        self.next_node_id += 1;
        self.node_handles.insert(id, node);
        id
    }

    pub fn resolve(&self, node_id: i64) -> Option<mar_dom::NodeId> {
        self.node_handles.get(&node_id).copied()
    }

    /// Forget the handles from a previous document.
    pub fn reset_nodes(&mut self) {
        self.node_handles.clear();
        self.next_node_id = 1;
    }

    /// Re-read the document after something ran that could have changed it.
    /// Node handles survive: the client is holding ids it was just given, and
    /// only a navigation makes those stale.
    pub fn refresh(&mut self) {
        if let Some(page) = self.page.as_mut() {
            self.document = Some(page.document().snapshot());
        }
    }

    /// The frame and loader a request from this page belongs to.
    pub fn frame(&self) -> crate::fetch::Frame {
        crate::fetch::Frame {
            frame_id: self.frame_id.clone(),
            loader_id: self.loader_id.clone(),
        }
    }
}

pub const DEFAULT_CONTEXT: &str = "MARDEFAULTBROWSERCONTEXT00000000";

fn origin_of(url: &str) -> String {
    Url::parse(url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|_| "://".into())
}

/// The browser a single CDP connection sees.
pub struct Browser {
    pub targets: Vec<Target>,
    pub client: HttpClient,
    pub limits: Limits,
    /// Whether the client asked to be told about targets as they appear.
    pub discover_targets: bool,
    pub auto_attach: bool,
    counter: u64,
    salt: String,
    /// Overrides the client set through `Emulation`/`Network`.
    pub user_agent_override: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub viewport: (u32, u32),
    /// Request interception, and the response bodies it saw go past. Shared
    /// with every page this connection builds, because that is where requests
    /// pass through.
    pub fetch: Rc<RefCell<Desk>>,
    /// The connection's cookie jar, seeded into each page as it is built.
    pub cookies: Vec<(String, String)>,
}

impl Browser {
    pub fn new(client: HttpClient, limits: Limits, salt: String) -> Self {
        Browser {
            targets: Vec::new(),
            client,
            limits,
            discover_targets: false,
            auto_attach: false,
            counter: 0,
            salt,
            user_agent_override: None,
            extra_headers: Vec::new(),
            viewport: (1280, 800),
            fetch: Desk::shared(),
            cookies: Vec::new(),
        }
    }

    fn next_id(&mut self) -> String {
        self.counter += 1;
        new_id(self.counter, &self.salt)
    }

    pub fn create_target(&mut self, url: Option<&str>) -> usize {
        let id = self.next_id();
        let frame = self.next_id();
        let loader = self.next_id();
        let mut target = Target::new(id, frame, loader);
        if let Some(url) = url
            && url != "about:blank"
        {
            target.url = url.to_owned();
        }
        self.targets.push(target);
        self.targets.len() - 1
    }

    pub fn attach(&mut self, index: usize) -> String {
        let session = self.next_id();
        self.targets[index].session_id = Some(session.clone());
        session
    }

    pub fn index_of_target(&self, target_id: &str) -> Option<usize> {
        self.targets.iter().position(|t| t.id == target_id)
    }

    pub fn index_of_session(&self, session_id: &str) -> Option<usize> {
        self.targets
            .iter()
            .position(|t| t.session_id.as_deref() == Some(session_id))
    }

    pub fn close_target(&mut self, target_id: &str) -> bool {
        match self.index_of_target(target_id) {
            Some(i) => {
                self.targets.remove(i);
                true
            }
            None => false,
        }
    }

    /// The cookies the client is asking about.
    ///
    /// A live page is the authority while it exists, because scripts write to
    /// `document.cookie` and a client reading them back expects to see that.
    pub fn jar(&self) -> Vec<(String, String)> {
        match self.targets.iter().find_map(|t| t.page.as_ref()) {
            Some(page) => page.state().borrow().cookie_pairs(),
            None => self.cookies.clone(),
        }
    }

    /// Write the jar back, to the connection and to any page holding a copy.
    pub fn set_jar(&mut self, pairs: Vec<(String, String)>) {
        let serialized = crate::cookies::serialize(&pairs);
        for target in &mut self.targets {
            if let Some(page) = target.page.as_ref() {
                page.state().borrow_mut().cookies = serialized.clone();
            }
        }
        self.cookies = pairs;
    }

    /// The user agent a page should report, honouring any override.
    pub fn effective_user_agent(&self) -> String {
        self.user_agent_override
            .clone()
            .unwrap_or_else(|| mar_js::default_user_agent().to_owned())
    }
}
