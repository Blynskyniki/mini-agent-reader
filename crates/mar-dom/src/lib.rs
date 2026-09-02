//! Arena-backed HTML DOM: parsing, traversal, CSS selector matching and
//! serialization, with no per-node reference counting.

pub mod arena;
pub mod parser;
pub mod select;
pub mod serialize;

pub use arena::{ChildIter, Document, ElementData, Node, NodeData, NodeId};
pub use parser::{ParsedDocument, parse_fragment_document, parse_html, parse_html_bytes};
pub use select::{ElementRef, Matcher, SelectorError, query_selector, query_selector_all};
pub use serialize::{document_html, inner_html, outer_html};

pub use html5ever::{Attribute, LocalName, Namespace, QualName, local_name, ns};
pub use markup5ever::interface::QuirksMode;
pub use tendril::StrTendril;
