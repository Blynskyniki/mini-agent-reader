//! CDP method dispatch.
//!
//! The set below is what a Puppeteer or Playwright script actually calls on the
//! path from `connect` through `newPage`, `goto`, `evaluate`, `$`, `content`
//! and `close`. Methods outside it return a protocol error naming the method,
//! which is what Chrome does for anything it does not implement.

use crate::browser::{Browser, DEFAULT_CONTEXT};
use crate::protocol::{Command, Outgoing};
use mar_dom::{LocalName, Matcher};
use mar_js::{Limits, Page};
use mar_net::PageNetwork;
use serde_json::{Value, json};
use url::Url;

/// Everything one command produced: a reply, plus any events to push after it.
pub struct Reply {
    pub response: Outgoing,
    pub events: Vec<Outgoing>,
}

impl Reply {
    fn just(response: Outgoing) -> Self {
        Reply {
            response,
            events: Vec::new(),
        }
    }
}

pub fn dispatch(browser: &mut Browser, command: &Command) -> Reply {
    let id = command.id;
    let session = command.session_id.clone();
    let method = command.method.as_str();

    match method {
        // ---- Browser ----------------------------------------------------
        "Browser.getVersion" => Reply::just(Outgoing::ok(
            id,
            session,
            json!({
                "protocolVersion": "1.3",
                "product": concat!("mini-agent-reader/", env!("CARGO_PKG_VERSION")),
                "revision": "0",
                "userAgent": browser.effective_user_agent(),
                "jsVersion": "quickjs-ng",
            }),
        )),
        "Browser.close" | "Browser.setDownloadBehavior" | "Browser.getWindowForTarget" => {
            Reply::just(Outgoing::empty(id, session))
        }

        // ---- Target -----------------------------------------------------
        "Target.getBrowserContexts" => Reply::just(Outgoing::ok(
            id,
            session,
            json!({"browserContextIds": [DEFAULT_CONTEXT]}),
        )),
        "Target.createBrowserContext" => Reply::just(Outgoing::ok(
            id,
            session,
            json!({"browserContextId": DEFAULT_CONTEXT}),
        )),
        "Target.disposeBrowserContext" => Reply::just(Outgoing::empty(id, session)),

        "Target.setDiscoverTargets" => {
            browser.discover_targets = command.bool_param("discover").unwrap_or(false);
            // Discovery means "tell me about what already exists", so every
            // current target is announced immediately.
            let events = if browser.discover_targets {
                browser
                    .targets
                    .iter()
                    .map(|t| Outgoing::event("Target.targetCreated", json!({"targetInfo": t.info()})))
                    .collect()
            } else {
                Vec::new()
            };
            Reply {
                response: Outgoing::empty(id, session),
                events,
            }
        }

        "Target.setAutoAttach" => {
            browser.auto_attach = command.bool_param("autoAttach").unwrap_or(false);
            Reply::just(Outgoing::empty(id, session))
        }

        "Target.getTargets" => {
            let infos: Vec<Value> = browser.targets.iter().map(|t| t.info()).collect();
            Reply::just(Outgoing::ok(id, session, json!({"targetInfos": infos})))
        }

        "Target.getTargetInfo" => {
            let index = command
                .str_param("targetId")
                .and_then(|t| browser.index_of_target(t))
                .or(Some(0))
                .filter(|&i| i < browser.targets.len());
            match index {
                Some(i) => Reply::just(Outgoing::ok(
                    id,
                    session,
                    json!({"targetInfo": browser.targets[i].info()}),
                )),
                None => Reply::just(Outgoing::error(id, session, "no such target")),
            }
        }

        "Target.createTarget" => {
            let index = browser.create_target(command.str_param("url"));
            let info = browser.targets[index].info();
            let target_id = browser.targets[index].id.clone();
            let mut events = vec![Outgoing::event(
                "Target.targetCreated",
                json!({"targetInfo": info}),
            )];
            // With auto-attach on, Puppeteer expects the session without asking.
            if browser.auto_attach {
                let session_id = browser.attach(index);
                events.push(attached_event(&session_id, &browser.targets[index].info()));
            }
            Reply {
                response: Outgoing::ok(id, session, json!({"targetId": target_id})),
                events,
            }
        }

        "Target.attachToTarget" => {
            let Some(index) = command
                .str_param("targetId")
                .and_then(|t| browser.index_of_target(t))
            else {
                return Reply::just(Outgoing::error(id, session, "no such target"));
            };
            let session_id = match browser.targets[index].session_id.clone() {
                Some(existing) => existing,
                None => browser.attach(index),
            };
            let info = browser.targets[index].info();
            Reply {
                response: Outgoing::ok(id, session, json!({"sessionId": session_id})),
                events: vec![attached_event(&session_id, &info)],
            }
        }

        "Target.detachFromTarget" => {
            if let Some(sid) = command.str_param("sessionId")
                && let Some(i) = browser.index_of_session(sid)
            {
                browser.targets[i].session_id = None;
            }
            Reply::just(Outgoing::empty(id, session))
        }

        "Target.closeTarget" => {
            let Some(target_id) = command.str_param("targetId").map(str::to_owned) else {
                return Reply::just(Outgoing::error(id, session, "targetId is required"));
            };
            // The session has to be reported detached before the target is
            // reported gone. A client waits for both, and closing a page hangs
            // if it only ever sees the second.
            let detached_session = browser
                .index_of_target(&target_id)
                .and_then(|i| browser.targets[i].session_id.clone());
            let closed = browser.close_target(&target_id);

            let mut events = Vec::new();
            if let Some(detached) = detached_session {
                events.push(Outgoing::event(
                    "Target.detachedFromTarget",
                    json!({"sessionId": detached, "targetId": target_id}),
                ));
            }
            events.push(Outgoing::event(
                "Target.targetDestroyed",
                json!({"targetId": target_id}),
            ));
            Reply {
                response: Outgoing::ok(id, session, json!({"success": closed})),
                events,
            }
        }

        "Target.activateTarget" | "Target.setRemoteLocations" => {
            Reply::just(Outgoing::empty(id, session))
        }

        // ---- Page -------------------------------------------------------
        "Page.enable" | "Page.disable" | "Page.setLifecycleEventsEnabled"
        | "Page.setInterceptFileChooserDialog" | "Page.setBypassCSP"
        | "Page.stopLoading" | "Page.bringToFront" => {
            Reply::just(Outgoing::empty(id, session))
        }

        "Page.close" => {
            // Same teardown as Target.closeTarget, addressed by session.
            let Some(index) = session.as_deref().and_then(|s| browser.index_of_session(s)) else {
                return Reply::just(Outgoing::empty(id, session));
            };
            let target_id = browser.targets[index].id.clone();
            let detached = browser.targets[index].session_id.clone();
            browser.targets.remove(index);

            let mut events = Vec::new();
            if let Some(detached) = detached {
                events.push(Outgoing::event(
                    "Target.detachedFromTarget",
                    json!({"sessionId": detached, "targetId": target_id}),
                ));
            }
            events.push(Outgoing::event(
                "Target.targetDestroyed",
                json!({"targetId": target_id}),
            ));
            Reply {
                response: Outgoing::empty(id, session),
                events,
            }
        }

        "Page.getFrameTree" => with_target(browser, command, |target| {
            Ok((target.frame_tree(), Vec::new()))
        }),

        "Page.getNavigationHistory" => with_target(browser, command, |target| {
            Ok((
                json!({
                    "currentIndex": 0,
                    "entries": [{
                        "id": 1,
                        "url": target.url,
                        "userTypedURL": target.url,
                        "title": target.title,
                        "transitionType": "typed",
                    }],
                }),
                Vec::new(),
            ))
        }),

        "Page.addScriptToEvaluateOnNewDocument" => {
            let source = command.str_param("source").unwrap_or_default().to_owned();
            with_target(browser, command, move |target| {
                target.init_scripts.push(source.clone());
                Ok((
                    json!({"identifier": target.init_scripts.len().to_string()}),
                    Vec::new(),
                ))
            })
        }

        "Page.createIsolatedWorld" => {
            let world_name = command
                .str_param("worldName")
                .unwrap_or("isolated")
                .to_owned();
            with_target(browser, command, move |target| {
                let context_id = target.next_context_id;
                target.next_context_id += 1;
                target.worlds.push(world_name.clone());
                let session = target.session_id.clone().unwrap_or_default();
                let frame = target.frame_id.clone();
                // The client blocks until it sees the context it just asked
                // for, so the event goes out with the reply.
                let event = Outgoing::session_event(
                    &session,
                    "Runtime.executionContextCreated",
                    json!({"context": {
                        "id": context_id,
                        "origin": target.url,
                        "name": world_name,
                        "uniqueId": format!("{frame}.{context_id}"),
                        "auxData": {
                            "isDefault": false,
                            "type": "isolated",
                            "frameId": frame,
                        },
                    }}),
                );
                Ok((json!({"executionContextId": context_id}), vec![event]))
            })
        }

        "Page.navigate" => navigate(browser, command),

        "Page.reload" => {
            // Re-navigating to the current URL is what a reload amounts to here.
            let url = command
                .session_id
                .as_deref()
                .and_then(|s| browser.index_of_session(s))
                .map(|i| browser.targets[i].url.clone());
            match url {
                Some(url) => navigate_to(browser, command, &url),
                None => Reply::just(Outgoing::empty(id, session)),
            }
        }

        "Page.getLayoutMetrics" => {
            let (w, h) = browser.viewport;
            Reply::just(Outgoing::ok(
                id,
                session,
                json!({
                    "layoutViewport": {"pageX": 0, "pageY": 0, "clientWidth": w, "clientHeight": h},
                    "visualViewport": {
                        "offsetX": 0, "offsetY": 0, "pageX": 0, "pageY": 0,
                        "clientWidth": w, "clientHeight": h, "scale": 1, "zoom": 1,
                    },
                    "contentSize": {"x": 0, "y": 0, "width": w, "height": h},
                    "cssLayoutViewport": {"pageX": 0, "pageY": 0, "clientWidth": w, "clientHeight": h},
                    "cssContentSize": {"x": 0, "y": 0, "width": w, "height": h},
                }),
            ))
        }

        // Screenshots and PDFs need the renderer this project does not have.
        // Saying so plainly beats returning a blank image a caller would ship.
        "Page.captureScreenshot" | "Page.printToPDF" | "Page.captureSnapshot" => {
            Reply::just(Outgoing::error(
                id,
                session,
                format!(
                    "{method} is not supported: this browser has no renderer. \
                     Use Runtime.evaluate or DOM.getOuterHTML for content."
                ),
            ))
        }

        // ---- Runtime ----------------------------------------------------
        "Runtime.enable" => {
            let events = command
                .session_id
                .as_deref()
                .and_then(|s| browser.index_of_session(s))
                .map(|i| {
                    let target = &browser.targets[i];
                    vec![Outgoing::session_event(
                        target.session_id.as_deref().unwrap_or_default(),
                        "Runtime.executionContextCreated",
                        json!({"context": {
                            "id": 1,
                            "origin": target.url,
                            "name": "",
                            "uniqueId": format!("{}.1", target.frame_id),
                            "auxData": {
                                "isDefault": true,
                                "type": "default",
                                "frameId": target.frame_id,
                            },
                        }}),
                    )]
                })
                .unwrap_or_default();
            Reply {
                response: Outgoing::empty(id, session),
                events,
            }
        }
        "Runtime.releaseObject" => {
            if let Some(handle) = handle_of(command.params.get("objectId"))
                && let Some(i) = session.as_deref().and_then(|s| browser.index_of_session(s))
                && let Some(page) = browser.targets[i].page.as_mut()
            {
                page.release_handle(handle);
            }
            Reply::just(Outgoing::empty(id, session))
        }

        "Runtime.disable" | "Runtime.releaseObjectGroup"
        | "Runtime.runIfWaitingForDebugger" | "Runtime.addBinding"
        | "Runtime.setAsyncCallStackDepth" => Reply::just(Outgoing::empty(id, session)),

        "Runtime.getProperties" => {
            let Some(handle) = handle_of(command.params.get("objectId")) else {
                return Reply::just(Outgoing::error(id, session, "objectId is required"));
            };
            let own_only = command.bool_param("ownProperties").unwrap_or(true);
            let index = match session.as_deref().and_then(|s| browser.index_of_session(s)) {
                Some(i) => i,
                None if browser.targets.len() == 1 => 0,
                None => {
                    return Reply::just(Outgoing::error(id, session, "no target for this session"));
                }
            };
            let Some(page) = browser.targets[index].page.as_mut() else {
                return Reply::just(Outgoing::error(id, session, "no page"));
            };
            match page.handle_properties(handle, own_only) {
                Ok(json) => {
                    let properties: Value =
                        serde_json::from_str(&json).unwrap_or_else(|_| json!([]));
                    Reply::just(Outgoing::ok(
                        id,
                        session,
                        json!({"result": properties, "internalProperties": []}),
                    ))
                }
                Err(e) => Reply::just(Outgoing::error(id, session, e)),
            }
        }

        // Turn a DOM node id into a handle the client can call functions on.
        "DOM.resolveNode" => {
            let node_id = command
                .int_param("nodeId")
                .or_else(|| command.int_param("backendNodeId"));
            let Some(index) = session
                .as_deref()
                .and_then(|s| browser.index_of_session(s))
                .or(if browser.targets.len() == 1 { Some(0) } else { None })
            else {
                return Reply::just(Outgoing::error(id, session, "no target for this session"));
            };
            let target = &mut browser.targets[index];
            let Some(node) = node_id.and_then(|n| target.resolve(n)) else {
                return Reply::just(Outgoing::error(id, session, "no such node"));
            };
            let name = target
                .document
                .as_ref()
                .and_then(|d| d.element(node))
                .map(|e| e.local_name().as_ref().to_ascii_uppercase())
                .unwrap_or_else(|| "#node".into());
            let Some(page) = target.page.as_mut() else {
                return Reply::just(Outgoing::error(id, session, "no page"));
            };
            match page.handle_for_node(node) {
                Ok(handle) => Reply::just(Outgoing::ok(
                    id,
                    session,
                    json!({"object": {
                        "type": "object", "subtype": "node",
                        "className": name, "description": name,
                        "objectId": handle.to_string(),
                    }}),
                )),
                Err(e) => Reply::just(Outgoing::error(id, session, e)),
            }
        }

        "Runtime.evaluate" => evaluate(browser, command, EvalKind::Expression),
        "Runtime.callFunctionOn" => evaluate(browser, command, EvalKind::FunctionOn),

        // ---- DOM --------------------------------------------------------
        "DOM.enable" | "DOM.disable" | "DOM.setNodeValue" => {
            Reply::just(Outgoing::empty(id, session))
        }

        "DOM.getDocument" => with_target(browser, command, |target| {
            let Some(doc) = target.document.as_ref() else {
                return Err("no document: navigate first".into());
            };
            let root = doc.root();
            let url = target.url.clone();
            let node_id = target.handle(root);
            Ok((
                json!({"root": {
                    "nodeId": node_id,
                    "backendNodeId": node_id,
                    "nodeType": 9,
                    "nodeName": "#document",
                    "localName": "",
                    "nodeValue": "",
                    "childNodeCount": 1,
                    "documentURL": url,
                    "baseURL": url,
                    "xmlVersion": "",
                }}),
                Vec::new(),
            ))
        }),

        "DOM.querySelector" | "DOM.querySelectorAll" => query(browser, command),

        "DOM.getOuterHTML" => with_target(browser, command, |target| {
            let node_id = command
                .int_param("nodeId")
                .or_else(|| command.int_param("backendNodeId"));
            let doc = target
                .document
                .as_ref()
                .ok_or_else(|| "no document: navigate first".to_owned())?;
            let node = match node_id {
                Some(n) => target.resolve(n).ok_or_else(|| "no such node".to_owned())?,
                None => doc.root(),
            };
            Ok((
                json!({"outerHTML": mar_dom::outer_html(doc, node)}),
                Vec::new(),
            ))
        }),

        "DOM.getAttributes" => with_target(browser, command, |target| {
            let node_id = command
                .int_param("nodeId")
                .ok_or_else(|| "nodeId is required".to_owned())?;
            let node = target.resolve(node_id).ok_or_else(|| "no such node".to_owned())?;
            let doc = target
                .document
                .as_ref()
                .ok_or_else(|| "no document".to_owned())?;
            // CDP returns attributes as a flat [name, value, name, value] list.
            let mut flat = Vec::new();
            if let Some(el) = doc.element(node) {
                for attr in &el.attrs {
                    flat.push(Value::from(attr.name.local.to_string()));
                    flat.push(Value::from(attr.value.to_string()));
                }
            }
            Ok((json!({"attributes": flat}), Vec::new()))
        }),

        "DOM.describeNode" => with_target(browser, command, |target| {
            // A client that got a node as a handle from `Runtime` describes it
            // by objectId; one that got it from `DOM.querySelector` uses a
            // nodeId. Both have to work, and Puppeteer uses the first.
            let node = match handle_of(command.params.get("objectId")) {
                Some(handle) => target
                    .page
                    .as_mut()
                    .and_then(|p| p.node_of_handle(handle))
                    .ok_or_else(|| "objectId does not refer to a node".to_owned())?,
                None => {
                    let node_id = command.int_param("nodeId").unwrap_or(1);
                    target.resolve(node_id).ok_or_else(|| "no such node".to_owned())?
                }
            };
            let node_id = target.handle(node);
            let doc = target
                .document
                .as_ref()
                .ok_or_else(|| "no document".to_owned())?;
            let data = doc.data(node);
            let name = match data {
                mar_dom::NodeData::Element(e) => e.local_name().as_ref().to_ascii_uppercase(),
                mar_dom::NodeData::Text(_) => "#text".into(),
                _ => "#other".into(),
            };
            Ok((
                json!({"node": {
                    "nodeId": node_id,
                    "backendNodeId": node_id,
                    "nodeType": data.node_type(),
                    "nodeName": name,
                    "localName": name.to_ascii_lowercase(),
                    "nodeValue": "",
                    "childNodeCount": doc.children(node).count(),
                }}),
                Vec::new(),
            ))
        }),

        // ---- Network and Emulation --------------------------------------
        "Network.enable" | "Network.disable" | "Network.clearBrowserCache"
        | "Network.clearBrowserCookies" | "Network.setCacheDisabled"
        | "Network.setCookies" | "Network.setCookie" | "Network.emulateNetworkConditions" => {
            Reply::just(Outgoing::empty(id, session))
        }

        "Network.setUserAgentOverride" | "Emulation.setUserAgentOverride" => {
            if let Some(ua) = command.str_param("userAgent") {
                browser.user_agent_override = Some(ua.to_owned());
            }
            Reply::just(Outgoing::empty(id, session))
        }

        "Network.setExtraHTTPHeaders" => {
            if let Some(map) = command.params.get("headers").and_then(Value::as_object) {
                browser.extra_headers = map
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect();
            }
            Reply::just(Outgoing::empty(id, session))
        }

        "Emulation.setDeviceMetricsOverride" => {
            // Nothing is laid out, but scripts read these, so the numbers a
            // page sees should be the ones the client asked for.
            let width = command.int_param("width").unwrap_or(1280).max(1) as u32;
            let height = command.int_param("height").unwrap_or(800).max(1) as u32;
            browser.viewport = (width, height);
            Reply::just(Outgoing::empty(id, session))
        }

        "Emulation.setDefaultBackgroundColorOverride"
        | "Emulation.setScriptExecutionDisabled"
        | "Emulation.setEmulatedMedia"
        | "Emulation.setTouchEmulationEnabled"
        | "Emulation.setTimezoneOverride"
        | "Emulation.setLocaleOverride"
        | "Emulation.clearDeviceMetricsOverride" => Reply::just(Outgoing::empty(id, session)),

        // ---- Everything else --------------------------------------------
        //
        // `Domain.enable` and `Domain.disable` are subscriptions: they ask to
        // start or stop receiving that domain's events and return nothing.
        // Succeeding on one for a domain we never emit events from is honest —
        // no events arrive, which is the correct outcome — and it is what lets
        // an unmodified Puppeteer or Playwright session get as far as the
        // methods that do return data.
        other if other.ends_with(".enable") || other.ends_with(".disable") => {
            tracing::debug!(method = other, "subscribed to a domain with no events");
            Reply::just(Outgoing::empty(id, session))
        }

        // Overrides and preferences that only matter to a renderer.
        "Security.setIgnoreCertificateErrors"
        | "Debugger.setPauseOnExceptions"
        | "Debugger.setAsyncCallStackDepth"
        | "Profiler.setSamplingInterval"
        | "Overlay.setShowViewportSizeOnResize"
        | "Animation.setPlaybackRate"
        | "Input.setIgnoreInputEvents"
        | "Accessibility.disable" => Reply::just(Outgoing::empty(id, session)),

        other => Reply::just(Outgoing::not_found(id, session, other)),
    }
}

fn attached_event(session_id: &str, info: &Value) -> Outgoing {
    Outgoing::event(
        "Target.attachedToTarget",
        json!({
            "sessionId": session_id,
            "targetInfo": info,
            "waitingForDebugger": false,
        }),
    )
}

/// Run `f` against the target the command is routed to.
fn with_target<F>(browser: &mut Browser, command: &Command, f: F) -> Reply
where
    F: FnOnce(&mut crate::browser::Target) -> Result<(Value, Vec<Outgoing>), String>,
{
    let session = command.session_id.clone();
    let index = match session.as_deref().and_then(|s| browser.index_of_session(s)) {
        Some(i) => i,
        // A command with no session is aimed at the browser, but several
        // clients send page commands unsessioned when there is only one page.
        None if browser.targets.len() == 1 => 0,
        None => {
            return Reply::just(Outgoing::error(
                command.id,
                session,
                "no target for this session",
            ));
        }
    };
    match f(&mut browser.targets[index]) {
        Ok((result, events)) => Reply {
            response: Outgoing::ok(command.id, session, result),
            events,
        },
        Err(message) => Reply::just(Outgoing::error(command.id, session, message)),
    }
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

fn navigate(browser: &mut Browser, command: &Command) -> Reply {
    let Some(url) = command.str_param("url").map(str::to_owned) else {
        return Reply::just(Outgoing::error(
            command.id,
            command.session_id.clone(),
            "url is required",
        ));
    };
    navigate_to(browser, command, &url)
}

fn navigate_to(browser: &mut Browser, command: &Command, url: &str) -> Reply {
    let session = command.session_id.clone();
    let index = match session.as_deref().and_then(|s| browser.index_of_session(s)) {
        Some(i) => i,
        None if browser.targets.len() == 1 => 0,
        None => {
            return Reply::just(Outgoing::error(command.id, session, "no target for this session"));
        }
    };

    let parsed = match Url::parse(url) {
        Ok(u) => u,
        Err(e) => {
            return Reply::just(Outgoing::error(
                command.id,
                session,
                format!("invalid URL {url}: {e}"),
            ));
        }
    };

    let fetched = match browser.client.get_document(parsed.as_str()) {
        Ok(f) => f,
        Err(e) => {
            // CDP reports a failed navigation in the reply, not as an error, so
            // Puppeteer's `goto` rejects with the reason rather than hanging.
            return Reply::just(Outgoing::ok(
                command.id,
                session,
                json!({
                    "frameId": browser.targets[index].frame_id,
                    "loaderId": browser.targets[index].loader_id,
                    "errorText": e.to_string(),
                }),
            ));
        }
    };

    let base = Url::parse(&fetched.final_url).unwrap_or(parsed);
    let document = mar_dom::parse_html(&fetched.body).document;
    let net = PageNetwork::new(browser.client.clone(), &base);

    let limits = Limits {
        ..browser.limits.clone()
    };
    let init_scripts = browser.targets[index].init_scripts.clone();
    let viewport = browser.viewport;
    let user_agent = browser.effective_user_agent();

    let outcome = match Page::with_document(document, base.clone(), limits, net) {
        Ok(mut page) => {
            {
                let mut state = page.state().borrow_mut();
                state.viewport = viewport;
                state.user_agent = user_agent;
            }
            // Scripts registered with addScriptToEvaluateOnNewDocument run
            // before the page's own, as the name promises.
            for (i, source) in init_scripts.iter().enumerate() {
                page.eval_script(source, &format!("init script #{i}"));
            }
            let outcome = page.run();
            let doc = page.document().snapshot();
            let target = &mut browser.targets[index];
            target.page = Some(page);
            target.document = Some(doc);
            Some(outcome)
        }
        Err(e) => {
            return Reply::just(Outgoing::error(
                command.id,
                session,
                format!("could not build page: {e}"),
            ));
        }
    };

    // A navigation starts a new document, so it gets a new loader id. Clients
    // key their lifecycle bookkeeping on it and ignore events carrying a stale
    // one, which is the usual reason a `goto` appears to hang.
    let loader_id = crate::browser::new_id(
        fetched.final_url.len() as u64 + command.id as u64,
        &fetched.final_url,
    );

    let target = &mut browser.targets[index];
    target.url = fetched.final_url.clone();
    target.title = outcome.as_ref().map(|o| o.title.clone()).unwrap_or_default();
    target.loader_id = loader_id.clone();
    target.reset_nodes();

    let frame_id = target.frame_id.clone();
    let session_for_events = target.session_id.clone().unwrap_or_default();
    let info = target.info();
    let origin = base.origin().ascii_serialization();

    let lifecycle = |name: &str| {
        Outgoing::session_event(
            &session_for_events,
            "Page.lifecycleEvent",
            json!({
                "frameId": frame_id, "loaderId": loader_id,
                "name": name, "timestamp": 0.0,
            }),
        )
    };

    // Everything already happened synchronously, so the sequence below is
    // replayed in the order a client would have observed it live. The "init"
    // event has to come first: that is what tells the client a new document
    // has begun and which loader id the rest of the events belong to.
    let mut events = vec![
        Outgoing::session_event(
            &session_for_events,
            "Page.frameStartedLoading",
            json!({"frameId": frame_id}),
        ),
        lifecycle("init"),
        Outgoing::session_event(
            &session_for_events,
            "Runtime.executionContextsCleared",
            json!({}),
        ),
        Outgoing::session_event(
            &session_for_events,
            "Page.frameNavigated",
            json!({"frame": {
                "id": frame_id,
                "loaderId": loader_id,
                "url": fetched.final_url,
                "domainAndRegistry": "",
                "securityOrigin": origin,
                "mimeType": "text/html",
                "adFrameStatus": {"adFrameType": "none"},
                "secureContextType": "Secure",
                "crossOriginIsolatedContextType": "NotIsolated",
                "gatedAPIFeatures": [],
            }, "type": "Navigation"}),
        ),
        Outgoing::session_event(
            &session_for_events,
            "Runtime.executionContextCreated",
            json!({"context": {
                "id": 1,
                "origin": fetched.final_url,
                "name": "",
                "uniqueId": format!("{frame_id}.1"),
                "auxData": {"isDefault": true, "type": "default", "frameId": frame_id},
            }}),
        ),
        lifecycle("DOMContentLoaded"),
        Outgoing::session_event(
            &session_for_events,
            "Page.domContentEventFired",
            json!({"timestamp": 0.0}),
        ),
        lifecycle("load"),
        Outgoing::session_event(
            &session_for_events,
            "Page.loadEventFired",
            json!({"timestamp": 0.0}),
        ),
        lifecycle("networkAlmostIdle"),
        lifecycle("networkIdle"),
        Outgoing::session_event(
            &session_for_events,
            "Page.frameStoppedLoading",
            json!({"frameId": frame_id}),
        ),
        Outgoing::event("Target.targetInfoChanged", json!({"targetInfo": info})),
    ];

    // A new document means every execution context is new, so the isolated
    // worlds the client created are announced again. Without this the client
    // waits forever for the world it used before the navigation.
    let worlds: Vec<(usize, String)> = browser.targets[index]
        .worlds
        .iter()
        .cloned()
        .enumerate()
        .collect();
    for (offset, world_name) in worlds {
        let context_id = 2 + offset as i64;
        events.push(Outgoing::session_event(
            &session_for_events,
            "Runtime.executionContextCreated",
            json!({"context": {
                "id": context_id,
                "origin": fetched.final_url,
                "name": world_name,
                "uniqueId": format!("{frame_id}.{context_id}"),
                "auxData": {"isDefault": false, "type": "isolated", "frameId": frame_id},
            }}),
        ));
    }

    // Console output from the page reaches the client as Runtime events.
    if let Some(outcome) = &outcome {
        for message in outcome.console.iter().take(200) {
            events.push(Outgoing::session_event(
                &session_for_events,
                "Runtime.consoleAPICalled",
                json!({
                    "type": message.level.as_str(),
                    "args": [{"type": "string", "value": message.text}],
                    "executionContextId": 1,
                    "timestamp": message.at_ms as f64,
                }),
            ));
        }
    }

    Reply {
        response: Outgoing::ok(
            command.id,
            session,
            json!({"frameId": frame_id, "loaderId": loader_id}),
        ),
        events,
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

enum EvalKind {
    Expression,
    FunctionOn,
}

/// A CDP `objectId` is our handle id as a string.
fn handle_of(value: Option<&Value>) -> Option<u32> {
    value?.as_str()?.parse().ok()
}

fn evaluate(browser: &mut Browser, command: &Command, kind: EvalKind) -> Reply {
    let session = command.session_id.clone();
    let index = match session.as_deref().and_then(|s| browser.index_of_session(s)) {
        Some(i) => i,
        None if browser.targets.len() == 1 => 0,
        None => {
            return Reply::just(Outgoing::error(command.id, session, "no target for this session"));
        }
    };

    // Puppeteer defaults to handles; only an explicit returnByValue serializes.
    let by_value = command.bool_param("returnByValue").unwrap_or(false);
    let await_promise = command.bool_param("awaitPromise").unwrap_or(false);

    let target = &mut browser.targets[index];
    let Some(page) = target.page.as_mut() else {
        return Reply::just(Outgoing::error(
            command.id,
            session,
            "no page: navigate before evaluating",
        ));
    };

    let result = match kind {
        EvalKind::Expression => {
            let expression = command.str_param("expression").unwrap_or("undefined");
            page.eval_remote(expression, by_value, await_promise)
        }
        EvalKind::FunctionOn => {
            let declaration = command
                .str_param("functionDeclaration")
                .unwrap_or("function(){}");
            let this_handle = handle_of(command.params.get("objectId"));

            // Each argument is either a handle the client is holding or a
            // literal value. Both forms have to survive into the call, which is
            // what makes `$$eval` and `elementHandle.evaluate` work.
            let (arg_handles, arg_values) = match command.params.get("arguments") {
                Some(Value::Array(list)) => {
                    let handles: Vec<Option<u32>> =
                        list.iter().map(|a| handle_of(a.get("objectId"))).collect();
                    let values: Vec<Value> = list
                        .iter()
                        .map(|a| a.get("value").cloned().unwrap_or(Value::Null))
                        .collect();
                    (handles, Value::Array(values).to_string())
                }
                _ => (Vec::new(), "[]".to_owned()),
            };

            page.call_on_handle(
                this_handle,
                declaration,
                &arg_handles,
                &arg_values,
                by_value,
                await_promise,
            )
        }
    };

    // Evaluation can mutate the DOM, so the cached document is refreshed.
    // Node handles are deliberately kept: the client is holding ids it was
    // just given, and only a navigation makes those stale.
    let refreshed = page.document().snapshot();
    target.document = Some(refreshed);

    match result {
        Ok(json) => {
            let remote: Value = serde_json::from_str(&json)
                .unwrap_or_else(|_| json!({"type": "undefined"}));
            Reply::just(Outgoing::ok(command.id, session, json!({"result": remote})))
        }
        Err(message) => Reply::just(Outgoing::ok(
            command.id,
            session,
            json!({
                "result": {"type": "object", "subtype": "error", "description": message},
                "exceptionDetails": {
                    "exceptionId": 1,
                    "text": "Uncaught",
                    "lineNumber": 0,
                    "columnNumber": 0,
                    "exception": {
                        "type": "object", "subtype": "error", "description": message,
                    },
                },
            }),
        )),
    }
}

/// Wrap a JSON value as a CDP `Runtime.RemoteObject`.
fn remote_object(value: Value) -> Value {
    match value {
        Value::Null => json!({"type": "object", "subtype": "null", "value": null}),
        Value::Bool(b) => json!({"type": "boolean", "value": b}),
        Value::Number(n) => json!({"type": "number", "value": n}),
        Value::String(s) => json!({"type": "string", "value": s}),
        Value::Array(_) => json!({"type": "object", "subtype": "array", "value": value}),
        Value::Object(_) => json!({"type": "object", "value": value}),
    }
}

// ---------------------------------------------------------------------------
// DOM queries
// ---------------------------------------------------------------------------

fn query(browser: &mut Browser, command: &Command) -> Reply {
    let all = command.method.ends_with("All");
    let selector = command.str_param("selector").unwrap_or("").to_owned();
    let root_id = command.int_param("nodeId");

    with_target(browser, command, move |target| {
        let doc = target
            .document
            .as_ref()
            .ok_or_else(|| "no document: navigate first".to_owned())?;
        let root = match root_id {
            Some(n) => target.resolve(n).unwrap_or_else(|| doc.root()),
            None => doc.root(),
        };
        let matcher = Matcher::new(&selector).map_err(|e| e.to_string())?;
        let found = if all {
            matcher.query_all(doc, root)
        } else {
            matcher.query_first(doc, root).into_iter().collect()
        };
        // handle() needs &mut, so the nodes are collected before registering.
        let handles: Vec<i64> = found.into_iter().map(|n| target.handle(n)).collect();
        Ok((
            if all {
                json!({"nodeIds": handles})
            } else {
                json!({"nodeId": handles.first().copied().unwrap_or(0)})
            },
            Vec::new(),
        ))
    })
}

/// Text of a node, used by clients that read `innerText` through CDP.
pub fn node_text(doc: &mar_dom::Document, node: mar_dom::NodeId) -> String {
    doc.text_content(node)
}

/// Value of an attribute on a node.
pub fn node_attr(doc: &mar_dom::Document, node: mar_dom::NodeId, name: &str) -> Option<String> {
    doc.element(node)?
        .attr(&LocalName::from(name))
        .map(str::to_owned)
}
