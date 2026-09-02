use mar_extract::{MarkdownOptions, read, to_markdown};
use url::Url;

fn options() -> MarkdownOptions {
    MarkdownOptions {
        base_url: Some(Url::parse("https://news.example.com/2026/09/story").unwrap()),
        ..MarkdownOptions::default()
    }
}

/// A page shaped like a real article: chrome around a single content column.
const ARTICLE: &str = r##"<!doctype html><html lang="en"><head>
<title>Reactor milestone reached — Example News</title>
<link rel="canonical" href="/2026/09/story">
<link rel="alternate" type="application/rss+xml" title="Feed" href="/feed.xml">
<meta property="og:site_name" content="Example News">
<meta name="author" content="Dana Okafor">
<meta property="article:published_time" content="2026-09-01T08:00:00Z">
<meta name="description" content="Engineers confirmed the milestone on Tuesday.">
<meta property="og:image" content="/img/reactor.jpg">
<script type="application/ld+json">
{"@context":"https://schema.org","@type":"NewsArticle",
 "headline":"Reactor milestone reached",
 "author":{"@type":"Person","name":"Dana Okafor"},
 "datePublished":"2026-09-01T08:00:00Z",
 "publisher":{"@type":"Organization","name":"Example News"}}
</script>
</head><body>
<header class="site-header"><nav class="main-nav">
  <a href="/">Home</a> <a href="/world">World</a> <a href="/tech">Tech</a>
  <a href="/sport">Sport</a> <a href="/culture">Culture</a> <a href="/login">Sign in</a>
</nav></header>
<div id="wrapper">
  <article class="post-content">
    <h1>Reactor milestone reached</h1>
    <p class="byline">By <a href="/authors/dana">Dana Okafor</a></p>
    <p>Engineers at the facility confirmed on Tuesday that the reactor had sustained a
       reaction for more than four hours, a duration that comfortably exceeds the previous
       record and clears the threshold the programme set for itself three years ago.</p>
    <p>The result matters because sustained operation, rather than peak output, is what
       determines whether the design can be scaled. Earlier attempts reached higher
       temperatures but could not hold them, and the team has spent two years on the
       cooling loop that made the difference here.</p>
    <h2>What happens next</h2>
    <ul><li>A full review of the instrumentation data, expected within a month.</li>
        <li>An independent audit before the results are published.</li></ul>
    <blockquote><p>We were not expecting it to hold this long, said the lead engineer.</p></blockquote>
    <pre><code class="language-python">def efficiency(output, input):
    return output / input</code></pre>
    <table><tr><th>Run</th><th>Duration</th></tr>
           <tr><td>Previous</td><td>90 min</td></tr>
           <tr><td>Latest</td><td>247 min</td></tr></table>
    <figure><img src="/img/reactor.jpg" alt="The reactor hall"><figcaption>The hall in 2025</figcaption></figure>
    <p>Funding for the next phase has not been confirmed, and the programme's budget runs
       out at the end of the financial year, which leaves the team in the familiar position
       of celebrating a result while waiting to learn whether the work continues.</p>
  </article>
  <aside class="sidebar related"><h3>Related</h3>
    <ul><li><a href="/a">Fusion timeline</a></li><li><a href="/b">Grid costs</a></li>
        <li><a href="/c">Reactor designs</a></li></ul>
  </aside>
</div>
<footer class="site-footer"><p>© 2026 Example News</p>
  <nav><a href="/about">About</a> <a href="/privacy">Privacy</a> <a href="/terms">Terms</a></nav>
</footer>
</body></html>"##;

#[test]
fn metadata_comes_from_json_ld_and_meta_tags() {
    let doc = mar_dom::parse_html(ARTICLE).document;
    let out = read(&doc, &options());
    let m = &out.metadata;

    // The JSON-LD headline beats the <title>, which carries the site suffix.
    assert_eq!(m.title.as_deref(), Some("Reactor milestone reached"));
    assert_eq!(m.author.as_deref(), Some("Dana Okafor"));
    assert_eq!(m.site_name.as_deref(), Some("Example News"));
    assert_eq!(m.published.as_deref(), Some("2026-09-01T08:00:00Z"));
    assert_eq!(m.language.as_deref(), Some("en"));
    assert_eq!(
        m.description.as_deref(),
        Some("Engineers confirmed the milestone on Tuesday.")
    );
    // Relative URLs are resolved against the page.
    assert_eq!(
        m.canonical_url.as_deref(),
        Some("https://news.example.com/2026/09/story")
    );
    assert_eq!(
        m.image.as_deref(),
        Some("https://news.example.com/img/reactor.jpg")
    );
    assert_eq!(m.feeds.len(), 1);
    assert_eq!(m.feeds[0].url, "https://news.example.com/feed.xml");
    assert!(m.schema_types.contains(&"NewsArticle".to_owned()));
}

#[test]
fn the_article_is_kept_and_the_chrome_is_dropped() {
    let doc = mar_dom::parse_html(ARTICLE).document;
    let out = read(&doc, &options());

    assert!(!out.low_confidence, "a clear article should score");
    assert!(out.content.contains("sustained a reaction"));
    assert!(out.content.contains("Funding for the next phase"));

    for chrome in [
        "Sign in",
        "Privacy",
        "© 2026",
        "Fusion timeline",
        "Grid costs",
    ] {
        assert!(
            !out.content.contains(chrome),
            "chrome leaked into the article: {chrome}\n---\n{}",
            out.content
        );
    }
}

#[test]
fn structure_survives_the_conversion_to_markdown() {
    let doc = mar_dom::parse_html(ARTICLE).document;
    let out = read(&doc, &options());
    let md = &out.content;

    assert!(md.contains("# Reactor milestone reached"), "h1\n{md}");
    assert!(md.contains("## What happens next"), "h2\n{md}");
    assert!(md.contains("- A full review"), "list\n{md}");
    assert!(md.contains("> We were not expecting"), "quote\n{md}");
    assert!(md.contains("```python"), "code fence with language\n{md}");
    assert!(md.contains("return output / input"), "code body\n{md}");
    assert!(md.contains("| Run | Duration |"), "table header\n{md}");
    assert!(md.contains("| --- | --- |"), "table separator\n{md}");
    assert!(md.contains("| Latest | 247 min |"), "table row\n{md}");
    assert!(
        md.contains("![The reactor hall](https://news.example.com/img/reactor.jpg)"),
        "image with resolved URL\n{md}"
    );
    assert!(md.contains("*The hall in 2025*"), "caption\n{md}");
    // Links resolve too.
    assert!(
        md.contains("[Dana Okafor](https://news.example.com/authors/dana)"),
        "link\n{md}"
    );
    // No raw HTML survives.
    assert!(
        !md.contains("<p>") && !md.contains("<div"),
        "html leaked\n{md}"
    );
}

#[test]
fn markdown_options_control_cost() {
    let doc = mar_dom::parse_html(ARTICLE).document;
    let article = mar_extract::readability::extract(&doc);

    let lean = MarkdownOptions {
        include_images: false,
        include_links: false,
        base_url: options().base_url,
        max_chars: None,
    };
    let md = to_markdown(&article.document, article.root, &lean);
    assert!(!md.contains("!["), "images suppressed\n{md}");
    assert!(!md.contains("](http"), "links suppressed\n{md}");
    // The link text is still there; only the URL is gone.
    assert!(md.contains("Dana Okafor"));

    let capped = MarkdownOptions {
        max_chars: Some(200),
        ..options()
    };
    let short = to_markdown(&article.document, article.root, &capped);
    assert!(
        short.len() < 400,
        "truncated to roughly the cap: {}",
        short.len()
    );
}

#[test]
fn a_page_with_no_article_falls_back_to_the_body() {
    // A link farm: nothing here is prose.
    let doc = mar_dom::parse_html(
        r#"<body><nav><a href="/1">One</a><a href="/2">Two</a></nav>
           <div><a href="/3">Three</a><a href="/4">Four</a></div></body>"#,
    )
    .document;
    let out = read(&doc, &MarkdownOptions::default());
    assert!(
        out.low_confidence,
        "the caller must be told this is unreliable"
    );
    assert!(out.content.contains("One"));
}

#[test]
fn markdown_escapes_what_would_otherwise_be_syntax() {
    let doc = mar_dom::parse_html(
        r#"<body><article><p>A price of 5*3 and an _underscore_ plus [brackets] and a `tick`.</p>
           <p>Some more prose here so the block scores as an article rather than being
              discarded as a stray caption or a piece of page furniture.</p>
           <p>And a second paragraph of similar length to make the candidate score
              comfortably above the minimum the extractor insists on.</p></article></body>"#,
    )
    .document;
    let out = read(&doc, &MarkdownOptions::default());
    assert!(
        out.content.contains(r"5\*3"),
        "asterisk escaped\n{}",
        out.content
    );
    assert!(
        out.content.contains(r"\_underscore\_"),
        "underscore escaped\n{}",
        out.content
    );
    assert!(
        out.content.contains(r"\[brackets\]"),
        "brackets escaped\n{}",
        out.content
    );
}

#[test]
fn nested_lists_indent_correctly() {
    let doc = mar_dom::parse_html(
        r#"<body><article>
        <p>An introduction long enough that the extractor treats this block as content
           rather than discarding it as a fragment of page furniture somewhere.</p>
        <ol><li>First
              <ul><li>Nested a</li><li>Nested b</li></ul></li>
            <li>Second</li></ol>
        <p>A closing paragraph, also long enough to count toward the score that this
           candidate needs in order to be chosen over the surrounding markup.</p>
        </article></body>"#,
    )
    .document;
    let out = read(&doc, &MarkdownOptions::default());
    let md = &out.content;
    assert!(md.contains("1. First"), "{md}");
    assert!(md.contains("  - Nested a"), "nested indent\n{md}");
    assert!(md.contains("2. Second"), "ordered counter continues\n{md}");
}
