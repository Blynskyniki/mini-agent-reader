//! HTML serialization: `innerHTML`, `outerHTML` and whole-document output.

use crate::arena::{Document, NodeData, NodeId};
use std::fmt::Write;

/// Elements that never have children and are written without a closing tag.
fn is_void(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Elements whose text children are CDATA: their content is written raw.
fn is_raw_text(tag: &str) -> bool {
    matches!(
        tag,
        "script" | "style" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext" | "noscript"
    )
}

fn escape_text(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\u{00A0}' => out.push_str("&nbsp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}

fn escape_attr(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\u{00A0}' => out.push_str("&nbsp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// Serialize the children of `id` (`Element.innerHTML`).
pub fn inner_html(doc: &Document, id: NodeId) -> String {
    let mut out = String::new();
    for child in doc.children(id) {
        write_node(doc, child, &mut out);
    }
    out
}

/// Serialize `id` including its own tags (`Element.outerHTML`).
pub fn outer_html(doc: &Document, id: NodeId) -> String {
    let mut out = String::new();
    write_node(doc, id, &mut out);
    out
}

/// Serialize the whole document, doctype included.
pub fn document_html(doc: &Document) -> String {
    let mut out = String::new();
    for child in doc.children(doc.root()) {
        write_node(doc, child, &mut out);
    }
    out
}

fn write_node(doc: &Document, id: NodeId, out: &mut String) {
    match doc.data(id) {
        NodeData::Document => {
            for child in doc.children(id) {
                write_node(doc, child, out);
            }
        }
        NodeData::Doctype { name, .. } => {
            let _ = write!(out, "<!DOCTYPE {name}>");
        }
        NodeData::Text(text) => {
            // Inside <script>/<style> the content is not escaped.
            let parent_is_raw = doc
                .node(id)
                .parent
                .and_then(|p| doc.element(p))
                .is_some_and(|e| is_raw_text(e.local_name().as_ref()));
            if parent_is_raw {
                out.push_str(text);
            } else {
                escape_text(text, out);
            }
        }
        NodeData::Comment(text) => {
            let _ = write!(out, "<!--{text}-->");
        }
        NodeData::ProcessingInstruction { target, data } => {
            let _ = write!(out, "<?{target} {data}>");
        }
        NodeData::Element(el) => {
            let tag = el.local_name().as_ref();
            out.push('<');
            out.push_str(tag);
            for attr in &el.attrs {
                out.push(' ');
                // Namespaced attributes keep their conventional prefix.
                let ns = attr.name.ns.as_ref();
                if !ns.is_empty() {
                    let prefix = match ns {
                        "http://www.w3.org/XML/1998/namespace" => Some("xml:"),
                        "http://www.w3.org/1999/xlink" => Some("xlink:"),
                        "http://www.w3.org/2000/xmlns/" => Some("xmlns:"),
                        _ => None,
                    };
                    if let Some(p) = prefix {
                        out.push_str(p);
                    }
                }
                out.push_str(&attr.name.local);
                out.push_str("=\"");
                escape_attr(&attr.value, out);
                out.push('"');
            }
            out.push('>');

            if is_void(tag) {
                return;
            }

            // <template> serializes its detached content, not its children.
            if let Some(contents) = el.template_contents {
                for child in doc.children(contents) {
                    write_node(doc, child, out);
                }
            } else {
                for child in doc.children(id) {
                    write_node(doc, child, out);
                }
            }

            let _ = write!(out, "</{tag}>");
        }
    }
}
