use mar_dom::{Matcher, parse_html, query_selector, query_selector_all};

fn names(html: &str, sel: &str) -> Vec<String> {
    let out = parse_html(html);
    let doc = &out.document;
    query_selector_all(doc, doc.root(), sel)
        .unwrap()
        .into_iter()
        .map(|id| {
            let e = doc.element(id).unwrap();
            match e.attr(&"id".into()) {
                Some(v) => format!("{}#{v}", e.local_name()),
                None => e.local_name().to_string(),
            }
        })
        .collect()
}

const PAGE: &str = r#"
<article class="post">
  <h1 id="t">Title</h1>
  <p id="p1" class="lead intro">First</p>
  <p id="p2">Second <a id="a1" href="/x" rel="next">link</a></p>
  <p id="p3" data-role="note">Third</p>
  <ul id="list"><li id="l1">a</li><li id="l2">b</li><li id="l3">c</li></ul>
  <figure id="fig"><img id="im" src="/i.png" alt="pic"></figure>
  <div id="empty"></div>
</article>"#;

#[test]
fn combinators_and_classes() {
    assert_eq!(names(PAGE, "article > p"), ["p#p1", "p#p2", "p#p3"]);
    assert_eq!(names(PAGE, ".lead.intro"), ["p#p1"]);
    assert_eq!(names(PAGE, "#p2 a"), ["a#a1"]);
    assert_eq!(names(PAGE, "#p1 + p"), ["p#p2"]);
    assert_eq!(names(PAGE, "#p1 ~ p"), ["p#p2", "p#p3"]);
}

#[test]
fn attribute_operators() {
    assert_eq!(names(PAGE, "[data-role]"), ["p#p3"]);
    assert_eq!(names(PAGE, "[rel=next]"), ["a#a1"]);
    assert_eq!(names(PAGE, "a[href^='/']"), ["a#a1"]);
    assert_eq!(names(PAGE, "img[src$='.png']"), ["img#im"]);
    assert_eq!(names(PAGE, "[class*='ntr']"), ["p#p1"]);
    assert!(
        names(PAGE, "[href='/X']").is_empty(),
        "value match is case-sensitive"
    );
}

#[test]
fn structural_and_functional_pseudo_classes() {
    assert_eq!(names(PAGE, "li:nth-child(2)"), ["li#l2"]);
    assert_eq!(names(PAGE, "li:nth-child(odd)"), ["li#l1", "li#l3"]);
    assert_eq!(names(PAGE, "li:last-child"), ["li#l3"]);
    assert_eq!(names(PAGE, "#list li:not(#l2)"), ["li#l1", "li#l3"]);
    assert_eq!(names(PAGE, ":is(h1, figure)"), ["h1#t", "figure#fig"]);
    assert_eq!(names(PAGE, "div:empty"), ["div#empty"]);
    // :has needs the matcher to look forward into descendants.
    assert_eq!(names(PAGE, "p:has(a)"), ["p#p2"]);
    assert_eq!(names(PAGE, "article:has(> ul)"), ["article"]);
}

#[test]
fn link_and_form_state_pseudo_classes() {
    let html = r#"<a id=l1 href=/a>x</a><a id=l2>y</a>
        <input id=i1 disabled><input id=i2><input id=i3 required>"#;
    assert_eq!(names(html, "a:any-link"), ["a#l1"]);
    assert_eq!(names(html, "input:disabled"), ["input#i1"]);
    assert_eq!(names(html, "input:enabled"), ["input#i2", "input#i3"]);
    assert_eq!(names(html, "input:required"), ["input#i3"]);
    // Interaction state never matches: there is no user.
    assert!(names(html, "a:hover").is_empty());
}

#[test]
fn scoped_queries_and_closest() {
    let out = parse_html(PAGE);
    let doc = &out.document;
    let list = query_selector(doc, doc.root(), "#list").unwrap().unwrap();
    // Scoped to #list, so <li> outside it would not be seen.
    assert_eq!(query_selector_all(doc, list, "li").unwrap().len(), 3);

    let a = query_selector(doc, doc.root(), "#a1").unwrap().unwrap();
    let m = Matcher::new("article").unwrap();
    let closest = m.closest(doc, a).unwrap();
    assert_eq!(
        doc.element(closest).unwrap().local_name().as_ref(),
        "article"
    );
    assert!(Matcher::new("h1").unwrap().closest(doc, a).is_none());
}

#[test]
fn unknown_pseudo_class_parses_but_never_matches() {
    // A selector list with one unsupported piece must not fail the query.
    assert!(names(PAGE, "p:state(custom)").is_empty());
    assert_eq!(names(PAGE, "h1, p:defined:nth-child(2)"), ["h1#t", "p#p1"]);
}

#[test]
fn invalid_selector_is_an_error_not_a_panic() {
    let out = parse_html(PAGE);
    let err = query_selector(&out.document, out.document.root(), "p >>> a").unwrap_err();
    assert!(err.to_string().contains("invalid CSS selector"));
}
