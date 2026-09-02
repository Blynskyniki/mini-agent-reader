//! html5ever tree sink that builds an arena [`Document`].

use crate::arena::{Document, NodeData, NodeId};
use html5ever::interface::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, LocalName, Namespace, ParseOpts, QualName, parse_document};
use markup5ever::interface::ElemName;
use std::borrow::Cow;
use std::cell::RefCell;

/// Owned element name handed back from [`TreeSink::elem_name`].
///
/// The sink keeps the document behind a `RefCell`, so it cannot hand out a
/// borrow into it. `QualName` is two atoms, and cloning an atom is a refcount
/// bump or a memcpy of an inline string, so this is cheap.
#[derive(Debug, Clone)]
pub struct OwnedElemName(QualName);

impl ElemName for OwnedElemName {
    #[inline]
    fn ns(&self) -> &Namespace {
        &self.0.ns
    }

    #[inline]
    fn local_name(&self) -> &LocalName {
        &self.0.local
    }
}

/// Tree sink over an arena document. All `TreeSink` methods take `&self`, so
/// the document sits behind a `RefCell`; the borrows are short and never nest.
pub struct ArenaSink {
    doc: RefCell<Document>,
    errors: RefCell<Vec<Cow<'static, str>>>,
}

impl ArenaSink {
    pub fn new() -> Self {
        ArenaSink {
            doc: RefCell::new(Document::new()),
            errors: RefCell::new(Vec::new()),
        }
    }

    /// Append text to `parent`, merging into a trailing text node when there is
    /// one. Without this a document ends up with a text node per token.
    fn append_text(&self, parent: NodeId, text: StrTendril) {
        let mut doc = self.doc.borrow_mut();
        if let Some(last) = doc.node(parent).last_child
            && let NodeData::Text(existing) = &mut doc.node_mut(last).data
        {
            existing.push_tendril(&text);
            return;
        }
        let id = doc.create_text(text);
        doc.append(parent, id);
    }

    fn insert_text_before(&self, sibling: NodeId, text: StrTendril) {
        let mut doc = self.doc.borrow_mut();
        if let Some(prev) = doc.node(sibling).prev_sibling
            && let NodeData::Text(existing) = &mut doc.node_mut(prev).data
        {
            existing.push_tendril(&text);
            return;
        }
        let id = doc.create_text(text);
        doc.insert_before(sibling, id);
    }
}

impl Default for ArenaSink {
    fn default() -> Self {
        ArenaSink::new()
    }
}

/// Result of parsing: the document plus any parse errors html5ever reported.
pub struct ParsedDocument {
    pub document: Document,
    pub errors: Vec<Cow<'static, str>>,
}

impl TreeSink for ArenaSink {
    type Handle = NodeId;
    type Output = ParsedDocument;
    type ElemName<'a>
        = OwnedElemName
    where
        Self: 'a;

    fn finish(self) -> ParsedDocument {
        ParsedDocument {
            document: self.doc.into_inner(),
            errors: self.errors.into_inner(),
        }
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        // Errors are informational; a real page hits plenty of them. Cap the
        // buffer so a pathological document cannot grow it without bound.
        let mut errors = self.errors.borrow_mut();
        if errors.len() < 256 {
            errors.push(msg);
        }
    }

    fn get_document(&self) -> NodeId {
        self.doc.borrow().root()
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> OwnedElemName {
        let doc = self.doc.borrow();
        let name = doc
            .element(*target)
            .expect("elem_name called on a non-element")
            .name
            .clone();
        OwnedElemName(name)
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> NodeId {
        let mut doc = self.doc.borrow_mut();
        let id = doc.create_element(name, attrs);
        if flags.template {
            let contents = doc.create(NodeData::Document);
            if let Some(el) = doc.element_mut(id) {
                el.template_contents = Some(contents);
            }
        }
        if flags.mathml_annotation_xml_integration_point
            && let Some(el) = doc.element_mut(id)
        {
            el.mathml_annotation_xml_integration_point = true;
        }
        id
    }

    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.doc.borrow_mut().create(NodeData::Comment(text))
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        self.doc
            .borrow_mut()
            .create(NodeData::ProcessingInstruction { target, data })
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        match child {
            NodeOrText::AppendNode(id) => self.doc.borrow_mut().append(*parent, id),
            NodeOrText::AppendText(text) => self.append_text(*parent, text),
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        let has_parent = self.doc.borrow().node(*element).parent.is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        let mut doc = self.doc.borrow_mut();
        let root = doc.root();
        let id = doc.create(NodeData::Doctype {
            name,
            public_id,
            system_id,
        });
        doc.append(root, id);
    }

    fn mark_script_already_started(&self, node: &NodeId) {
        if let Some(el) = self.doc.borrow_mut().element_mut(*node) {
            el.script_already_started = true;
        }
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        self.doc
            .borrow()
            .element(*target)
            .and_then(|e| e.template_contents)
            .expect("get_template_contents on a non-template element")
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.doc.borrow_mut().quirks_mode = mode;
    }

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        match new_node {
            NodeOrText::AppendNode(id) => self.doc.borrow_mut().insert_before(*sibling, id),
            NodeOrText::AppendText(text) => self.insert_text_before(*sibling, text),
        }
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        let mut doc = self.doc.borrow_mut();
        let Some(el) = doc.element_mut(*target) else {
            return;
        };
        for attr in attrs {
            if !el.attrs.iter().any(|a| a.name == attr.name) {
                el.attrs.push(attr);
            }
        }
    }

    fn remove_from_parent(&self, target: &NodeId) {
        self.doc.borrow_mut().detach(*target);
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        self.doc.borrow_mut().reparent_children(*node, *new_parent);
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &NodeId) -> bool {
        self.doc
            .borrow()
            .element(*handle)
            .is_some_and(|e| e.mathml_annotation_xml_integration_point)
    }
}

/// Parse a full HTML document from a UTF-8 string.
pub fn parse_html(html: &str) -> ParsedDocument {
    parse_document(ArenaSink::new(), ParseOpts::default()).one(html)
}

/// Parse a full HTML document from raw bytes, decoding as UTF-8 with lossy
/// replacement. Use [`crate::encoding::decode_html`] first when the response
/// declares a non-UTF-8 charset.
pub fn parse_html_bytes(bytes: &[u8]) -> ParsedDocument {
    parse_document(ArenaSink::new(), ParseOpts::default())
        .from_utf8()
        .one(bytes)
}

/// Parse `html` as the contents of a `context` element (`innerHTML` semantics).
///
/// The result is a scratch document whose returned node holds the parsed
/// children; callers move them across with [`Document::import_subtree`].
/// Parsing into a fresh document rather than the live one keeps the tree sink
/// simple and means a malformed fragment cannot corrupt the page.
pub fn parse_fragment_document(html: &str, context: &QualName) -> (Document, NodeId) {
    use html5ever::parse_fragment;
    let parsed = parse_fragment(
        ArenaSink::new(),
        ParseOpts::default(),
        context.clone(),
        Vec::new(),
        false,
    )
    .one(html);
    // parse_fragment wraps output in an <html> element under the root.
    let doc = parsed.document;
    let holder = doc
        .document_element()
        .unwrap_or_else(|| doc.root());
    (doc, holder)
}
