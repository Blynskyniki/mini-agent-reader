//! The native surface exposed to JavaScript as `__mar`.
//!
//! Deliberately small. Anything that can be written in JavaScript lives in
//! `prelude.js` instead; this module only covers what needs the document, the
//! page state or the host network. Fewer Rust bindings means less unsafe
//! surface and a much smaller thing to keep in step with the DOM.

use crate::dom::{DomNode, SharedDoc};
use crate::net::{HttpRequest, NetworkProvider};
use crate::state::{LogLevel, Shared};
use crate::timers::TimerKind;
use mar_dom::{LocalName, NodeData, QualName, StrTendril, ns};
use rquickjs::prelude::Func;
use rquickjs::{Class, Ctx, Function, Object, Persistent, Result, Value};
use std::rc::Rc;


// A closure written inline gets two independent lifetimes for `Ctx<'a>` and the
// `Value<'b>` it returns, which cannot be unified. These identity functions
// pin the closure to a single higher-ranked `'js`, which is what the JS engine
// actually guarantees. They compile away entirely.
fn hr_v<F>(f: F) -> F
where
    F: for<'js> Fn(Ctx<'js>) -> Result<Value<'js>>,
{
    f
}

fn hr_n<F>(f: F) -> F
where
    F: for<'js> Fn(Ctx<'js>) -> Result<Class<'js, DomNode>>,
{
    f
}

fn hr_sv<F>(f: F) -> F
where
    F: for<'js> Fn(Ctx<'js>, String) -> Result<Value<'js>>,
{
    f
}

fn hr_sn<F>(f: F) -> F
where
    F: for<'js> Fn(Ctx<'js>, String) -> Result<Class<'js, DomNode>>,
{
    f
}

fn hr_o<F>(f: F) -> F
where
    F: for<'js> Fn(Ctx<'js>) -> Result<Object<'js>>,
{
    f
}

fn hr_timer<F>(f: F) -> F
where
    F: for<'js> Fn(Ctx<'js>, Function<'js>, Option<f64>) -> Result<u32>,
{
    f
}

fn hr_timer0<F>(f: F) -> F
where
    F: for<'js> Fn(Ctx<'js>, Function<'js>) -> Result<u32>,
{
    f
}

fn hr_request<F>(f: F) -> F
where
    F: for<'js> Fn(Ctx<'js>, String, String, Object<'js>, Option<String>) -> Result<Object<'js>>,
{
    f
}

/// Expose `Node` as a global whose `.prototype` is the class prototype.
///
/// `Class::define` only installs a constructor for classes that declare one,
/// and node handles are never constructed from JS. Publishing the prototype
/// under a callable stand-in gives the prelude something to extend and makes
/// `x instanceof Node` work, since `instanceof` walks the prototype chain.
fn install_node_global(ctx: &Ctx<'_>) -> Result<()> {
    let Some(proto) = Class::<DomNode>::prototype(ctx)? else {
        return Ok(());
    };
    let ctor = Function::new(ctx.clone(), || -> Result<()> {
        Err(rquickjs::Error::Unknown)
    })?;
    ctor.set("prototype", proto)?;
    ctx.globals().set("Node", ctor)?;
    Ok(())
}

/// Install `__mar` on the globals.
pub fn install<N: NetworkProvider + 'static>(
    ctx: &Ctx<'_>,
    doc: &SharedDoc,
    state: &Shared,
    net: &Rc<N>,
) -> Result<()> {
    install_node_global(ctx)?;
    let globals = ctx.globals();
    let api = Object::new(ctx.clone())?;

    install_console(&api, state)?;
    install_timers(&api, state)?;
    install_document(ctx, &api, doc, state)?;
    install_location(&api, state)?;
    install_storage(&api, state)?;
    install_network(&api, state, net)?;
    install_url(&api)?;

    globals.set("__mar", api)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// console
// ---------------------------------------------------------------------------

fn install_console(api: &Object<'_>, state: &Shared) -> Result<()> {
    for (name, level) in [
        ("log", LogLevel::Log),
        ("info", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("error", LogLevel::Error),
        ("debug", LogLevel::Debug),
    ] {
        let st = state.clone();
        api.set(
            format!("log_{name}"),
            Func::from(move |text: String| {
                st.borrow_mut().log(level, text);
            }),
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// timers
// ---------------------------------------------------------------------------

fn install_timers(api: &Object<'_>, state: &Shared) -> Result<()> {
    fn schedule<'js>(
        state: &Shared,
        ctx: Ctx<'js>,
        callback: Function<'js>,
        delay: Option<f64>,
        kind: TimerKind,
        repeat: bool,
    ) -> Result<u32> {
        // Persistent detaches the function from the current `'js` borrow so it
        // can sit in the queue until the loop runs it.
        let persisted = Persistent::save(&ctx, callback);
        let delay_ms = delay.unwrap_or(0.0);
        let delay_ms = if delay_ms.is_finite() {
            delay_ms as i64
        } else {
            0
        };
        Ok(state
            .borrow_mut()
            .timers
            .schedule(persisted, delay_ms, kind, repeat))
    }

    let st = state.clone();
    api.set(
        "set_timeout",
        Func::from(hr_timer(move |ctx, cb, delay| {
            schedule(&st, ctx, cb, delay, TimerKind::Timeout, false)
        })),
    )?;

    let st = state.clone();
    api.set(
        "set_interval",
        Func::from(hr_timer(move |ctx, cb, delay| {
            schedule(&st, ctx, cb, delay, TimerKind::Interval, true)
        })),
    )?;

    let st = state.clone();
    api.set(
        "request_animation_frame",
        Func::from(hr_timer0(move |ctx, cb| {
            // No frames are ever painted; 16ms keeps rAF chains progressing at
            // a plausible rate on the virtual clock.
            schedule(&st, ctx, cb, Some(16.0), TimerKind::AnimationFrame, false)
        })),
    )?;

    let st = state.clone();
    api.set(
        "request_idle_callback",
        Func::from(hr_timer0(move |ctx, cb| {
            schedule(&st, ctx, cb, Some(0.0), TimerKind::Idle, false)
        })),
    )?;

    let st = state.clone();
    api.set(
        "clear_timer",
        Func::from(move |id: Option<u32>| {
            if let Some(id) = id {
                st.borrow_mut().timers.cancel(id);
            }
        }),
    )?;

    let st = state.clone();
    api.set(
        "now",
        Func::from(move || st.borrow().timers.now_ms() as f64),
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// document
// ---------------------------------------------------------------------------

fn install_document(
    ctx: &Ctx<'_>,
    api: &Object<'_>,
    doc: &SharedDoc,
    state: &Shared,
) -> Result<()> {
    // Node handles for the pieces the prelude needs to build `document`.
    let d = doc.clone();
    api.set(
        "document_element",
        Func::from(hr_v(move |ctx| {
            let id = d.borrow().document_element();
            DomNode::wrap_opt(&ctx, &d, id)
        })),
    )?;

    let d = doc.clone();
    api.set(
        "body",
        Func::from(hr_v(move |ctx| {
            let id = d.borrow().body();
            DomNode::wrap_opt(&ctx, &d, id)
        })),
    )?;

    let d = doc.clone();
    api.set(
        "head",
        Func::from(hr_v(move |ctx| {
            let id = d.borrow().head();
            DomNode::wrap_opt(&ctx, &d, id)
        })),
    )?;

    // The arena root doubles as the document node, so queries can be rooted at
    // it and reach <html>.
    let d = doc.clone();
    api.set(
        "root",
        Func::from(hr_n(move |ctx| {
            let id = d.borrow().root();
            DomNode::wrap(&ctx, &d, id)
        })),
    )?;

    let d = doc.clone();
    api.set(
        "create_element",
        Func::from(hr_sn(move |ctx, name: String| {
            let qual = QualName::new(
                None,
                ns!(html),
                LocalName::from(name.to_ascii_lowercase().as_str()),
            );
            let id = d.borrow_mut().create_element(qual, Vec::new());
            DomNode::wrap(&ctx, &d, id)
        })),
    )?;

    let d = doc.clone();
    api.set(
        "create_text_node",
        Func::from(hr_sn(move |ctx, text: String| {
            let id = d.borrow_mut().create_text(text);
            DomNode::wrap(&ctx, &d, id)
        })),
    )?;

    let d = doc.clone();
    api.set(
        "create_comment",
        Func::from(hr_sn(move |ctx, text: String| {
            let id = d
                .borrow_mut()
                .create(NodeData::Comment(StrTendril::from(text)));
            DomNode::wrap(&ctx, &d, id)
        })),
    )?;

    // Document fragments have no dedicated node kind; a detached Document node
    // behaves the same for append/query purposes.
    let d = doc.clone();
    api.set(
        "create_fragment",
        Func::from(hr_n(move |ctx| {
            let id = d.borrow_mut().create(NodeData::Document);
            DomNode::wrap(&ctx, &d, id)
        })),
    )?;

    let d = doc.clone();
    api.set(
        "get_element_by_id",
        Func::from(hr_sv(move |ctx, wanted: String| {
            let doc = d.borrow();
            let root = doc.root();
            let found = doc.descendants(root).find(|&n| {
                doc.element(n)
                    .and_then(|e| e.attr(&LocalName::from("id")))
                    .is_some_and(|v| v == wanted)
            });
            drop(doc);
            DomNode::wrap_opt(&ctx, &d, found)
        })),
    )?;

    // Address a node by its arena id. The CDP layer resolves a client's nodeId
    // to an arena id, then needs the live node to hand back as a handle.
    let d = doc.clone();
    api.set(
        "node_by_id",
        Func::from(hr_iv(move |ctx, raw: u32| {
            let id = mar_dom::NodeId::from_u32(raw);
            let exists = id.is_some_and(|id| id.index() < d.borrow().len() + 1);
            DomNode::wrap_opt(&ctx, &d, exists.then_some(id).flatten())
        })),
    )?;

    let d = doc.clone();
    api.set(
        "title",
        Func::from(move || {
            let doc = d.borrow();
            doc.head()
                .and_then(|h| {
                    doc.children(h).find(|&c| {
                        doc.element(c)
                            .is_some_and(|e| e.local_name().as_ref() == "title")
                    })
                })
                .map(|t| doc.text_content(t).trim().to_owned())
                .unwrap_or_default()
        }),
    )?;

    let d = doc.clone();
    api.set(
        "serialize",
        Func::from(move || mar_dom::document_html(&d.borrow())),
    )?;

    let st = state.clone();
    api.set(
        "cookie_get",
        Func::from(move || st.borrow().cookies.clone()),
    )?;

    let st = state.clone();
    api.set(
        "cookie_set",
        Func::from(move |raw: String| st.borrow_mut().set_cookie(&raw)),
    )?;

    let st = state.clone();
    api.set(
        "ready_state",
        Func::from(move || st.borrow().ready_state.to_string()),
    )?;

    let st = state.clone();
    api.set(
        "record_error",
        Func::from(move |source: String, message: String| {
            st.borrow_mut().record_error(source, message);
        }),
    )?;

    let _ = ctx;
    Ok(())
}

// ---------------------------------------------------------------------------
// location and navigator
// ---------------------------------------------------------------------------

fn install_location(api: &Object<'_>, state: &Shared) -> Result<()> {
    let st = state.clone();
    api.set(
        "location",
        Func::from(hr_o(move |ctx| {
            let s = st.borrow();
            let u = &s.url;
            let o = Object::new(ctx.clone())?;
            o.set("href", u.as_str())?;
            o.set("protocol", format!("{}:", u.scheme()))?;
            o.set("host", u.host_str().map(|h| match u.port() {
                Some(p) => format!("{h}:{p}"),
                None => h.to_owned(),
            }).unwrap_or_default())?;
            o.set("hostname", u.host_str().unwrap_or_default())?;
            o.set("port", u.port().map(|p| p.to_string()).unwrap_or_default())?;
            o.set("pathname", u.path())?;
            o.set("search", u.query().map(|q| format!("?{q}")).unwrap_or_default())?;
            o.set("hash", u.fragment().map(|f| format!("#{f}")).unwrap_or_default())?;
            o.set("origin", u.origin().ascii_serialization())?;
            Ok(o)
        })),
    )?;

    let st = state.clone();
    api.set(
        "navigate",
        Func::from(move |target: String| {
            // Record the intent; the caller decides whether to follow it. A
            // page cannot make us fetch something behind its own back.
            let mut s = st.borrow_mut();
            if s.requested_navigation.is_none() {
                let resolved = s.url.join(&target).map(|u| u.to_string()).unwrap_or(target);
                s.requested_navigation = Some(resolved);
            }
        }),
    )?;

    let st = state.clone();
    api.set(
        "user_agent",
        Func::from(move || st.borrow().user_agent.clone()),
    )?;

    let st = state.clone();
    api.set(
        "viewport",
        Func::from(move || {
            let (w, h) = st.borrow().viewport;
            vec![w as f64, h as f64]
        }),
    )?;

    let st = state.clone();
    api.set(
        "referrer",
        Func::from(move || st.borrow().referrer.clone()),
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// storage
// ---------------------------------------------------------------------------

fn install_storage(api: &Object<'_>, state: &Shared) -> Result<()> {
    // Both storages are in-memory and die with the page. Scripts that persist
    // a flag and read it back in the same run still work.
    let st = state.clone();
    api.set(
        "storage_get",
        Func::from(move |session: bool, key: String| {
            let s = st.borrow();
            let map = if session {
                &s.session_storage
            } else {
                &s.local_storage
            };
            map.get(&key).cloned()
        }),
    )?;

    let st = state.clone();
    api.set(
        "storage_set",
        Func::from(move |session: bool, key: String, value: String| {
            let mut s = st.borrow_mut();
            let map = if session {
                &mut s.session_storage
            } else {
                &mut s.local_storage
            };
            // Cap it; a page that writes megabytes into storage gains nothing.
            if map.len() < 1000 {
                map.insert(key, value);
            }
        }),
    )?;

    let st = state.clone();
    api.set(
        "storage_remove",
        Func::from(move |session: bool, key: String| {
            let mut s = st.borrow_mut();
            if session {
                s.session_storage.remove(&key);
            } else {
                s.local_storage.remove(&key);
            }
        }),
    )?;

    let st = state.clone();
    api.set(
        "storage_clear",
        Func::from(move |session: bool| {
            let mut s = st.borrow_mut();
            if session {
                s.session_storage.clear();
            } else {
                s.local_storage.clear();
            }
        }),
    )?;

    let st = state.clone();
    api.set(
        "storage_keys",
        Func::from(move |session: bool| {
            let s = st.borrow();
            let map = if session {
                &s.session_storage
            } else {
                &s.local_storage
            };
            map.keys().cloned().collect::<Vec<_>>()
        }),
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// network
// ---------------------------------------------------------------------------

fn install_network<N: NetworkProvider + 'static>(
    api: &Object<'_>,
    state: &Shared,
    net: &Rc<N>,
) -> Result<()> {
    let st = state.clone();
    let net = net.clone();
    api.set(
        "request",
        Func::from(hr_request(
            move |ctx, method: String, url: String, headers: Object<'_>, body: Option<String>| {
                let out = Object::new(ctx.clone())?;
                let (resolved, over_budget) = {
                    let mut s = st.borrow_mut();
                    if s.request_count >= s.limits.max_requests {
                        (None, true)
                    } else {
                        s.request_count += 1;
                        (s.url.join(&url).ok(), false)
                    }
                };
                if over_budget {
                    out.set("ok", false)?;
                    out.set("status", 0)?;
                    out.set("error", "request budget exhausted")?;
                    return Ok(out);
                }
                let Some(resolved) = resolved else {
                    out.set("ok", false)?;
                    out.set("status", 0)?;
                    out.set("error", format!("invalid URL: {url}"))?;
                    return Ok(out);
                };

                let mut header_pairs = Vec::new();
                for (k, v) in headers.props::<String, String>().flatten() {
                    header_pairs.push((k, v));
                }

                let response = net.fetch(HttpRequest {
                    method,
                    url: resolved.to_string(),
                    headers: header_pairs,
                    body,
                });

                match response {
                    Ok(r) => {
                        out.set("ok", (200..300).contains(&r.status))?;
                        out.set("status", r.status)?;
                        out.set("statusText", r.status_text)?;
                        out.set("url", r.url)?;
                        let hdrs = Object::new(ctx.clone())?;
                        for (k, v) in r.headers {
                            hdrs.set(k.to_ascii_lowercase(), v)?;
                        }
                        out.set("headers", hdrs)?;
                        out.set("body", r.body)?;
                    }
                    Err(e) => {
                        out.set("ok", false)?;
                        out.set("status", 0)?;
                        out.set("error", e)?;
                    }
                }
                Ok(out)
            },
        )),
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// URL parsing
// ---------------------------------------------------------------------------

/// Expose WHATWG URL parsing so the prelude can build `URL` on top.
///
/// Parsing is delegated to the `url` crate rather than reimplemented in
/// JavaScript: URL parsing is full of edge cases, scripts use it constantly,
/// and a subtly wrong implementation produces wrong links everywhere.
fn install_url(api: &Object<'_>) -> Result<()> {
    api.set(
        "parse_url",
        Func::from(hr_url(|ctx, input: String, base: Option<String>| {
            let parsed = match base.as_deref().filter(|b| !b.is_empty()) {
                Some(base) => url::Url::parse(base).and_then(|b| b.join(&input)),
                None => url::Url::parse(&input),
            };
            let out = Object::new(ctx.clone())?;
            let Ok(u) = parsed else {
                out.set("ok", false)?;
                return Ok(out);
            };
            out.set("ok", true)?;
            out.set("href", u.as_str())?;
            out.set("protocol", format!("{}:", u.scheme()))?;
            out.set("hostname", u.host_str().unwrap_or_default())?;
            out.set("port", u.port().map(|p| p.to_string()).unwrap_or_default())?;
            out.set(
                "host",
                match (u.host_str(), u.port()) {
                    (Some(h), Some(p)) => format!("{h}:{p}"),
                    (Some(h), None) => h.to_owned(),
                    _ => String::new(),
                },
            )?;
            out.set("pathname", u.path())?;
            out.set(
                "search",
                u.query().map(|q| format!("?{q}")).unwrap_or_default(),
            )?;
            out.set(
                "hash",
                u.fragment().map(|f| format!("#{f}")).unwrap_or_default(),
            )?;
            out.set("origin", u.origin().ascii_serialization())?;
            out.set("username", u.username())?;
            out.set("password", u.password().unwrap_or_default())?;
            // Query pairs, already percent-decoded, for URLSearchParams.
            let pairs = rquickjs::Array::new(ctx.clone())?;
            for (i, (k, v)) in u.query_pairs().enumerate() {
                let pair = rquickjs::Array::new(ctx.clone())?;
                pair.set(0, k.as_ref())?;
                pair.set(1, v.as_ref())?;
                pairs.set(i, pair)?;
            }
            out.set("pairs", pairs)?;
            Ok(out)
        })),
    )?;

    api.set(
        "encode_query",
        Func::from(|pairs: Vec<Vec<String>>| {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for pair in &pairs {
                if pair.len() == 2 {
                    serializer.append_pair(&pair[0], &pair[1]);
                }
            }
            serializer.finish()
        }),
    )?;

    Ok(())
}

fn hr_iv<F>(f: F) -> F
where
    F: for<'js> Fn(Ctx<'js>, u32) -> Result<Value<'js>>,
{
    f
}

fn hr_url<F>(f: F) -> F
where
    F: for<'js> Fn(Ctx<'js>, String, Option<String>) -> Result<Object<'js>>,
{
    f
}

/// Read a JS value as a display string without invoking user `toString`.
pub fn describe(value: &Value<'_>) -> String {
    match value.type_of() {
        rquickjs::Type::Undefined => "undefined".into(),
        rquickjs::Type::Null => "null".into(),
        rquickjs::Type::Bool => value.as_bool().unwrap_or(false).to_string(),
        rquickjs::Type::Int | rquickjs::Type::Float => value
            .as_number()
            .map(|n| n.to_string())
            .unwrap_or_default(),
        rquickjs::Type::String => value
            .as_string()
            .and_then(|s| s.to_string().ok())
            .unwrap_or_default(),
        other => format!("[{other:?}]"),
    }
}
