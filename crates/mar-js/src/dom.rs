//! JavaScript bindings for the arena DOM.
//!
//! The document lives in Rust behind an `Rc<RefCell<_>>`; every JS object is a
//! thin handle holding that pointer plus a `NodeId`. Nothing about the tree is
//! duplicated into the JS heap, so a large page costs the same whether or not
//! scripts walk it.

use mar_dom::{Document, LocalName, Matcher, NodeData, NodeId, QualName, StrTendril, ns};
use rquickjs::class::{Trace, Tracer};
use rquickjs::{Class, Ctx, Error, IntoJs, JsLifetime, Object, Persistent, Result, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A string argument, coerced the way the DOM coerces.
///
/// Every DOM method that takes a string takes `ToString(value)`, so
/// `setAttribute('hidden', true)`, `el.className = 1` and `textContent = 7` are
/// all ordinary code. rquickjs converts by type instead and throws
/// "Error converting from js 'bool' into type 'string'", which does not just
/// lose the call — it throws out of whatever was running. Across a 603-site
/// corpus this was the third most common script error.
pub struct JsString(pub String);

impl<'js> rquickjs::FromJs<'js> for JsString {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        // `undefined` is the one value the DOM does not stringify uniformly:
        // an omitted argument is "undefined" as a string, which is what
        // `String(undefined)` gives, so the same rule covers it.
        Ok(JsString(match value.type_of() {
            rquickjs::Type::String => value
                .as_string()
                .expect("a string value has a string")
                .to_string()?,
            _ => value
                .get::<rquickjs::Coerced<String>>()
                .map(|c| c.0)
                .or_else(|_| -> Result<String> {
                    let _ = ctx;
                    Ok(String::new())
                })?,
        }))
    }
}

impl std::ops::Deref for JsString {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl From<JsString> for String {
    fn from(value: JsString) -> String {
        value.0
    }
}

impl From<JsString> for StrTendril {
    fn from(value: JsString) -> StrTendril {
        StrTendril::from(value.0)
    }
}

/// The prototypes a node handle is created with, one per kind of node and
/// one per tag, as the prelude built them.
///
/// A `<div>` is an `HTMLDivElement`, and a page that patches
/// `HTMLTemplateElement.prototype` expects the patch to reach templates and
/// nothing else. With one prototype for every node the patch reaches every
/// node, and with none of these registered every node is a bare `Node`.
#[derive(Default)]
pub struct Prototypes {
    element: Option<Persistent<Object<'static>>>,
    document: Option<Persistent<Object<'static>>>,
    fragment: Option<Persistent<Object<'static>>>,
    text: Option<Persistent<Object<'static>>>,
    comment: Option<Persistent<Object<'static>>>,
    doctype: Option<Persistent<Object<'static>>>,
    by_tag: HashMap<String, Persistent<Object<'static>>>,
}

/// One JS object per node, for as long as the page lives.
///
/// The DOM's identity rule is that the same node is the same object: a page
/// keeps a `WeakMap` keyed by element, sets `el.__reactFiber$` and reads it
/// back off `event.target.parentNode`, and compares `a.parentNode ===
/// b.parentNode`. A fresh handle on every access breaks all three, and with
/// them React's event delegation, Alpine's scopes and anything else that
/// remembers an element.
#[derive(Default)]
pub struct Handles {
    cache: HashMap<u32, Persistent<Object<'static>>>,
    prototypes: Prototypes,
}

/// The page document, shared between Rust and every JS handle.
#[derive(Clone)]
pub struct SharedDoc {
    doc: Rc<RefCell<Document>>,
    handles: Rc<RefCell<Handles>>,
}

impl SharedDoc {
    pub fn new(doc: Document) -> Self {
        SharedDoc {
            doc: Rc::new(RefCell::new(doc)),
            handles: Rc::new(RefCell::new(Handles::default())),
        }
    }

    #[inline]
    pub fn borrow(&self) -> std::cell::Ref<'_, Document> {
        self.doc.borrow()
    }

    #[inline]
    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, Document> {
        self.doc.borrow_mut()
    }

    /// Take the document back out once scripting is done.
    pub fn into_inner(self) -> std::result::Result<Document, Self> {
        let handles = self.handles.clone();
        Rc::try_unwrap(self.doc)
            .map(RefCell::into_inner)
            .map_err(|doc| SharedDoc { doc, handles })
    }

    /// Forget every handle. Must run while the runtime that owns them is
    /// still alive: a handle freed after it is a use after free.
    pub fn clear_handles(&self) {
        if let Ok(mut handles) = self.handles.try_borrow_mut() {
            handles.cache.clear();
            handles.prototypes = Prototypes::default();
        }
    }

    /// Install the prototypes the prelude built. `tags` maps a lower-case
    /// tag name to the prototype its elements get.
    pub fn register_prototypes<'js>(
        &self,
        ctx: &Ctx<'js>,
        kinds: Object<'js>,
        tags: Object<'js>,
    ) -> Result<()> {
        let save = |name: &str| -> Result<Option<Persistent<Object<'static>>>> {
            let value: Option<Object<'js>> = kinds.get(name)?;
            Ok(value.map(|o| Persistent::save(ctx, o)))
        };
        let mut prototypes = Prototypes {
            element: save("element")?,
            document: save("document")?,
            fragment: save("fragment")?,
            text: save("text")?,
            comment: save("comment")?,
            doctype: save("doctype")?,
            by_tag: HashMap::new(),
        };
        for (tag, proto) in tags.props::<String, Object<'js>>().flatten() {
            prototypes
                .by_tag
                .insert(tag.to_ascii_lowercase(), Persistent::save(ctx, proto));
        }
        if let Ok(mut handles) = self.handles.try_borrow_mut() {
            handles.prototypes = prototypes;
        }
        Ok(())
    }

    /// The prototype a fresh handle for `id` should have, if one is registered.
    fn prototype_for<'js>(&self, ctx: &Ctx<'js>, id: NodeId) -> Option<Object<'js>> {
        let handles = self.handles.try_borrow().ok()?;
        let protos = &handles.prototypes;
        let doc = self.doc.try_borrow().ok()?;
        let chosen = match doc.data(id) {
            NodeData::Element(e) => {
                let tag = e.local_name().as_ref();
                protos.by_tag.get(tag).or(protos.element.as_ref())
            }
            NodeData::Text(_) => protos.text.as_ref(),
            NodeData::Comment(_) => protos.comment.as_ref(),
            NodeData::Document if id == doc.root() => protos.document.as_ref(),
            NodeData::Document => protos.fragment.as_ref(),
            NodeData::Doctype { .. } => protos.doctype.as_ref(),
            NodeData::ProcessingInstruction { .. } => protos.comment.as_ref(),
        }?;
        chosen.clone().restore(ctx).ok()
    }

    /// Copy the current tree out without consuming the handle.
    pub fn snapshot(&self) -> Document {
        let src = self.borrow();
        let mut out = Document::new();
        out.quirks_mode = src.quirks_mode;
        let root = out.root();
        for child in src.children(src.root()) {
            let copy = out.import_subtree(&src, child);
            out.append(root, copy);
        }
        out
    }
}

/// A DOM node handle exposed to JavaScript.
#[rquickjs::class(rename = "Node")]
pub struct DomNode {
    pub doc: SharedDoc,
    pub id: NodeId,
}

// The handle owns no JS values, so there is nothing for the GC to trace.
impl<'js> Trace<'js> for DomNode {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

unsafe impl<'js> JsLifetime<'js> for DomNode {
    type Changed<'to> = DomNode;
}

impl DomNode {
    pub fn new(doc: SharedDoc, id: NodeId) -> Self {
        DomNode { doc, id }
    }

    /// The JS object for a node: the one it already has, or a new one that
    /// is remembered from now on.
    pub fn wrap<'js>(ctx: &Ctx<'js>, doc: &SharedDoc, id: NodeId) -> Result<Class<'js, DomNode>> {
        let key = id.as_u32();
        if let Ok(handles) = doc.handles.try_borrow()
            && let Some(kept) = handles.cache.get(&key)
        {
            let object = kept.clone().restore(ctx)?;
            if let Some(class) = Class::<DomNode>::from_object(&object) {
                return Ok(class);
            }
        }
        let node = DomNode::new(doc.clone(), id);
        let class = match doc.prototype_for(ctx, id) {
            Some(proto) => Class::instance_proto(node, proto)?,
            None => Class::instance(ctx.clone(), node)?,
        };
        if let Ok(mut handles) = doc.handles.try_borrow_mut() {
            let object: Object<'js> = class.clone().into_inner();
            handles.cache.insert(key, Persistent::save(ctx, object));
        }
        Ok(class)
    }

    /// Wrap an optional node id, mapping `None` to JS `null`.
    pub fn wrap_opt<'js>(
        ctx: &Ctx<'js>,
        doc: &SharedDoc,
        id: Option<NodeId>,
    ) -> Result<Value<'js>> {
        match id {
            Some(id) => Ok(DomNode::wrap(ctx, doc, id)?.into_value()),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }

    fn wrap_list<'js>(
        ctx: &Ctx<'js>,
        doc: &SharedDoc,
        ids: impl IntoIterator<Item = NodeId>,
    ) -> Result<Value<'js>> {
        let arr = rquickjs::Array::new(ctx.clone())?;
        for (i, id) in ids.into_iter().enumerate() {
            arr.set(i, DomNode::wrap(ctx, doc, id)?)?;
        }
        arr.into_js(ctx)
    }

    fn tag(&self) -> Option<String> {
        self.doc
            .borrow()
            .element(self.id)
            .map(|e| e.local_name().to_string())
    }

    fn attr(&self, name: &str) -> Option<String> {
        self.doc
            .borrow()
            .element(self.id)
            .and_then(|e| e.attr(&LocalName::from(name)).map(str::to_owned))
    }

    /// What inserting `node` actually inserts: the node, or, for a document
    /// fragment, its children. A fragment is a carrier, and a page that
    /// builds one and appends it expects the children in the tree and the
    /// fragment empty afterwards.
    fn to_insert(&self, node: NodeId) -> Vec<NodeId> {
        let doc = self.doc.borrow();
        let is_fragment = matches!(doc.data(node), NodeData::Document) && node != doc.root();
        if is_fragment {
            doc.children(node).collect()
        } else {
            vec![node]
        }
    }

    /// The content fragment of a `<template>`, made on first use for a
    /// template the page built itself; `None` for any other element.
    fn template_contents(&self) -> Option<NodeId> {
        let existing = {
            let doc = self.doc.borrow();
            match doc.element(self.id) {
                Some(e) if e.local_name().as_ref() == "template" => Some(e.template_contents),
                _ => None,
            }
        }?;
        Some(existing.unwrap_or_else(|| {
            let mut doc = self.doc.borrow_mut();
            let id = doc.create(NodeData::Document);
            if let Some(el) = doc.element_mut(self.id) {
                el.template_contents = Some(id);
            }
            id
        }))
    }

    /// Where an element's markup lives: under it, or, for a template, in its
    /// content fragment.
    fn markup_root(&self) -> NodeId {
        self.template_contents().unwrap_or(self.id)
    }

    fn set_attr_raw(&self, name: &str, value: &str) {
        let mut doc = self.doc.borrow_mut();
        if let Some(el) = doc.element_mut(self.id) {
            el.set_attr(
                QualName::new(None, ns!(), LocalName::from(name)),
                StrTendril::from(value),
            );
        }
    }
}

/// JavaScript truthiness, for a flag a page may pass as anything.
fn truthy(value: &Value<'_>) -> bool {
    match value.type_of() {
        rquickjs::Type::Undefined | rquickjs::Type::Null | rquickjs::Type::Uninitialized => false,
        rquickjs::Type::Bool => value.as_bool().unwrap_or(false),
        rquickjs::Type::Int | rquickjs::Type::Float => {
            value.as_number().is_some_and(|n| n != 0.0 && !n.is_nan())
        }
        rquickjs::Type::String => value
            .as_string()
            .and_then(|s| s.to_string().ok())
            .is_some_and(|s| !s.is_empty()),
        _ => true,
    }
}

fn throw_str(ctx: &Ctx<'_>, message: &str) -> Error {
    match rquickjs::String::from_str(ctx.clone(), message) {
        Ok(s) => ctx.throw(s.into_value()),
        Err(e) => e,
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl DomNode {
    // ---- identity ------------------------------------------------------

    // A detached Document node stands in for a fragment, and a script tells
    // the two apart by nodeType: React and every sanitiser branch on 11.
    #[qjs(get, configurable)]
    fn node_type(&self) -> u16 {
        let doc = self.doc.borrow();
        match doc.data(self.id) {
            NodeData::Document if self.id != doc.root() => 11,
            data => data.node_type(),
        }
    }

    #[qjs(get, configurable)]
    fn node_name(&self) -> String {
        let doc = self.doc.borrow();
        match doc.data(self.id) {
            NodeData::Element(e) => e.local_name().as_ref().to_ascii_uppercase(),
            NodeData::Text(_) => "#text".into(),
            NodeData::Comment(_) => "#comment".into(),
            NodeData::Document if self.id != doc.root() => "#document-fragment".into(),
            NodeData::Document => "#document".into(),
            NodeData::Doctype { name, .. } => name.to_string(),
            NodeData::ProcessingInstruction { target, .. } => target.to_string(),
        }
    }

    #[qjs(get, configurable)]
    fn tag_name(&self) -> Option<String> {
        self.tag().map(|t| t.to_ascii_uppercase())
    }

    #[qjs(get, rename = "localName", configurable)]
    fn js_local_name(&self) -> Option<String> {
        self.tag()
    }

    /// Stable arena id. The CDP layer addresses nodes by this.
    #[qjs(get, configurable)]
    fn mar_node_id(&self) -> u32 {
        self.id.as_u32()
    }

    // ---- tree ----------------------------------------------------------

    #[qjs(get, configurable)]
    fn parent_node<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        // The arena root stands in for `document`, and `document` is what the
        // DOM says the parent of <html> is. A walk up from any node therefore
        // ends at the document, which is where a delegated listener sits: a
        // `document.addEventListener('click')` has to see a click that bubbled.
        let parent = self.doc.borrow().node(self.id).parent;
        DomNode::wrap_opt(&ctx, &self.doc, parent)
    }

    #[qjs(get, configurable)]
    fn parent_element<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let doc = self.doc.borrow();
        let parent = doc
            .node(self.id)
            .parent
            .filter(|&p| doc.data(p).is_element());
        drop(doc);
        DomNode::wrap_opt(&ctx, &self.doc, parent)
    }

    #[qjs(get, configurable)]
    fn child_nodes<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let ids: Vec<_> = self.doc.borrow().children(self.id).collect();
        DomNode::wrap_list(&ctx, &self.doc, ids)
    }

    #[qjs(get, configurable)]
    fn children<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let doc = self.doc.borrow();
        let ids: Vec<_> = doc
            .children(self.id)
            .filter(|&c| doc.data(c).is_element())
            .collect();
        drop(doc);
        DomNode::wrap_list(&ctx, &self.doc, ids)
    }

    #[qjs(get, configurable)]
    fn child_element_count(&self) -> usize {
        let doc = self.doc.borrow();
        doc.children(self.id)
            .filter(|&c| doc.data(c).is_element())
            .count()
    }

    #[qjs(get, configurable)]
    fn first_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().node(self.id).first_child;
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get, configurable)]
    fn last_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().node(self.id).last_child;
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get, configurable)]
    fn first_element_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let doc = self.doc.borrow();
        let id = doc.children(self.id).find(|&c| doc.data(c).is_element());
        drop(doc);
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get, configurable)]
    fn last_element_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let doc = self.doc.borrow();
        let id = doc
            .children(self.id)
            .filter(|&c| doc.data(c).is_element())
            .last();
        drop(doc);
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get, configurable)]
    fn next_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().node(self.id).next_sibling;
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get, configurable)]
    fn previous_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().node(self.id).prev_sibling;
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get, configurable)]
    fn next_element_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().next_element_sibling(self.id);
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get, configurable)]
    fn previous_element_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().prev_element_sibling(self.id);
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get, configurable)]
    fn owner_document<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        // `document` is installed as a global; hand back the same object.
        ctx.globals().get::<_, Value>("document")
    }

    // `contains(null)` and `contains(window)` are false, not exceptions: the
    // usual caller is `if (!menu.contains(event.target))` and the target is
    // whatever the event landed on.
    fn contains(&self, other: Value<'_>) -> bool {
        let Some(other) = other.as_object().and_then(Class::<DomNode>::from_object) else {
            return false;
        };
        let other_id = other.borrow().id;
        self.doc.borrow().contains(self.id, other_id)
    }

    fn has_child_nodes(&self) -> bool {
        self.doc.borrow().node(self.id).first_child.is_some()
    }

    // ---- mutation ------------------------------------------------------

    fn append_child<'js>(
        &self,
        ctx: Ctx<'js>,
        child: Class<'js, DomNode>,
    ) -> Result<Class<'js, DomNode>> {
        let child_id = child.borrow().id;
        if self.doc.borrow().contains(child_id, self.id) {
            return Err(throw_str(&ctx, "HierarchyRequestError: cyclic appendChild"));
        }
        for id in self.to_insert(child_id) {
            self.doc.borrow_mut().append(self.id, id);
        }
        Ok(child)
    }

    fn insert_before<'js>(
        &self,
        ctx: Ctx<'js>,
        node: Class<'js, DomNode>,
        reference: rquickjs::function::Opt<Option<Class<'js, DomNode>>>,
    ) -> Result<Class<'js, DomNode>> {
        let reference = reference.0.flatten();
        let node_id = node.borrow().id;
        if self.doc.borrow().contains(node_id, self.id) {
            return Err(throw_str(
                &ctx,
                "HierarchyRequestError: cyclic insertBefore",
            ));
        }
        for id in self.to_insert(node_id) {
            match &reference {
                Some(r) => {
                    let ref_id = r.borrow().id;
                    self.doc.borrow_mut().insert_before(ref_id, id);
                }
                // A null reference means append, per the DOM spec.
                None => self.doc.borrow_mut().append(self.id, id),
            }
        }
        Ok(node)
    }

    fn remove_child<'js>(
        &self,
        ctx: Ctx<'js>,
        child: Class<'js, DomNode>,
    ) -> Result<Class<'js, DomNode>> {
        let child_id = child.borrow().id;
        if self.doc.borrow().node(child_id).parent != Some(self.id) {
            return Err(throw_str(&ctx, "NotFoundError: node is not a child"));
        }
        self.doc.borrow_mut().detach(child_id);
        Ok(child)
    }

    fn replace_child<'js>(
        &self,
        ctx: Ctx<'js>,
        new_child: Class<'js, DomNode>,
        old_child: Class<'js, DomNode>,
    ) -> Result<Class<'js, DomNode>> {
        let new_id = new_child.borrow().id;
        let old_id = old_child.borrow().id;
        if self.doc.borrow().node(old_id).parent != Some(self.id) {
            return Err(throw_str(&ctx, "NotFoundError: node is not a child"));
        }
        for id in self.to_insert(new_id) {
            self.doc.borrow_mut().insert_before(old_id, id);
        }
        self.doc.borrow_mut().detach(old_id);
        Ok(old_child)
    }

    fn remove(&self) {
        self.doc.borrow_mut().detach(self.id);
    }

    // `Opt`, not `Option`: an `Option` parameter still counts towards the
    // arity, and `cloneNode()` with no argument then throws.
    fn clone_node<'js>(
        &self,
        ctx: Ctx<'js>,
        deep: rquickjs::function::Opt<Value<'js>>,
    ) -> Result<Class<'js, DomNode>> {
        let deep = deep.0.is_some_and(|v| truthy(&v));
        let copy = self.doc.borrow_mut().clone_node(self.id, deep);
        DomNode::wrap(&ctx, &self.doc, copy)
    }

    fn append(&self, nodes: rquickjs::function::Rest<Class<'_, DomNode>>) {
        for n in nodes.0 {
            let id = n.borrow().id;
            for id in self.to_insert(id) {
                self.doc.borrow_mut().append(self.id, id);
            }
        }
    }

    fn prepend(&self, nodes: rquickjs::function::Rest<Class<'_, DomNode>>) {
        let first = self.doc.borrow().node(self.id).first_child;
        for n in nodes.0 {
            let id = n.borrow().id;
            for id in self.to_insert(id) {
                let mut doc = self.doc.borrow_mut();
                match first {
                    Some(f) => doc.insert_before(f, id),
                    None => doc.append(self.id, id),
                }
            }
        }
    }

    // ---- text and markup -----------------------------------------------

    #[qjs(get, configurable)]
    fn text_content(&self) -> String {
        self.doc.borrow().text_content(self.id)
    }

    #[qjs(set, rename = "textContent", configurable)]
    fn set_text_content(&self, value: JsString) {
        self.doc.borrow_mut().set_text_content(self.id, value);
    }

    #[qjs(get, configurable)]
    fn inner_text(&self) -> String {
        self.doc.borrow().rendered_text(self.id)
    }

    #[qjs(set, rename = "innerText", configurable)]
    fn set_inner_text(&self, value: JsString) {
        self.doc.borrow_mut().set_text_content(self.id, value);
    }

    #[qjs(skip)]
    fn character_data(&self) -> Option<String> {
        match self.doc.borrow().data(self.id) {
            NodeData::Text(t) => Some(t.to_string()),
            NodeData::Comment(t) => Some(t.to_string()),
            _ => None,
        }
    }

    /// `null`, not `undefined`, where the DOM says null: an element's
    /// `nodeValue`, a missing attribute. Code compares with `=== null`.
    #[qjs(skip)]
    fn nullable<'js>(ctx: &Ctx<'js>, value: Option<String>) -> Result<Value<'js>> {
        match value {
            Some(value) => value.into_js(ctx),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }

    #[qjs(get, configurable)]
    fn node_value<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::nullable(&ctx, self.character_data())
    }

    #[qjs(set, rename = "nodeValue", configurable)]
    fn set_node_value(&self, value: JsString) {
        let mut doc = self.doc.borrow_mut();
        match &mut doc.node_mut(self.id).data {
            NodeData::Text(t) => *t = StrTendril::from(value),
            NodeData::Comment(t) => *t = StrTendril::from(value),
            _ => {}
        }
    }

    #[qjs(get, configurable)]
    fn data<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::nullable(&ctx, self.character_data())
    }

    #[qjs(set, rename = "data", configurable)]
    fn set_data(&self, value: JsString) {
        self.set_node_value(value);
    }

    /// The fragment a `<template>` keeps its content in. The parser puts a
    /// template's children there rather than under the element, and a
    /// template the page built itself gets one the first time it is asked.
    #[qjs(get, configurable)]
    fn template_content<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        DomNode::wrap_opt(&ctx, &self.doc, self.template_contents())
    }

    #[qjs(get, rename = "innerHTML", configurable)]
    fn inner_html(&self) -> String {
        mar_dom::inner_html(&self.doc.borrow(), self.markup_root())
    }

    #[qjs(set, rename = "innerHTML", configurable)]
    fn set_inner_html(&self, html: JsString) {
        // Parse into a scratch document first: a malformed fragment then cannot
        // corrupt the live tree, and the tree sink stays single-document.
        let context = self
            .doc
            .borrow()
            .element(self.id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| QualName::new(None, ns!(html), LocalName::from("div")));
        let (frag, holder) = mar_dom::parse_fragment_document(&html, &context);
        let root = self.markup_root();
        let mut doc = self.doc.borrow_mut();
        while let Some(c) = doc.node(root).first_child {
            doc.detach(c);
        }
        let kids: Vec<_> = frag.children(holder).collect();
        for k in kids {
            let copy = doc.import_subtree(&frag, k);
            doc.append(root, copy);
        }
    }

    #[qjs(get, rename = "outerHTML", configurable)]
    fn outer_html(&self) -> String {
        mar_dom::outer_html(&self.doc.borrow(), self.id)
    }

    #[qjs(set, rename = "outerHTML", configurable)]
    fn set_outer_html(&self, html: JsString) {
        let (context, parent) = {
            let doc = self.doc.borrow();
            let parent = doc.node(self.id).parent;
            let name = parent
                .and_then(|p| doc.element(p))
                .map(|e| e.name.clone())
                .unwrap_or_else(|| QualName::new(None, ns!(html), LocalName::from("div")));
            (name, parent)
        };
        if parent.is_none() {
            return;
        }
        let (frag, holder) = mar_dom::parse_fragment_document(&html, &context);
        let mut doc = self.doc.borrow_mut();
        let kids: Vec<_> = frag.children(holder).collect();
        for k in kids {
            let copy = doc.import_subtree(&frag, k);
            doc.insert_before(self.id, copy);
        }
        doc.detach(self.id);
    }

    // The DOM spells the acronym in capitals, and camelCase renaming does
    // not know that. Without the explicit name the method is published as
    // `insertAdjacentHtml`, which no page has ever called.
    #[qjs(rename = "insertAdjacentHTML")]
    fn insert_adjacent_html(&self, position: JsString, html: JsString) {
        let context = self
            .doc
            .borrow()
            .element(self.id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| QualName::new(None, ns!(html), LocalName::from("div")));
        let (frag, holder) = mar_dom::parse_fragment_document(&html, &context);
        let kids: Vec<_> = frag.children(holder).collect();
        let mut doc = self.doc.borrow_mut();
        match position.to_ascii_lowercase().as_str() {
            "beforebegin" => {
                for k in kids {
                    let copy = doc.import_subtree(&frag, k);
                    doc.insert_before(self.id, copy);
                }
            }
            "afterbegin" => {
                let first = doc.node(self.id).first_child;
                for k in kids {
                    let copy = doc.import_subtree(&frag, k);
                    match first {
                        Some(f) => doc.insert_before(f, copy),
                        None => doc.append(self.id, copy),
                    }
                }
            }
            "afterend" => {
                let next = doc.node(self.id).next_sibling;
                for k in kids {
                    let copy = doc.import_subtree(&frag, k);
                    match next {
                        Some(n) => doc.insert_before(n, copy),
                        None => {
                            if let Some(p) = doc.node(self.id).parent {
                                doc.append(p, copy);
                            }
                        }
                    }
                }
            }
            // "beforeend" and anything unrecognised append.
            _ => {
                for k in kids {
                    let copy = doc.import_subtree(&frag, k);
                    doc.append(self.id, copy);
                }
            }
        }
    }

    // ---- attributes ----------------------------------------------------

    fn get_attribute<'js>(&self, ctx: Ctx<'js>, name: JsString) -> Result<Value<'js>> {
        Self::nullable(&ctx, self.attr(&name.to_ascii_lowercase()))
    }

    fn set_attribute(&self, name: JsString, value: JsString) {
        self.set_attr_raw(&name.to_ascii_lowercase(), &value);
    }

    fn has_attribute(&self, name: JsString) -> bool {
        self.attr(&name.to_ascii_lowercase()).is_some()
    }

    fn remove_attribute(&self, name: JsString) {
        let mut doc = self.doc.borrow_mut();
        if let Some(el) = doc.element_mut(self.id) {
            el.remove_attr(&LocalName::from(name.to_ascii_lowercase()));
        }
    }

    fn toggle_attribute(&self, name: JsString, force: rquickjs::function::Opt<Value<'_>>) -> bool {
        let lower = name.to_ascii_lowercase();
        let present = self.attr(&lower).is_some();
        let target = match force.0 {
            Some(v) if !v.is_undefined() => truthy(&v),
            _ => !present,
        };
        if target {
            self.set_attr_raw(&lower, "");
        } else {
            let mut doc = self.doc.borrow_mut();
            if let Some(el) = doc.element_mut(self.id) {
                el.remove_attr(&LocalName::from(lower));
            }
        }
        target
    }

    fn get_attribute_names(&self) -> Vec<String> {
        self.doc
            .borrow()
            .element(self.id)
            .map(|e| e.attrs.iter().map(|a| a.name.local.to_string()).collect())
            .unwrap_or_default()
    }

    #[qjs(get, configurable)]
    fn id(&self) -> String {
        self.attr("id").unwrap_or_default()
    }

    #[qjs(set, rename = "id", configurable)]
    fn set_id(&self, value: JsString) {
        self.set_attr_raw("id", &value);
    }

    #[qjs(get, configurable)]
    fn class_name(&self) -> String {
        self.attr("class").unwrap_or_default()
    }

    #[qjs(set, rename = "className", configurable)]
    fn set_class_name(&self, value: JsString) {
        self.set_attr_raw("class", &value);
    }

    // ---- queries -------------------------------------------------------

    fn query_selector<'js>(&self, ctx: Ctx<'js>, selector: JsString) -> Result<Value<'js>> {
        let m = Matcher::new(&selector).map_err(|e| throw_str(&ctx, &e.to_string()))?;
        let found = m.query_first(&self.doc.borrow(), self.id);
        DomNode::wrap_opt(&ctx, &self.doc, found)
    }

    fn query_selector_all<'js>(&self, ctx: Ctx<'js>, selector: JsString) -> Result<Value<'js>> {
        let m = Matcher::new(&selector).map_err(|e| throw_str(&ctx, &e.to_string()))?;
        let found = m.query_all(&self.doc.borrow(), self.id);
        DomNode::wrap_list(&ctx, &self.doc, found)
    }

    fn matches(&self, ctx: Ctx<'_>, selector: JsString) -> Result<bool> {
        let m = Matcher::new(&selector).map_err(|e| throw_str(&ctx, &e.to_string()))?;
        let doc = self.doc.borrow();
        Ok(mar_dom::ElementRef::new(&doc, self.id).is_some_and(|e| m.matches(e)))
    }

    fn closest<'js>(&self, ctx: Ctx<'js>, selector: JsString) -> Result<Value<'js>> {
        let m = Matcher::new(&selector).map_err(|e| throw_str(&ctx, &e.to_string()))?;
        let found = m.closest(&self.doc.borrow(), self.id);
        DomNode::wrap_opt(&ctx, &self.doc, found)
    }

    fn get_elements_by_tag_name<'js>(&self, ctx: Ctx<'js>, name: JsString) -> Result<Value<'js>> {
        let doc = self.doc.borrow();
        let wanted = name.to_ascii_lowercase();
        let ids: Vec<_> = doc
            .descendants(self.id)
            .filter(|&c| {
                doc.element(c)
                    .is_some_and(|e| wanted == "*" || e.local_name().as_ref() == wanted)
            })
            .collect();
        drop(doc);
        DomNode::wrap_list(&ctx, &self.doc, ids)
    }

    fn get_elements_by_class_name<'js>(
        &self,
        ctx: Ctx<'js>,
        names: JsString,
    ) -> Result<Value<'js>> {
        let wanted: Vec<&str> = names.split_ascii_whitespace().collect();
        let doc = self.doc.borrow();
        let ids: Vec<_> = doc
            .descendants(self.id)
            .filter(|&c| {
                doc.element(c).is_some_and(|e| {
                    let classes = e.attr(&LocalName::from("class")).unwrap_or("");
                    wanted
                        .iter()
                        .all(|w| classes.split_ascii_whitespace().any(|c| c == *w))
                })
            })
            .collect();
        drop(doc);
        DomNode::wrap_list(&ctx, &self.doc, ids)
    }
}
