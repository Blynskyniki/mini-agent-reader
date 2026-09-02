//! Turning a rendered page into something worth sending to a model: the
//! article, its metadata, and Markdown instead of HTML.

pub mod markdown;
pub mod metadata;
pub mod readability;

pub use markdown::{MarkdownOptions, to_markdown};
pub use metadata::{Feed, Metadata};
pub use readability::{Article, visible_text};

use mar_dom::Document;
use serde::Serialize;

/// Everything the reader produces for one page.
#[derive(Debug, Serialize)]
pub struct Reading {
    #[serde(flatten)]
    pub metadata: Metadata,
    /// The article as Markdown.
    pub content: String,
    /// Characters of extracted text.
    pub length: usize,
    /// True when no candidate scored well and the whole body was used, so the
    /// content probably includes navigation.
    pub low_confidence: bool,
}

/// Read a parsed document: find the article, collect metadata, emit Markdown.
pub fn read(doc: &Document, options: &MarkdownOptions) -> Reading {
    let mut metadata = metadata::extract(doc);
    if let Some(base) = &options.base_url {
        metadata::resolve_urls(&mut metadata, base);
    }

    let article = readability::extract(doc);
    let content = to_markdown(&article.document, article.root, options);

    Reading {
        metadata,
        length: article.text_len,
        low_confidence: article.fell_back,
        content,
    }
}
