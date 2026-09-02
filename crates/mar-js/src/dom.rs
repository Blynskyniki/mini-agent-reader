//! JavaScript bindings for the arena DOM.
//!
//! The document lives in Rust behind an `Rc<RefCell<_>>`; every JS object is a
//! thin handle holding that pointer plus a `NodeId`. Nothing about the tree is
//! duplicated into the JS heap, so a large page costs the same whether or not
//! scripts walk it.

use mar_dom::{Document, LocalName, Matcher, NodeData, NodeId, QualName, StrTendril, ns};
use rquickjs::class::{Trace, Tracer};
use rquickjs::{Class, Ctx, Error, IntoJs, JsLifetime, Result, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// The page document, shared between Rust and every JS handle.
#[derive(Clone)]
pub struct SharedDoc(Rc<RefCell<Document>>);

impl SharedDoc {
    pub fn new(doc: Document) -> Self {
        SharedDoc(Rc::new(RefCell::new(doc)))
    }

    #[inline]
    pub fn borrow(&self) -> std::cell::Ref<'_, Document> {
        self.0.borrow()
    }

    #[inline]
    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, Document> {
        self.0.borrow_mut()
    }

    /// Take the document back out once scripting is done.
    pub fn into_inner(self) -> std::result::Result<Document, Self> {
        Rc::try_unwrap(self.0)
            .map(RefCell::into_inner)
            .map_err(SharedDoc)
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

    /// Wrap a node id as a JS object.
    pub fn wrap<'js>(ctx: &Ctx<'js>, doc: &SharedDoc, id: NodeId) -> Result<Class<'js, DomNode>> {
        Class::instance(ctx.clone(), DomNode::new(doc.clone(), id))
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

fn throw_str(ctx: &Ctx<'_>, message: &str) -> Error {
    match rquickjs::String::from_str(ctx.clone(), message) {
        Ok(s) => ctx.throw(s.into_value()),
        Err(e) => e,
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl DomNode {
    // ---- identity ------------------------------------------------------

    #[qjs(get)]
    fn node_type(&self) -> u16 {
        self.doc.borrow().data(self.id).node_type()
    }

    #[qjs(get)]
    fn node_name(&self) -> String {
        let doc = self.doc.borrow();
        match doc.data(self.id) {
            NodeData::Element(e) => e.local_name().as_ref().to_ascii_uppercase(),
            NodeData::Text(_) => "#text".into(),
            NodeData::Comment(_) => "#comment".into(),
            NodeData::Document => "#document".into(),
            NodeData::Doctype { name, .. } => name.to_string(),
            NodeData::ProcessingInstruction { target, .. } => target.to_string(),
        }
    }

    #[qjs(get)]
    fn tag_name(&self) -> Option<String> {
        self.tag().map(|t| t.to_ascii_uppercase())
    }

    #[qjs(get, rename = "localName")]
    fn js_local_name(&self) -> Option<String> {
        self.tag()
    }

    /// Stable arena id. The CDP layer addresses nodes by this.
    #[qjs(get)]
    fn mar_node_id(&self) -> u32 {
        self.id.as_u32()
    }

    // ---- tree ----------------------------------------------------------

    #[qjs(get)]
    fn parent_node<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let doc = self.doc.borrow();
        // The arena root stands in for `document`; it is not an element parent.
        let parent = doc.node(self.id).parent.filter(|&p| p != doc.root());
        drop(doc);
        DomNode::wrap_opt(&ctx, &self.doc, parent)
    }

    #[qjs(get)]
    fn parent_element<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let doc = self.doc.borrow();
        let parent = doc
            .node(self.id)
            .parent
            .filter(|&p| doc.data(p).is_element());
        drop(doc);
        DomNode::wrap_opt(&ctx, &self.doc, parent)
    }

    #[qjs(get)]
    fn child_nodes<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let ids: Vec<_> = self.doc.borrow().children(self.id).collect();
        DomNode::wrap_list(&ctx, &self.doc, ids)
    }

    #[qjs(get)]
    fn children<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let doc = self.doc.borrow();
        let ids: Vec<_> = doc
            .children(self.id)
            .filter(|&c| doc.data(c).is_element())
            .collect();
        drop(doc);
        DomNode::wrap_list(&ctx, &self.doc, ids)
    }

    #[qjs(get)]
    fn child_element_count(&self) -> usize {
        let doc = self.doc.borrow();
        doc.children(self.id)
            .filter(|&c| doc.data(c).is_element())
            .count()
    }

    #[qjs(get)]
    fn first_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().node(self.id).first_child;
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get)]
    fn last_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().node(self.id).last_child;
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get)]
    fn first_element_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let doc = self.doc.borrow();
        let id = doc.children(self.id).find(|&c| doc.data(c).is_element());
        drop(doc);
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get)]
    fn last_element_child<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let doc = self.doc.borrow();
        let id = doc
            .children(self.id)
            .filter(|&c| doc.data(c).is_element())
            .last();
        drop(doc);
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get)]
    fn next_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().node(self.id).next_sibling;
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get)]
    fn previous_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().node(self.id).prev_sibling;
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get)]
    fn next_element_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().next_element_sibling(self.id);
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get)]
    fn previous_element_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let id = self.doc.borrow().prev_element_sibling(self.id);
        DomNode::wrap_opt(&ctx, &self.doc, id)
    }

    #[qjs(get)]
    fn owner_document<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        // `document` is installed as a global; hand back the same object.
        ctx.globals().get::<_, Value>("document")
    }

    fn contains(&self, other: Class<'_, DomNode>) -> bool {
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
        self.doc.borrow_mut().append(self.id, child_id);
        Ok(child)
    }

    fn insert_before<'js>(
        &self,
        ctx: Ctx<'js>,
        node: Class<'js, DomNode>,
        reference: Option<Class<'js, DomNode>>,
    ) -> Result<Class<'js, DomNode>> {
        let node_id = node.borrow().id;
        if self.doc.borrow().contains(node_id, self.id) {
            return Err(throw_str(
                &ctx,
                "HierarchyRequestError: cyclic insertBefore",
            ));
        }
        match reference {
            Some(r) => {
                let ref_id = r.borrow().id;
                self.doc.borrow_mut().insert_before(ref_id, node_id);
            }
            // A null reference means append, per the DOM spec.
            None => self.doc.borrow_mut().append(self.id, node_id),
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
        {
            let mut doc = self.doc.borrow_mut();
            if doc.node(old_id).parent != Some(self.id) {
                drop(doc);
                return Err(throw_str(&ctx, "NotFoundError: node is not a child"));
            }
            doc.insert_before(old_id, new_id);
            doc.detach(old_id);
        }
        Ok(old_child)
    }

    fn remove(&self) {
        self.doc.borrow_mut().detach(self.id);
    }

    fn clone_node<'js>(&self, ctx: Ctx<'js>, deep: Option<bool>) -> Result<Class<'js, DomNode>> {
        let copy = self
            .doc
            .borrow_mut()
            .clone_node(self.id, deep.unwrap_or(false));
        DomNode::wrap(&ctx, &self.doc, copy)
    }

    fn append(&self, nodes: rquickjs::function::Rest<Class<'_, DomNode>>) {
        let mut doc = self.doc.borrow_mut();
        for n in nodes.0 {
            let id = n.borrow().id;
            doc.append(self.id, id);
        }
    }

    fn prepend(&self, nodes: rquickjs::function::Rest<Class<'_, DomNode>>) {
        let mut doc = self.doc.borrow_mut();
        let first = doc.node(self.id).first_child;
        for n in nodes.0 {
            let id = n.borrow().id;
            match first {
                Some(f) => doc.insert_before(f, id),
                None => doc.append(self.id, id),
            }
        }
    }

    // ---- text and markup -----------------------------------------------

    #[qjs(get)]
    fn text_content(&self) -> String {
        self.doc.borrow().text_content(self.id)
    }

    #[qjs(set, rename = "textContent")]
    fn set_text_content(&self, value: String) {
        self.doc.borrow_mut().set_text_content(self.id, value);
    }

    #[qjs(get)]
    fn inner_text(&self) -> String {
        self.doc.borrow().text_content(self.id)
    }

    #[qjs(set, rename = "innerText")]
    fn set_inner_text(&self, value: String) {
        self.doc.borrow_mut().set_text_content(self.id, value);
    }

    #[qjs(get)]
    fn node_value(&self) -> Option<String> {
        match self.doc.borrow().data(self.id) {
            NodeData::Text(t) => Some(t.to_string()),
            NodeData::Comment(t) => Some(t.to_string()),
            _ => None,
        }
    }

    #[qjs(set, rename = "nodeValue")]
    fn set_node_value(&self, value: String) {
        let mut doc = self.doc.borrow_mut();
        match &mut doc.node_mut(self.id).data {
            NodeData::Text(t) => *t = StrTendril::from(value),
            NodeData::Comment(t) => *t = StrTendril::from(value),
            _ => {}
        }
    }

    #[qjs(get)]
    fn data(&self) -> Option<String> {
        self.node_value()
    }

    #[qjs(set, rename = "data")]
    fn set_data(&self, value: String) {
        self.set_node_value(value);
    }

    #[qjs(get, rename = "innerHTML")]
    fn inner_html(&self) -> String {
        mar_dom::inner_html(&self.doc.borrow(), self.id)
    }

    #[qjs(set, rename = "innerHTML")]
    fn set_inner_html(&self, html: String) {
        // Parse into a scratch document first: a malformed fragment then cannot
        // corrupt the live tree, and the tree sink stays single-document.
        let context = self
            .doc
            .borrow()
            .element(self.id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| QualName::new(None, ns!(html), LocalName::from("div")));
        let (frag, holder) = mar_dom::parse_fragment_document(&html, &context);
        let mut doc = self.doc.borrow_mut();
        while let Some(c) = doc.node(self.id).first_child {
            doc.detach(c);
        }
        let kids: Vec<_> = frag.children(holder).collect();
        for k in kids {
            let copy = doc.import_subtree(&frag, k);
            doc.append(self.id, copy);
        }
    }

    #[qjs(get, rename = "outerHTML")]
    fn outer_html(&self) -> String {
        mar_dom::outer_html(&self.doc.borrow(), self.id)
    }

    #[qjs(set, rename = "outerHTML")]
    fn set_outer_html(&self, html: String) {
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

    fn insert_adjacent_html(&self, position: String, html: String) {
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

    fn get_attribute(&self, name: String) -> Option<String> {
        self.attr(&name.to_ascii_lowercase())
    }

    fn set_attribute(&self, name: String, value: String) {
        self.set_attr_raw(&name.to_ascii_lowercase(), &value);
    }

    fn has_attribute(&self, name: String) -> bool {
        self.attr(&name.to_ascii_lowercase()).is_some()
    }

    fn remove_attribute(&self, name: String) {
        let mut doc = self.doc.borrow_mut();
        if let Some(el) = doc.element_mut(self.id) {
            el.remove_attr(&LocalName::from(name.to_ascii_lowercase()));
        }
    }

    fn toggle_attribute(&self, name: String, force: Option<bool>) -> bool {
        let lower = name.to_ascii_lowercase();
        let present = self.attr(&lower).is_some();
        let target = force.unwrap_or(!present);
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

    #[qjs(get)]
    fn id(&self) -> String {
        self.attr("id").unwrap_or_default()
    }

    #[qjs(set, rename = "id")]
    fn set_id(&self, value: String) {
        self.set_attr_raw("id", &value);
    }

    #[qjs(get)]
    fn class_name(&self) -> String {
        self.attr("class").unwrap_or_default()
    }

    #[qjs(set, rename = "className")]
    fn set_class_name(&self, value: String) {
        self.set_attr_raw("class", &value);
    }

    // ---- queries -------------------------------------------------------

    fn query_selector<'js>(&self, ctx: Ctx<'js>, selector: String) -> Result<Value<'js>> {
        let m = Matcher::new(&selector).map_err(|e| throw_str(&ctx, &e.to_string()))?;
        let found = m.query_first(&self.doc.borrow(), self.id);
        DomNode::wrap_opt(&ctx, &self.doc, found)
    }

    fn query_selector_all<'js>(&self, ctx: Ctx<'js>, selector: String) -> Result<Value<'js>> {
        let m = Matcher::new(&selector).map_err(|e| throw_str(&ctx, &e.to_string()))?;
        let found = m.query_all(&self.doc.borrow(), self.id);
        DomNode::wrap_list(&ctx, &self.doc, found)
    }

    fn matches(&self, ctx: Ctx<'_>, selector: String) -> Result<bool> {
        let m = Matcher::new(&selector).map_err(|e| throw_str(&ctx, &e.to_string()))?;
        let doc = self.doc.borrow();
        Ok(mar_dom::ElementRef::new(&doc, self.id).is_some_and(|e| m.matches(e)))
    }

    fn closest<'js>(&self, ctx: Ctx<'js>, selector: String) -> Result<Value<'js>> {
        let m = Matcher::new(&selector).map_err(|e| throw_str(&ctx, &e.to_string()))?;
        let found = m.closest(&self.doc.borrow(), self.id);
        DomNode::wrap_opt(&ctx, &self.doc, found)
    }

    fn get_elements_by_tag_name<'js>(&self, ctx: Ctx<'js>, name: String) -> Result<Value<'js>> {
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

    fn get_elements_by_class_name<'js>(&self, ctx: Ctx<'js>, names: String) -> Result<Value<'js>> {
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
