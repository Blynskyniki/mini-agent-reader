//! Finding the article inside a page.
//!
//! The approach is the one Readability established: score block elements by
//! how much prose-shaped text they hold, penalise anything that looks like
//! navigation or advertising, then take the best-scoring subtree together with
//! its close siblings. It is heuristic, and the comments below say what each
//! number is for so the heuristics can be tuned against real failures rather
//! than guessed at.

use mar_dom::{Document, LocalName, NodeData, NodeId};
use std::collections::HashMap;

/// Tags that never carry article text and are dropped before scoring.
const STRIPPED_TAGS: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "canvas", "iframe", "object", "embed",
    "applet", "link", "meta", "form", "button", "input", "select", "textarea", "dialog",
];

/// Tags that are structurally never the article body.
const CHROME_TAGS: &[&str] = &["nav", "header", "footer", "aside", "menu"];

/// Substrings in `id`/`class` that suggest chrome rather than content.
const NEGATIVE_HINTS: &[&str] = &[
    "banner", "combx", "comment", "community", "cover-wrap", "disqus", "extra", "foot", "header",
    "legend", "menu", "modal", "related", "remark", "replies", "rss", "shoutbox", "sidebar",
    "skyscraper", "social", "sponsor", "supplemental", "ad-break", "agegate", "pagination",
    "pager", "popup", "yom-remote", "share", "promo", "newsletter", "subscribe", "cookie",
    "breadcrumb", "widget", "sitemap", "toolbar", "masthead", "nav-", "-nav", "advert",
    // Reference and citation lists are long, comma-rich and prose-like, so they
    // out-score the article body unless they are named explicitly.
    "reference", "reflist", "citation", "footnote", "bibliograph", "endnote", "further-reading",
    "navbox", "infobox", "metadata", "catlinks", "portal", "mw-editsection", "hatnote",
];

/// Substrings that suggest the opposite.
const POSITIVE_HINTS: &[&str] = &[
    "article", "body", "content", "entry", "hentry", "h-entry", "main", "page", "pagination",
    "post", "text", "blog", "story", "column", "prose", "markdown",
];

/// Elements that start a scoring candidate.
const BLOCK_TAGS: &[&str] = &[
    "p", "td", "pre", "article", "section", "div", "blockquote", "li", "dd", "figure",
];

/// A scored candidate for the main content.
#[derive(Debug, Clone, Copy)]
pub struct Candidate {
    pub node: NodeId,
    pub score: f64,
    /// Characters of text in the subtree.
    pub text_len: usize,
    /// Share of that text sitting inside links. High means navigation.
    pub link_density: f64,
}

/// The outcome of extraction.
#[derive(Debug)]
pub struct Article {
    /// Root of the extracted content in the cleaned document.
    pub root: NodeId,
    /// The cleaned document. Nodes in `root` belong to this, not the original.
    pub document: Document,
    pub text_len: usize,
    pub score: f64,
    /// True when nothing scored well and the whole body was used.
    pub fell_back: bool,
}

/// Extract the main content of `doc`.
///
/// The input is left untouched: cleaning happens on a copy, so a caller can
/// still serve the full page alongside the article.
pub fn extract(doc: &Document) -> Article {
    let mut work = clone_document(doc);
    let body = work.body().unwrap_or_else(|| work.root());

    strip_noise(&mut work, body);

    let candidates = score_candidates(&work, body);
    let best = pick_best(&work, &candidates);

    match best {
        Some(best) if best.text_len >= MIN_ARTICLE_CHARS => {
            let root = grow_with_siblings(&mut work, best, &candidates);
            clean_conditionally(&mut work, root);
            let text_len = visible_text(&work, root).chars().count();
            Article {
                root,
                document: work,
                text_len,
                score: best.score,
                fell_back: false,
            }
        }
        // Nothing scored: hand back the whole body rather than nothing at all.
        _ => {
            let text_len = visible_text(&work, body).chars().count();
            Article {
                root: body,
                document: work,
                text_len,
                score: best.map(|b| b.score).unwrap_or(0.0),
                fell_back: true,
            }
        }
    }
}

/// Below this many characters an "article" is more likely a teaser or a
/// cookie notice than the real content, so the whole body is used instead.
const MIN_ARTICLE_CHARS: usize = 200;

fn clone_document(src: &Document) -> Document {
    let mut out = Document::new();
    out.quirks_mode = src.quirks_mode;
    let root = out.root();
    for child in src.children(src.root()) {
        let copy = out.import_subtree(src, child);
        out.append(root, copy);
    }
    out
}

/// Remove everything that cannot be article text.
fn strip_noise(doc: &mut Document, root: NodeId) {
    let doomed: Vec<NodeId> = doc
        .descendants(root)
        .filter(|&id| {
            let Some(el) = doc.element(id) else {
                // Comments carry no reading value and bloat the output.
                return matches!(doc.data(id), NodeData::Comment(_));
            };
            let tag = el.local_name().as_ref();
            if STRIPPED_TAGS.contains(&tag) {
                return true;
            }
            // aria-hidden and hidden are explicit statements that a human is
            // not meant to read this.
            if el.has_attr(&LocalName::from("hidden")) {
                return true;
            }
            if el.attr(&LocalName::from("aria-hidden")) == Some("true") {
                return true;
            }
            // display:none set inline is the same statement.
            if el
                .attr(&LocalName::from("style"))
                .is_some_and(|s| {
                    let s = s.replace(' ', "").to_ascii_lowercase();
                    s.contains("display:none") || s.contains("visibility:hidden")
                })
            {
                return true;
            }
            false
        })
        .collect();
    for id in doomed {
        doc.detach(id);
    }
}

/// How many item siblings the list containing `id` has.
fn siblings_in_list(doc: &Document, id: NodeId) -> usize {
    let Some(parent) = doc.node(id).parent else {
        return 0;
    };
    doc.children(parent)
        .filter(|&c| {
            doc.element(c)
                .is_some_and(|e| matches!(e.local_name().as_ref(), "li" | "dd"))
        })
        .count()
}

fn hint_text(doc: &Document, id: NodeId) -> String {
    let Some(el) = doc.element(id) else {
        return String::new();
    };
    let mut s = String::new();
    if let Some(v) = el.attr(&LocalName::from("class")) {
        s.push_str(v);
        s.push(' ');
    }
    if let Some(v) = el.attr(&LocalName::from("id")) {
        s.push_str(v);
        s.push(' ');
    }
    if let Some(v) = el.attr(&LocalName::from("role")) {
        s.push_str(v);
    }
    s.to_ascii_lowercase()
}

/// Multiplier from `id`/`class`/`role` hints.
///
/// A multiplier rather than a flat bonus: a container that has accumulated
/// several hundred points from its children is not meaningfully changed by
/// adding or subtracting twenty-five, but halving it says something.
fn hint_multiplier(doc: &Document, id: NodeId) -> f64 {
    let hints = hint_text(doc, id);
    if hints.is_empty() {
        return 1.0;
    }
    let mut factor = 1.0;
    if NEGATIVE_HINTS.iter().any(|h| hints.contains(h)) {
        factor *= 0.2;
    }
    if POSITIVE_HINTS.iter().any(|h| hints.contains(h)) {
        factor *= 1.4;
    }
    // Explicit semantics beat guessing from names.
    if hints.contains("main") {
        factor *= 1.2;
    }
    factor
}

/// Base score for an element's own tag.
fn tag_score(tag: &str) -> f64 {
    match tag {
        "article" | "main" => 10.0,
        "section" => 5.0,
        "div" => 5.0,
        "pre" | "td" | "blockquote" => 3.0,
        "address" | "ol" | "ul" | "dl" | "dd" | "dt" | "li" | "form" => -3.0,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th" => -5.0,
        _ => 0.0,
    }
}

/// Fraction of the text inside `id` that sits within links.
///
/// A navigation block is almost all link text; an article is almost none.
fn link_density(doc: &Document, id: NodeId) -> f64 {
    let total = visible_text(doc, id).chars().count();
    if total == 0 {
        return 0.0;
    }
    let link_chars: usize = doc
        .descendants(id)
        .filter(|&d| {
            doc.element(d)
                .is_some_and(|e| e.local_name().as_ref() == "a")
        })
        .map(|a| visible_text(doc, a).chars().count())
        .sum();
    link_chars as f64 / total as f64
}

/// Text of a subtree with whitespace collapsed.
pub fn visible_text(doc: &Document, id: NodeId) -> String {
    let raw = doc.text_content(id);
    let mut out = String::with_capacity(raw.len());
    let mut in_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            if !in_space && !out.is_empty() {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out.trim_end().to_owned()
}

/// Score every block element by the prose it and its children contain.
fn score_candidates(doc: &Document, body: NodeId) -> HashMap<NodeId, Candidate> {
    let mut scores: HashMap<NodeId, f64> = HashMap::new();

    for id in doc.descendants(body) {
        let Some(el) = doc.element(id) else { continue };
        let tag = el.local_name().as_ref();
        // Only leaf-ish text blocks seed a score; containers earn theirs from
        // their children, which is what makes the best container win.
        //
        // A paragraph counts fully. A list item counts for much less: a
        // reference list or a link menu has hundreds of them, each long and
        // comma-rich, and at full weight they bury the article body.
        let weight = match tag {
            "p" | "pre" | "blockquote" => 1.0,
            "td" | "figcaption" => 0.6,
            "li" | "dd" => 0.25,
            _ => continue,
        };
        if matches!(tag, "li" | "dd") && siblings_in_list(doc, id) > 25 {
            continue;
        }
        let text = visible_text(doc, id);
        let len = text.chars().count();
        // Under 25 characters is a label, a caption or a button, not prose.
        if len < 25 {
            continue;
        }

        // One point for existing, one per comma (a rough proxy for sentence
        // structure), and one per 100 characters, capped so a single huge
        // block cannot outweigh a genuinely structured article.
        let mut base = 1.0;
        base += text.matches(',').count() as f64;
        base += text.matches('，').count() as f64;
        base += (len as f64 / 100.0).min(3.0);
        base *= weight;

        // The score flows up: full to the parent, half to the grandparent,
        // and a third to the one above that.
        let ancestors: Vec<NodeId> = doc.ancestors(id).take(3).collect();
        for (level, ancestor) in ancestors.into_iter().enumerate() {
            if !doc.data(ancestor).is_element() {
                continue;
            }
            let divisor = match level {
                0 => 1.0,
                1 => 2.0,
                _ => level as f64 * 3.0,
            };
            *scores.entry(ancestor).or_insert(0.0) += base / divisor;
        }
    }

    // Fold in per-element adjustments, then discount by link density.
    let mut out = HashMap::with_capacity(scores.len());
    for (id, content_score) in scores {
        let Some(el) = doc.element(id) else { continue };
        let tag = el.local_name().as_ref().to_owned();
        if !BLOCK_TAGS.contains(&tag.as_str()) && !matches!(tag.as_str(), "main" | "body") {
            continue;
        }
        // Chrome elements are excluded outright: a <nav> full of prose is
        // still navigation.
        if CHROME_TAGS.contains(&tag.as_str()) {
            continue;
        }

        let density = link_density(doc, id);
        let text_len = visible_text(doc, id).chars().count();
        let score =
            (content_score + tag_score(&tag)) * hint_multiplier(doc, id) * (1.0 - density);

        out.insert(
            id,
            Candidate {
                node: id,
                score,
                text_len,
                link_density: density,
            },
        );
    }
    out
}

fn pick_best(doc: &Document, candidates: &HashMap<NodeId, Candidate>) -> Option<Candidate> {
    let mut best: Option<Candidate> = None;
    for candidate in candidates.values() {
        // A block that is mostly links is a menu however well it scored.
        if candidate.link_density > 0.5 {
            continue;
        }
        match best {
            Some(current) if current.score >= candidate.score => {}
            _ => best = Some(*candidate),
        }
    }

    // Prefer an ancestor when it scores nearly as well: containers hold the
    // figures and headings that belong to the article, and picking the
    // deepest high-scoring node tends to cut them off.
    let mut chosen = best?;
    for ancestor in doc.ancestors(chosen.node) {
        let Some(parent) = candidates.get(&ancestor) else {
            continue;
        };
        if parent.link_density <= 0.5 && parent.score >= chosen.score * 0.85 {
            chosen = *parent;
        }
    }
    Some(chosen)
}

/// Pull in sibling blocks that plausibly belong to the same article.
///
/// Publishers routinely split an article across sibling `<div>`s, so the
/// top-scoring node alone often stops mid-story.
fn grow_with_siblings(
    doc: &mut Document,
    best: Candidate,
    candidates: &HashMap<NodeId, Candidate>,
) -> NodeId {
    let Some(parent) = doc.node(best.node).parent else {
        return best.node;
    };
    if !doc.data(parent).is_element() {
        return best.node;
    }

    // A sibling has to clear a bar set relative to the winner, so a weak page
    // does not sweep in its whole layout.
    let threshold = (best.score * 0.2).max(10.0);
    let siblings: Vec<NodeId> = doc.children(parent).collect();
    let keep: Vec<NodeId> = siblings
        .iter()
        .copied()
        .filter(|&sib| {
            if sib == best.node {
                return true;
            }
            let Some(el) = doc.element(sib) else {
                return false;
            };
            if CHROME_TAGS.contains(&el.local_name().as_ref()) {
                return false;
            }
            if let Some(c) = candidates.get(&sib)
                && c.score >= threshold && c.link_density < 0.4 {
                    return true;
                }
                // Fall through: a low score can simply mean the section holds
                // its text in tags that do not seed scores, such as <dl> or
                // <pre>. The structural test below still applies.
            let tag = el.local_name().as_ref().to_owned();
            let text = visible_text(doc, sib);
            let len = text.chars().count();
            let density = link_density(doc, sib);

            // An unscored paragraph next to the article is usually part of it.
            if tag == "p" {
                return len > 80 && density < 0.25;
            }

            // A section that opens with a heading and carries real prose is a
            // continuation of the article: reference pages put "Examples" and
            // "Interfaces" in exactly this shape, and dropping them loses half
            // the page.
            let negative = NEGATIVE_HINTS
                .iter()
                .any(|h| hint_text(doc, sib).contains(h));
            matches!(tag.as_str(), "section" | "div" | "article")
                && !negative
                && len > 150
                && density < 0.35
                && doc.descendants(sib).any(|d| {
                    doc.element(d).is_some_and(|e| {
                        matches!(e.local_name().as_ref(), "h1" | "h2" | "h3" | "h4")
                    })
                })
        })
        .collect();

    if keep.len() <= 1 {
        return best.node;
    }

    // Build a fresh container holding just the kept siblings, in order.
    let container = doc.create_element(
        mar_dom::QualName::new(None, mar_dom::ns!(html), LocalName::from("div")),
        Vec::new(),
    );
    for sib in keep {
        doc.append(container, sib);
    }
    container
}

/// Drop leftover blocks inside the chosen article that still look like chrome.
fn clean_conditionally(doc: &mut Document, root: NodeId) {
    let doomed: Vec<NodeId> = doc
        .descendants(root)
        .filter(|&id| {
            let Some(el) = doc.element(id) else {
                return false;
            };
            let tag = el.local_name().as_ref();
            if CHROME_TAGS.contains(&tag) {
                return true;
            }
            if !matches!(tag, "div" | "section" | "ul" | "ol" | "table") {
                return false;
            }
            let text = visible_text(doc, id);
            let len = text.chars().count();
            if len == 0 {
                // Keep empty wrappers that exist to hold media.
                return !doc.descendants(id).any(|d| {
                    doc.element(d).is_some_and(|e| {
                        matches!(e.local_name().as_ref(), "img" | "video" | "picture" | "figure")
                    })
                });
            }
            // A short block that is mostly links, inside an article, is a
            // "related stories" box.
            let density = link_density(doc, id);
            let negative = NEGATIVE_HINTS
                .iter()
                .any(|h| hint_text(doc, id).contains(h));
            (density > 0.5 && len < 400) || (negative && density > 0.25) || (negative && len < 150)
        })
        .collect();

    for id in doomed {
        // A node may already have gone with an ancestor.
        if doc.node(id).parent.is_some() && id != root {
            doc.detach(id);
        }
    }
}
