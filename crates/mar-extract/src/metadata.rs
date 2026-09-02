//! Page metadata: title, description, author, dates, canonical URL, feeds.
//!
//! Read from the places publishers actually populate, in order of how much they
//! can be trusted: JSON-LD, then OpenGraph and Twitter cards, then plain meta
//! tags, then the document itself.

use mar_dom::{Document, LocalName, NodeId};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Metadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub site_name: Option<String>,
    /// Publication date, as the page states it. Not normalised: publishers use
    /// several formats and guessing wrong is worse than passing it through.
    pub published: Option<String>,
    pub modified: Option<String>,
    pub canonical_url: Option<String>,
    pub image: Option<String>,
    pub language: Option<String>,
    /// `<meta name="robots">`, so a caller can honour `noindex` if it wants to.
    pub robots: Option<String>,
    /// RSS and Atom feeds declared by the page.
    pub feeds: Vec<Feed>,
    /// Schema.org `@type` values found in JSON-LD, e.g. "NewsArticle".
    pub schema_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Feed {
    pub url: String,
    pub title: Option<String>,
    pub kind: String,
}

/// Collect metadata from a parsed document.
pub fn extract(doc: &Document) -> Metadata {
    let mut meta = Metadata::default();

    let tags = collect_meta_tags(doc);
    let json_ld = collect_json_ld(doc);

    // JSON-LD first: it is structured data the publisher wrote on purpose.
    let mut titles = TitleCandidates::default();
    for entry in &json_ld {
        apply_json_ld(&mut meta, entry, &mut titles);
    }
    titles.og = first_of(&tags, &["og:title", "twitter:title"]);
    titles.document = document_title(doc);
    meta.title = titles.pick();

    // OpenGraph and Twitter cards fill the gaps.
    meta.description = meta.description.or_else(|| {
        first_of(
            &tags,
            &["og:description", "twitter:description", "description"],
        )
    });
    meta.author = meta
        .author
        .or_else(|| first_of(&tags, &["author", "article:author", "twitter:creator"]));
    meta.site_name = meta
        .site_name
        .or_else(|| first_of(&tags, &["og:site_name", "application-name"]));
    meta.published = meta.published.or_else(|| {
        first_of(
            &tags,
            &[
                "article:published_time",
                "datePublished",
                "publish-date",
                "date",
                "dc.date",
            ],
        )
    });
    meta.modified = meta
        .modified
        .or_else(|| first_of(&tags, &["article:modified_time", "dateModified", "lastmod"]));
    meta.image = meta
        .image
        .or_else(|| first_of(&tags, &["og:image", "twitter:image", "twitter:image:src"]));
    meta.robots = first_of(&tags, &["robots"]);

    // Then the document itself.
    meta.title = meta.title.map(clean);
    meta.canonical_url = meta
        .canonical_url
        .or_else(|| link_href(doc, "canonical"))
        .or_else(|| first_of(&tags, &["og:url"]));
    meta.language = html_lang(doc).or_else(|| first_of(&tags, &["og:locale"]));
    meta.feeds = collect_feeds(doc);

    // A description is worth having even when nothing declared one: the first
    // real paragraph is what a search engine would show.
    if meta.description.is_none() {
        meta.description = first_paragraph(doc);
    }

    meta.description = meta.description.map(clean);
    meta.author = meta.author.map(clean);
    meta.site_name = meta.site_name.map(clean);
    meta
}

fn clean(value: String) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_of(tags: &HashMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| tags.get(&k.to_ascii_lowercase()).cloned())
        .filter(|v| !v.trim().is_empty())
}

/// Index every `<meta>` by `name`, `property` or `itemprop`.
fn collect_meta_tags(doc: &Document) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for id in doc.descendants(doc.root()) {
        let Some(el) = doc.element(id) else { continue };
        if el.local_name().as_ref() != "meta" {
            continue;
        }
        let Some(content) = el.attr(&LocalName::from("content")) else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }
        for key_attr in ["property", "name", "itemprop"] {
            if let Some(key) = el.attr(&LocalName::from(key_attr)) {
                // First declaration wins: pages repeat og:image for galleries.
                out.entry(key.to_ascii_lowercase())
                    .or_insert_with(|| content.to_owned());
            }
        }
    }
    out
}

fn document_title(doc: &Document) -> Option<String> {
    let head = doc.head()?;
    let title = doc.children(head).find(|&c| {
        doc.element(c)
            .is_some_and(|e| e.local_name().as_ref() == "title")
    })?;
    let text = doc.text_content(title);
    (!text.trim().is_empty()).then(|| text.trim().to_owned())
}

fn html_lang(doc: &Document) -> Option<String> {
    let html = doc.document_element()?;
    doc.element(html)?
        .attr(&LocalName::from("lang"))
        .map(str::to_owned)
        .filter(|v| !v.trim().is_empty())
}

fn link_href(doc: &Document, rel: &str) -> Option<String> {
    doc.descendants(doc.root()).find_map(|id| {
        let el = doc.element(id)?;
        if el.local_name().as_ref() != "link" {
            return None;
        }
        let rels = el.attr(&LocalName::from("rel"))?.to_ascii_lowercase();
        rels.split_ascii_whitespace()
            .any(|r| r == rel)
            .then(|| el.attr(&LocalName::from("href")))
            .flatten()
            .map(str::to_owned)
    })
}

fn collect_feeds(doc: &Document) -> Vec<Feed> {
    doc.descendants(doc.root())
        .filter_map(|id| {
            let el = doc.element(id)?;
            if el.local_name().as_ref() != "link" {
                return None;
            }
            let rels = el.attr(&LocalName::from("rel"))?.to_ascii_lowercase();
            if !rels.split_ascii_whitespace().any(|r| r == "alternate") {
                return None;
            }
            let ty = el.attr(&LocalName::from("type"))?.to_ascii_lowercase();
            let kind = if ty.contains("atom") {
                "atom"
            } else if ty.contains("rss") || ty.contains("xml") {
                "rss"
            } else {
                return None;
            };
            Some(Feed {
                url: el.attr(&LocalName::from("href"))?.to_owned(),
                title: el.attr(&LocalName::from("title")).map(str::to_owned),
                kind: kind.to_owned(),
            })
        })
        .collect()
}

fn first_paragraph(doc: &Document) -> Option<String> {
    let body = doc.body()?;
    doc.descendants(body).find_map(|id| {
        let el = doc.element(id)?;
        if el.local_name().as_ref() != "p" {
            return None;
        }
        let text = clean(doc.text_content(id));
        // Short paragraphs are captions, bylines and cookie notices.
        (text.chars().count() >= 80).then(|| truncate_chars(&text, 300))
    })
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    // Break at a word boundary so the summary does not end mid-word.
    match cut.rfind(' ') {
        Some(i) if i > max / 2 => format!("{}…", &cut[..i]),
        _ => format!("{cut}…"),
    }
}

/// Parse every `<script type="application/ld+json">` block.
fn collect_json_ld(doc: &Document) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for id in doc.descendants(doc.root()) {
        let Some(el) = doc.element(id) else { continue };
        if el.local_name().as_ref() != "script" {
            continue;
        }
        if el
            .attr(&LocalName::from("type"))
            .is_none_or(|t| !t.eq_ignore_ascii_case("application/ld+json"))
        {
            continue;
        }
        let body = doc.text_content(id);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
            // Malformed JSON-LD is common and never worth failing over.
            continue;
        };
        flatten_json_ld(value, &mut out);
    }
    out
}

/// JSON-LD arrives as an object, an array, or an object with `@graph`.
fn flatten_json_ld(value: serde_json::Value, out: &mut Vec<serde_json::Value>) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                flatten_json_ld(item, out);
            }
        }
        serde_json::Value::Object(ref map) => {
            if let Some(graph) = map.get("@graph").cloned() {
                flatten_json_ld(graph, out);
            }
            out.push(value);
        }
        _ => {}
    }
}

/// The several places a page states its title, kept apart so they can be
/// cross-checked rather than taken in a fixed order.
#[derive(Debug, Default)]
struct TitleCandidates {
    /// schema.org `headline`. Correct per the spec, but some large sites put
    /// a one-line summary here instead.
    headline: Option<String>,
    /// schema.org `name`.
    name: Option<String>,
    og: Option<String>,
    /// The `<title>` element, usually with a site suffix attached.
    document: Option<String>,
}

impl TitleCandidates {
    fn pick(&self) -> Option<String> {
        // The document title is the tiebreaker: whichever structured value the
        // page also put in its <title> is the one the page means as its title.
        // This is what separates a real headline from a summary parked in the
        // headline field.
        let reference = self
            .document
            .as_deref()
            .or(self.og.as_deref())
            .map(str::to_ascii_lowercase);

        if let Some(reference) = &reference {
            for candidate in [&self.headline, &self.name] {
                if let Some(value) = candidate
                    && !value.trim().is_empty()
                    && reference.contains(&value.to_ascii_lowercase())
                {
                    return Some(value.clone());
                }
            }
        }

        self.headline
            .clone()
            .or_else(|| self.name.clone())
            .or_else(|| self.og.clone().map(|t| strip_site_suffix(&t)))
            .or_else(|| self.document.clone().map(|t| strip_site_suffix(&t)))
    }
}

/// Drop a trailing " | Site Name" or " - Site Name" from a document title.
///
/// Only when the tail is short: an em dash inside a real headline is common,
/// and cutting there would truncate the title.
fn strip_site_suffix(title: &str) -> String {
    for separator in [" — ", " – ", " | ", " - ", " · ", " :: "] {
        if let Some((head, tail)) = title.rsplit_once(separator)
            && !head.trim().is_empty()
            && tail.chars().count() <= 40
            && head.chars().count() > tail.chars().count()
        {
            return head.trim().to_owned();
        }
    }
    title.trim().to_owned()
}

fn apply_json_ld(meta: &mut Metadata, entry: &serde_json::Value, titles: &mut TitleCandidates) {
    let Some(map) = entry.as_object() else { return };

    if let Some(ty) = map.get("@type") {
        match ty {
            serde_json::Value::String(s) => meta.schema_types.push(s.clone()),
            serde_json::Value::Array(items) => {
                for item in items {
                    if let Some(s) = item.as_str() {
                        meta.schema_types.push(s.to_owned());
                    }
                }
            }
            _ => {}
        }
    }

    // Only article-ish entries should supply the page's own metadata. A
    // BreadcrumbList or Organization block on the same page would otherwise
    // overwrite the title with the site name.
    let is_content = meta.schema_types.iter().any(|t| {
        t.contains("Article")
            || t.contains("BlogPosting")
            || t.contains("WebPage")
            || t == "Recipe"
            || t == "Product"
            || t == "Report"
    });
    if !is_content {
        return;
    }

    let string_at = |key: &str| map.get(key).and_then(|v| v.as_str()).map(str::to_owned);

    if titles.headline.is_none() {
        titles.headline = string_at("headline");
    }
    if titles.name.is_none() {
        titles.name = string_at("name");
    }
    meta.description = meta.description.take().or_else(|| string_at("description"));
    meta.published = meta.published.take().or_else(|| string_at("datePublished"));
    meta.modified = meta.modified.take().or_else(|| string_at("dateModified"));
    meta.canonical_url = meta.canonical_url.take().or_else(|| string_at("url"));

    if meta.author.is_none() {
        meta.author = map.get("author").and_then(author_name);
    }
    if meta.image.is_none() {
        meta.image = map.get("image").and_then(image_url);
    }
    if meta.site_name.is_none() {
        meta.site_name = map
            .get("publisher")
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .map(str::to_owned);
    }
}

/// `author` is a string, an object with `name`, or an array of either.
fn author_name(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => map.get("name")?.as_str().map(str::to_owned),
        serde_json::Value::Array(items) => {
            let names: Vec<String> = items.iter().filter_map(author_name).collect();
            (!names.is_empty()).then(|| names.join(", "))
        }
        _ => None,
    }
}

/// `image` is a URL string, an ImageObject, or an array of either.
fn image_url(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => map.get("url")?.as_str().map(str::to_owned),
        serde_json::Value::Array(items) => items.iter().find_map(image_url),
        _ => None,
    }
}

/// Resolve every relative URL in `meta` against `base`.
pub fn resolve_urls(meta: &mut Metadata, base: &url::Url) {
    let resolve = |value: &mut Option<String>| {
        if let Some(raw) = value.as_deref()
            && let Ok(absolute) = base.join(raw)
        {
            *value = Some(absolute.to_string());
        }
    };
    resolve(&mut meta.canonical_url);
    resolve(&mut meta.image);
    for feed in &mut meta.feeds {
        if let Ok(absolute) = base.join(&feed.url) {
            feed.url = absolute.to_string();
        }
    }
}

/// Node id of the `<title>` element, if the caller wants to edit it.
pub fn title_node(doc: &Document) -> Option<NodeId> {
    let head = doc.head()?;
    doc.children(head).find(|&c| {
        doc.element(c)
            .is_some_and(|e| e.local_name().as_ref() == "title")
    })
}
