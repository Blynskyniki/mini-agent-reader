//! Arena-backed DOM tree.
//!
//! Nodes live in a flat `Vec` and are addressed by a 32-bit `NodeId`. There is
//! no `Rc`/`RefCell` per node, so a document costs roughly `size_of::<Node>()`
//! per node plus its children vector, and dropping a document is a single
//! deallocation walk instead of a reference-counting cascade.

use html5ever::{Attribute, LocalName, QualName};
use markup5ever::interface::QuirksMode;
use std::num::NonZeroU32;
use tendril::StrTendril;

/// Index of a node inside a [`Document`] arena.
///
/// Stored as a `NonZeroU32` so `Option<NodeId>` is also 4 bytes. Slot 0 of the
/// arena is a permanently unused sentinel to make that niche valid.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(NonZeroU32);

impl NodeId {
    #[inline]
    fn new(index: usize) -> Self {
        debug_assert!(index > 0 && index < u32::MAX as usize);
        NodeId(NonZeroU32::new(index as u32).expect("node index 0 is the sentinel"))
    }

    #[inline]
    pub fn index(self) -> usize {
        self.0.get() as usize
    }

    /// Stable identifier for CDP / JS handles.
    #[inline]
    pub fn as_u32(self) -> u32 {
        self.0.get()
    }

    #[inline]
    pub fn from_u32(raw: u32) -> Option<Self> {
        NonZeroU32::new(raw).map(NodeId)
    }
}

impl std::fmt::Debug for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0.get())
    }
}

#[derive(Debug, Clone)]
pub struct ElementData {
    pub name: QualName,
    pub attrs: Vec<Attribute>,
    /// `<template>` content lives in a detached fragment.
    pub template_contents: Option<NodeId>,
    pub mathml_annotation_xml_integration_point: bool,
    /// Set once a `<script>` has been handed to the JS engine, so a re-run of
    /// the settle loop never executes the same script twice.
    pub script_already_started: bool,
}

impl ElementData {
    #[inline]
    pub fn attr(&self, name: &LocalName) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.name.local == *name && a.name.ns.is_empty())
            .map(|a| &*a.value)
    }

    #[inline]
    pub fn set_attr(&mut self, name: QualName, value: StrTendril) {
        match self.attrs.iter_mut().find(|a| a.name == name) {
            Some(existing) => existing.value = value,
            None => self.attrs.push(Attribute { name, value }),
        }
    }

    #[inline]
    pub fn remove_attr(&mut self, name: &LocalName) {
        self.attrs
            .retain(|a| !(a.name.local == *name && a.name.ns.is_empty()));
    }

    #[inline]
    pub fn has_attr(&self, name: &LocalName) -> bool {
        self.attr(name).is_some()
    }

    #[inline]
    pub fn local_name(&self) -> &LocalName {
        &self.name.local
    }
}

#[derive(Debug, Clone)]
pub enum NodeData {
    Document,
    Doctype {
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    },
    Text(StrTendril),
    Comment(StrTendril),
    Element(ElementData),
    ProcessingInstruction {
        target: StrTendril,
        data: StrTendril,
    },
}

impl NodeData {
    #[inline]
    pub fn as_element(&self) -> Option<&ElementData> {
        match self {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }

    #[inline]
    pub fn as_element_mut(&mut self) -> Option<&mut ElementData> {
        match self {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }

    #[inline]
    pub fn is_element(&self) -> bool {
        matches!(self, NodeData::Element(_))
    }

    /// DOM `Node.nodeType`.
    pub fn node_type(&self) -> u16 {
        match self {
            NodeData::Element(_) => 1,
            NodeData::Text(_) => 3,
            NodeData::ProcessingInstruction { .. } => 7,
            NodeData::Comment(_) => 8,
            NodeData::Document => 9,
            NodeData::Doctype { .. } => 10,
        }
    }
}

/// A node plus its links. Sibling links are kept explicit so insert/remove is
/// O(1) and does not shift a children vector.
#[derive(Debug, Clone)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub first_child: Option<NodeId>,
    pub last_child: Option<NodeId>,
    pub prev_sibling: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
    pub data: NodeData,
}

impl Node {
    fn new(data: NodeData) -> Self {
        Node {
            parent: None,
            first_child: None,
            last_child: None,
            prev_sibling: None,
            next_sibling: None,
            data,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Document {
    nodes: Vec<Node>,
    root: NodeId,
    pub quirks_mode: QuirksMode,
}

impl Document {
    pub fn new() -> Self {
        // Slot 0 is the sentinel that makes the NonZeroU32 niche sound.
        let sentinel = Node::new(NodeData::Document);
        let root = Node::new(NodeData::Document);
        Document {
            nodes: vec![sentinel, root],
            root: NodeId::new(1),
            quirks_mode: QuirksMode::NoQuirks,
        }
    }

    #[inline]
    pub fn root(&self) -> NodeId {
        self.root
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.nodes.len() - 1
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    #[inline]
    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }

    #[inline]
    pub fn data(&self, id: NodeId) -> &NodeData {
        &self.nodes[id.index()].data
    }

    #[inline]
    pub fn element(&self, id: NodeId) -> Option<&ElementData> {
        self.nodes[id.index()].data.as_element()
    }

    #[inline]
    pub fn element_mut(&mut self, id: NodeId) -> Option<&mut ElementData> {
        self.nodes[id.index()].data.as_element_mut()
    }

    pub fn create(&mut self, data: NodeData) -> NodeId {
        let id = NodeId::new(self.nodes.len());
        self.nodes.push(Node::new(data));
        id
    }

    pub fn create_element(&mut self, name: QualName, attrs: Vec<Attribute>) -> NodeId {
        self.create(NodeData::Element(ElementData {
            name,
            attrs,
            template_contents: None,
            mathml_annotation_xml_integration_point: false,
            script_already_started: false,
        }))
    }

    pub fn create_text(&mut self, text: impl Into<StrTendril>) -> NodeId {
        self.create(NodeData::Text(text.into()))
    }

    // ---- tree mutation -------------------------------------------------

    /// Would putting `child` under `parent` make a node contain itself?
    ///
    /// The general answer is "is `child` an inclusive ancestor of `parent`",
    /// which costs a walk to the root. A node with no children cannot be an
    /// ancestor of anything, and that is every node the parser appends, so the
    /// walk is skipped for the common case.
    fn would_cycle(&self, parent: NodeId, child: NodeId) -> bool {
        if child == parent {
            return true;
        }
        self.node(child).first_child.is_some() && self.contains(child, parent)
    }

    /// Detach `id` from its current parent. Safe to call on a detached node.
    pub fn detach(&mut self, id: NodeId) {
        let (parent, prev, next) = {
            let n = self.node(id);
            (n.parent, n.prev_sibling, n.next_sibling)
        };
        let Some(parent) = parent else { return };

        match prev {
            Some(p) => self.node_mut(p).next_sibling = next,
            None => self.node_mut(parent).first_child = next,
        }
        match next {
            Some(n) => self.node_mut(n).prev_sibling = prev,
            None => self.node_mut(parent).last_child = prev,
        }

        let n = self.node_mut(id);
        n.parent = None;
        n.prev_sibling = None;
        n.next_sibling = None;
    }

    /// Append `child` as the last child of `parent`, detaching it first.
    ///
    /// Refuses to make a node contain itself. The DOM throws for this and the
    /// JS layer does too, but the invariant belongs here: a tree spliced into a
    /// ring makes every later walk — `descendants`, `ancestors`, a selector
    /// query, serialization — run forever, in native code that no budget or
    /// interrupt can stop.
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        if self.would_cycle(parent, child) {
            return;
        }
        self.detach(child);
        let last = self.node(parent).last_child;
        match last {
            Some(l) => {
                self.node_mut(l).next_sibling = Some(child);
                self.node_mut(child).prev_sibling = Some(l);
            }
            None => self.node_mut(parent).first_child = Some(child),
        }
        self.node_mut(parent).last_child = Some(child);
        self.node_mut(child).parent = Some(parent);
    }

    /// Insert `new_node` immediately before `sibling`.
    ///
    /// See [`Document::append`] for why the cycle checks are here.
    pub fn insert_before(&mut self, sibling: NodeId, new_node: NodeId) {
        let Some(parent) = self.node(sibling).parent else {
            return;
        };
        if self.would_cycle(parent, new_node) {
            return;
        }
        // Moving a node to just before itself is a no-op, but only if the
        // reference is taken before the node is removed. The DOM spec does
        // this by advancing the reference to the next sibling first; without
        // it, the node ends up as its own next sibling and the child list is
        // a ring.
        let reference = if new_node == sibling {
            self.node(sibling).next_sibling
        } else {
            Some(sibling)
        };
        self.detach(new_node);
        let Some(sibling) = reference else {
            self.append(parent, new_node);
            return;
        };
        let prev = self.node(sibling).prev_sibling;
        match prev {
            Some(p) => self.node_mut(p).next_sibling = Some(new_node),
            None => self.node_mut(parent).first_child = Some(new_node),
        }
        let n = self.node_mut(new_node);
        n.prev_sibling = prev;
        n.next_sibling = Some(sibling);
        n.parent = Some(parent);
        self.node_mut(sibling).prev_sibling = Some(new_node);
    }

    /// Move every child of `from` to the end of `to`.
    pub fn reparent_children(&mut self, from: NodeId, to: NodeId) {
        while let Some(child) = self.node(from).first_child {
            self.append(to, child);
        }
    }

    // ---- traversal -----------------------------------------------------

    #[inline]
    pub fn children(&self, id: NodeId) -> ChildIter<'_> {
        ChildIter {
            doc: self,
            next: self.node(id).first_child,
        }
    }

    /// Depth-first pre-order walk of `id` and its descendants.
    #[inline]
    pub fn descendants(&self, id: NodeId) -> Descendants<'_> {
        Descendants {
            doc: self,
            root: id,
            next: self.node(id).first_child,
        }
    }

    #[inline]
    pub fn ancestors(&self, id: NodeId) -> Ancestors<'_> {
        Ancestors {
            doc: self,
            next: self.node(id).parent,
        }
    }

    /// Previous sibling that is an element.
    pub fn prev_element_sibling(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = self.node(id).prev_sibling;
        while let Some(c) = cur {
            if self.data(c).is_element() {
                return Some(c);
            }
            cur = self.node(c).prev_sibling;
        }
        None
    }

    /// Next sibling that is an element.
    pub fn next_element_sibling(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = self.node(id).next_sibling;
        while let Some(c) = cur {
            if self.data(c).is_element() {
                return Some(c);
            }
            cur = self.node(c).next_sibling;
        }
        None
    }

    /// Nearest ancestor (or self) that is an element.
    pub fn closest_element(&self, id: NodeId) -> Option<NodeId> {
        if self.data(id).is_element() {
            return Some(id);
        }
        self.ancestors(id).find(|&a| self.data(a).is_element())
    }

    /// The `<html>` element, if the tree has one.
    pub fn document_element(&self) -> Option<NodeId> {
        self.children(self.root)
            .find(|&c| self.data(c).is_element())
    }

    /// The `<body>` element, if the tree has one.
    pub fn body(&self) -> Option<NodeId> {
        let html = self.document_element()?;
        self.children(html).find(|&c| {
            self.element(c)
                .is_some_and(|e| e.local_name().as_ref() == "body")
        })
    }

    /// The `<head>` element, if the tree has one.
    pub fn head(&self) -> Option<NodeId> {
        let html = self.document_element()?;
        self.children(html).find(|&c| {
            self.element(c)
                .is_some_and(|e| e.local_name().as_ref() == "head")
        })
    }

    /// Concatenated text of `id` and its descendants (`Node.textContent`).
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        if let NodeData::Text(t) = self.data(id) {
            out.push_str(t);
        }
        for d in self.descendants(id) {
            if let NodeData::Text(t) = self.data(d) {
                out.push_str(t);
            }
        }
        out
    }

    /// Replace all children of `id` with a single text node.
    pub fn set_text_content(&mut self, id: NodeId, text: impl Into<StrTendril>) {
        while let Some(child) = self.node(id).first_child {
            self.detach(child);
        }
        let t = self.create_text(text);
        self.append(id, t);
    }
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

pub struct ChildIter<'a> {
    doc: &'a Document,
    next: Option<NodeId>,
}

impl Iterator for ChildIter<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let cur = self.next?;
        self.next = self.doc.node(cur).next_sibling;
        Some(cur)
    }
}

pub struct Ancestors<'a> {
    doc: &'a Document,
    next: Option<NodeId>,
}

impl Iterator for Ancestors<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let cur = self.next?;
        self.next = self.doc.node(cur).parent;
        Some(cur)
    }
}

/// Pre-order descendant walk that never allocates a stack: it climbs back up
/// through parent links when a subtree is exhausted.
pub struct Descendants<'a> {
    doc: &'a Document,
    root: NodeId,
    next: Option<NodeId>,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let cur = self.next?;
        let node = self.doc.node(cur);
        self.next = if let Some(fc) = node.first_child {
            Some(fc)
        } else {
            let mut walk = cur;
            loop {
                if walk == self.root {
                    break None;
                }
                if let Some(ns) = self.doc.node(walk).next_sibling {
                    break Some(ns);
                }
                match self.doc.node(walk).parent {
                    Some(p) if p != self.root => walk = p,
                    _ => break None,
                }
            }
        };
        Some(cur)
    }
}

impl Document {
    /// Deep-copy `src_id` and its descendants out of `src` into this document.
    ///
    /// Returns the new root of the copy, detached from any parent. Used by
    /// `innerHTML` (which parses into a scratch document first) and by
    /// `cloneNode`, where `src` and `self` are the same document.
    pub fn import_subtree(&mut self, src: &Document, src_id: NodeId) -> NodeId {
        let new_root = self.create(src.data(src_id).clone());
        // Explicit stack of (source, destination-parent) pairs: a document can
        // be deeper than the native stack tolerates.
        let mut stack: Vec<(NodeId, NodeId)> = src
            .children(src_id)
            .map(|c| (c, new_root))
            .collect::<Vec<_>>();
        stack.reverse();

        while let Some((src_child, dst_parent)) = stack.pop() {
            let copy = self.create(src.data(src_child).clone());
            self.append(dst_parent, copy);
            // A copied <template> keeps its own detached content fragment.
            if let Some(tpl) = src.element(src_child).and_then(|e| e.template_contents) {
                let tpl_copy = self.import_subtree(src, tpl);
                if let Some(el) = self.element_mut(copy) {
                    el.template_contents = Some(tpl_copy);
                }
            }
            let mut kids: Vec<_> = src.children(src_child).map(|c| (c, copy)).collect();
            kids.reverse();
            stack.extend(kids);
        }
        new_root
    }

    /// Copy `id` within this document. `deep = false` copies the node alone.
    pub fn clone_node(&mut self, id: NodeId, deep: bool) -> NodeId {
        if !deep {
            return self.create(self.data(id).clone());
        }
        // Split the borrow by cloning the source shallowly first: import_subtree
        // needs &self and &mut self at once, so walk it manually here.
        let new_root = self.create(self.data(id).clone());
        let mut stack: Vec<(NodeId, NodeId)> = self.children(id).map(|c| (c, new_root)).collect();
        stack.reverse();
        while let Some((src_child, dst_parent)) = stack.pop() {
            let data = self.data(src_child).clone();
            let copy = self.create(data);
            self.append(dst_parent, copy);
            let mut kids: Vec<_> = self.children(src_child).map(|c| (c, copy)).collect();
            kids.reverse();
            stack.extend(kids);
        }
        new_root
    }

    /// Is `ancestor` an inclusive ancestor of `id`? (`Node.contains`)
    pub fn contains(&self, ancestor: NodeId, id: NodeId) -> bool {
        ancestor == id || self.ancestors(id).any(|a| a == ancestor)
    }
}
