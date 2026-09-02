//! DOM to Markdown.
//!
//! Markdown is the point of the whole exercise: it carries the structure a
//! model needs (headings, lists, tables, links, code) at a fraction of the
//! tokens the equivalent HTML would cost.

use mar_dom::{Document, LocalName, NodeData, NodeId};
use url::Url;

#[derive(Debug, Clone)]
pub struct MarkdownOptions {
    /// Resolve relative links and images against this URL.
    pub base_url: Option<Url>,
    /// Emit images as `![alt](src)`. Off produces text-only output.
    pub include_images: bool,
    /// Emit links as `[text](href)`. Off keeps the link text only, which is
    /// noticeably cheaper on pages dense with navigation.
    pub include_links: bool,
    /// Hard limit on output characters. Truncation happens at a block boundary.
    pub max_chars: Option<usize>,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        MarkdownOptions {
            base_url: None,
            include_images: true,
            include_links: true,
            max_chars: None,
        }
    }
}

/// Convert a subtree to Markdown.
pub fn to_markdown(doc: &Document, root: NodeId, options: &MarkdownOptions) -> String {
    let mut writer = Writer {
        doc,
        options,
        out: String::new(),
        list_stack: Vec::new(),
        in_pre: false,
        truncated: false,
    };
    writer.write_children(root);
    let mut text = writer.out;
    normalize_blank_lines(&mut text);
    text
}

/// State of the list we are inside, if any.
#[derive(Debug, Clone, Copy)]
struct ListFrame {
    ordered: bool,
    index: usize,
}

struct Writer<'a> {
    doc: &'a Document,
    options: &'a MarkdownOptions,
    out: String,
    list_stack: Vec<ListFrame>,
    /// Inside `<pre>`, whitespace is significant and nothing is escaped.
    in_pre: bool,
    truncated: bool,
}

impl Writer<'_> {
    fn over_budget(&self) -> bool {
        self.options
            .max_chars
            .is_some_and(|max| self.out.len() >= max)
    }

    fn push(&mut self, s: &str) {
        if self.truncated {
            return;
        }
        self.out.push_str(s);
        if self.over_budget() {
            self.truncated = true;
        }
    }

    /// Start a new block, leaving exactly one blank line before it.
    fn block_break(&mut self) {
        if self.out.is_empty() {
            return;
        }
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.out.ends_with("\n\n") {
            if self.out.ends_with('\n') {
                self.out.push('\n');
            } else {
                self.out.push_str("\n\n");
            }
        }
    }

    fn line_break(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn resolve(&self, raw: &str) -> String {
        match &self.options.base_url {
            Some(base) => base
                .join(raw)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| raw.to_owned()),
            None => raw.to_owned(),
        }
    }

    fn attr(&self, id: NodeId, name: &str) -> Option<String> {
        self.doc
            .element(id)
            .and_then(|e| e.attr(&LocalName::from(name)))
            .map(str::to_owned)
    }

    fn write_children(&mut self, id: NodeId) {
        for child in self.doc.children(id) {
            if self.truncated {
                return;
            }
            self.write_node(child);
        }
    }

    fn write_node(&mut self, id: NodeId) {
        match self.doc.data(id) {
            NodeData::Text(text) => {
                if self.in_pre {
                    self.push(text);
                } else {
                    let collapsed = collapse_whitespace(text);
                    if !collapsed.is_empty() {
                        // Do not open a block with a stray space.
                        if collapsed == " " && (self.out.is_empty() || self.out.ends_with('\n')) {
                            return;
                        }
                        self.push(&escape_inline(&collapsed));
                    }
                }
            }
            NodeData::Element(_) => self.write_element(id),
            // Comments, doctype and processing instructions carry no reading value.
            _ => {}
        }
    }

    fn write_element(&mut self, id: NodeId) {
        let tag = self
            .doc
            .element(id)
            .map(|e| e.local_name().to_string())
            .unwrap_or_default();

        match tag.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = tag[1..].parse::<usize>().unwrap_or(1);
                self.block_break();
                self.push(&"#".repeat(level));
                self.push(" ");
                self.write_children(id);
                self.block_break();
            }
            "p" => {
                self.block_break();
                self.write_children(id);
                self.block_break();
            }
            "br" => self.push("  \n"),
            "hr" => {
                self.block_break();
                self.push("---");
                self.block_break();
            }
            "strong" | "b" => self.wrap_inline(id, "**"),
            "em" | "i" => self.wrap_inline(id, "*"),
            "del" | "s" | "strike" => self.wrap_inline(id, "~~"),
            "code" if !self.in_pre => {
                let text = self.doc.text_content(id);
                // A backtick inside code needs a longer fence around it.
                let fence = "`".repeat(longest_backtick_run(&text) + 1);
                self.push(&fence);
                if text.starts_with('`') || text.ends_with('`') {
                    self.push(" ");
                    self.push(&text);
                    self.push(" ");
                } else {
                    self.push(&text);
                }
                self.push(&fence);
            }
            "pre" => self.write_pre(id),
            "blockquote" => self.write_blockquote(id),
            "a" => self.write_link(id),
            "img" => self.write_image(id),
            "picture" | "figure" => {
                self.block_break();
                self.write_children(id);
                self.block_break();
            }
            "figcaption" => {
                self.line_break();
                self.push("*");
                self.write_children(id);
                self.push("*");
                self.block_break();
            }
            "ul" | "ol" => self.write_list(id, tag == "ol"),
            "li" => self.write_list_item(id),
            "dl" => {
                self.block_break();
                self.write_children(id);
                self.block_break();
            }
            "dt" => {
                self.line_break();
                self.push("**");
                self.write_children(id);
                self.push("**");
                self.line_break();
            }
            "dd" => {
                self.push(": ");
                self.write_children(id);
                self.line_break();
            }
            "table" => self.write_table(id),
            // Structural containers contribute nothing themselves.
            "div" | "section" | "article" | "main" | "span" | "body" | "html" | "header"
            | "footer" | "aside" | "nav" | "tbody" | "thead" | "tfoot" | "label" | "small"
            | "sup" | "sub" | "u" | "mark" | "time" | "abbr" | "cite" | "q" | "video" | "audio"
            | "source" | "details" | "summary" | "address" | "font" | "center" => {
                let is_block = !matches!(
                    tag.as_str(),
                    "span"
                        | "small"
                        | "sup"
                        | "sub"
                        | "u"
                        | "mark"
                        | "time"
                        | "abbr"
                        | "cite"
                        | "q"
                        | "label"
                        | "font"
                );
                if is_block {
                    self.block_break();
                }
                self.write_children(id);
                if is_block {
                    self.block_break();
                }
            }
            // Anything unrecognised still gets its text.
            _ => self.write_children(id),
        }
    }

    fn wrap_inline(&mut self, id: NodeId, marker: &str) {
        let text = self.doc.text_content(id);
        // An empty or whitespace-only emphasis produces `****`, which renders
        // as literal asterisks.
        if text.trim().is_empty() {
            return;
        }
        self.push(marker);
        self.write_children(id);
        self.push(marker);
    }

    fn write_pre(&mut self, id: NodeId) {
        self.block_break();
        // A language class is worth preserving: it tells a reader, and a model,
        // what the block is.
        let language = self
            .doc
            .descendants(id)
            .find(|&d| {
                self.doc
                    .element(d)
                    .is_some_and(|e| e.local_name().as_ref() == "code")
            })
            .and_then(|code| self.attr(code, "class"))
            .and_then(|class| {
                class
                    .split_ascii_whitespace()
                    .find_map(|c| {
                        c.strip_prefix("language-")
                            .or_else(|| c.strip_prefix("lang-"))
                    })
                    .map(str::to_owned)
            })
            .unwrap_or_default();

        let body = self.doc.text_content(id);
        let fence = "`".repeat(longest_backtick_run(&body).max(2) + 1);
        self.push(&fence);
        self.push(&language);
        self.push("\n");
        self.push(body.trim_end_matches('\n'));
        self.push("\n");
        self.push(&fence);
        self.block_break();
    }

    fn write_blockquote(&mut self, id: NodeId) {
        self.block_break();
        // Render the contents, then prefix every line. Doing it afterwards
        // handles nested blocks without tracking depth during the walk.
        let start = self.out.len();
        self.write_children(id);
        let inner = self.out.split_off(start);
        let quoted: String = inner
            .trim_matches('\n')
            .lines()
            .map(|line| {
                if line.is_empty() {
                    ">".to_owned()
                } else {
                    format!("> {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.push(&quoted);
        self.block_break();
    }

    fn write_link(&mut self, id: NodeId) {
        let href = self.attr(id, "href");
        let text_empty = self.doc.text_content(id).trim().is_empty();

        let Some(href) = href.filter(|h| self.options.include_links && !h.trim().is_empty()) else {
            self.write_children(id);
            return;
        };
        // Fragment-only and javascript: links go nowhere useful for a reader.
        let trimmed = href.trim();
        if trimmed.starts_with('#') || trimmed.to_ascii_lowercase().starts_with("javascript:") {
            self.write_children(id);
            return;
        }
        // An image-only link would otherwise render as [![alt](src)](href),
        // which is noise; keep the image alone.
        if text_empty {
            self.write_children(id);
            return;
        }

        self.push("[");
        self.write_children(id);
        self.push("](");
        self.push(&escape_url(&self.resolve(trimmed)));
        self.push(")");
    }

    fn write_image(&mut self, id: NodeId) {
        if !self.options.include_images {
            return;
        }
        // Lazy-loaded images keep the real URL in a data attribute and leave
        // src as a placeholder, so check those first.
        let src = ["src", "data-src", "data-original", "data-lazy-src"]
            .iter()
            .find_map(|a| self.attr(id, a))
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                // srcset: take the first candidate.
                self.attr(id, "srcset").and_then(|set| {
                    set.split(',')
                        .next()
                        .and_then(|c| c.split_whitespace().next())
                        .map(str::to_owned)
                })
            });
        let Some(src) = src else { return };
        // Tracking pixels and inline placeholders are not worth the tokens.
        if src.starts_with("data:") {
            return;
        }
        let alt = self.attr(id, "alt").unwrap_or_default();
        self.push("![");
        self.push(&escape_inline(&collapse_whitespace(&alt)));
        self.push("](");
        self.push(&escape_url(&self.resolve(src.trim())));
        self.push(")");
    }

    fn write_list(&mut self, id: NodeId, ordered: bool) {
        self.block_break();
        self.list_stack.push(ListFrame { ordered, index: 0 });
        self.write_children(id);
        self.list_stack.pop();
        self.block_break();
    }

    fn write_list_item(&mut self, id: NodeId) {
        let depth = self.list_stack.len().saturating_sub(1);
        let marker = match self.list_stack.last_mut() {
            Some(frame) => {
                frame.index += 1;
                if frame.ordered {
                    format!("{}. ", frame.index)
                } else {
                    "- ".to_owned()
                }
            }
            // A stray <li> outside any list still deserves a bullet.
            None => "- ".to_owned(),
        };

        self.line_break();
        let indent = "  ".repeat(depth);
        self.push(&indent);
        self.push(&marker);

        // Render the item, then indent its continuation lines so nested blocks
        // stay inside the bullet.
        let start = self.out.len();
        self.write_children(id);
        let inner = self.out.split_off(start);
        let continuation = format!("{indent}  ");
        let body = inner
            .trim_matches('\n')
            .lines()
            .enumerate()
            .map(|(i, line)| {
                if i == 0 || line.is_empty() {
                    line.to_owned()
                } else {
                    format!("{continuation}{line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.push(&body);
        self.line_break();
    }

    fn write_table(&mut self, id: NodeId) {
        // Collect the grid first: a Markdown table needs its column count
        // before it can write the separator row.
        let mut rows: Vec<Vec<String>> = Vec::new();
        for row in self.doc.descendants(id).filter(|&d| {
            self.doc
                .element(d)
                .is_some_and(|e| e.local_name().as_ref() == "tr")
        }) {
            let cells: Vec<String> = self
                .doc
                .children(row)
                .filter(|&c| {
                    self.doc
                        .element(c)
                        .is_some_and(|e| matches!(e.local_name().as_ref(), "td" | "th"))
                })
                .map(|c| {
                    // Pipes and newlines would break the row.
                    collapse_whitespace(&self.doc.text_content(c))
                        .replace('|', "\\|")
                        .trim()
                        .to_owned()
                })
                .collect();
            if !cells.is_empty() {
                rows.push(cells);
            }
        }

        if rows.is_empty() {
            return;
        }
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        // A one-column table is a layout wrapper far more often than data.
        if columns < 2 {
            self.block_break();
            for row in rows {
                self.push(&row.join(" "));
                self.line_break();
            }
            self.block_break();
            return;
        }

        self.block_break();
        for (i, row) in rows.iter().enumerate() {
            let mut cells = row.clone();
            cells.resize(columns, String::new());
            self.push("| ");
            self.push(&cells.join(" | "));
            self.push(" |");
            self.line_break();
            if i == 0 {
                self.push("| ");
                self.push(&vec!["---"; columns].join(" | "));
                self.push(" |");
                self.line_break();
            }
        }
        self.block_break();
    }
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// Escape characters that would otherwise be read as Markdown syntax.
///
/// Deliberately conservative: over-escaping prose is uglier and costs more
/// tokens than the rare mis-render it prevents.
fn escape_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_line_start = true;
    for c in text.chars() {
        match c {
            '\\' | '`' | '*' | '_' | '[' | ']' => {
                out.push('\\');
                out.push(c);
            }
            // These only mean something at the start of a line.
            '#' | '>' | '-' | '+' if at_line_start => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
        at_line_start = c == '\n';
    }
    out
}

fn escape_url(url: &str) -> String {
    // Spaces and parentheses end the URL part of a Markdown link.
    url.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for c in text.chars() {
        if c == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

/// Collapse runs of blank lines and trim the ends.
fn normalize_blank_lines(text: &mut String) {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run > 1 || out.is_empty() {
                continue;
            }
            out.push('\n');
        } else {
            blank_run = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    *text = out.trim_end().to_owned();
    text.push('\n');
}
