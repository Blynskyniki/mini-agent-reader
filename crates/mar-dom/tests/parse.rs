use mar_dom::{NodeData, parse_html};

#[test]
fn builds_a_tree_and_merges_text() {
    let out = parse_html(
        r#"<!doctype html><html><head><title>Hi</title></head>
           <body><div id="a" class="x y">one<span>two</span>three</div>
           <p>tail<p>unclosed</body></html>"#,
    );
    let doc = out.document;

    let body = doc.body().expect("body");
    let div = doc.children(body).find(|&c| doc.data(c).is_element()).unwrap();
    let el = doc.element(div).unwrap();
    assert_eq!(el.local_name().as_ref(), "div");
    assert_eq!(el.attr(&"id".into()), Some("a"));
    assert_eq!(doc.text_content(div), "onetwothree");

    // "one" and "three" are separate tokens but must not become 2 nodes each.
    let texts = doc
        .children(div)
        .filter(|&c| matches!(doc.data(c), NodeData::Text(_)))
        .count();
    assert_eq!(texts, 2, "adjacent text tokens should merge");

    // The parser must recover from the unclosed <p>.
    let ps = doc
        .descendants(body)
        .filter(|&c| doc.element(c).is_some_and(|e| e.local_name().as_ref() == "p"))
        .count();
    assert_eq!(ps, 2);
    assert!(doc.head().is_some());
}

#[test]
fn detach_and_reparent_keep_links_consistent() {
    let out = parse_html("<div><a></a><b></b><i></i></div>");
    let mut doc = out.document;
    let body = doc.body().unwrap();
    let div = doc.children(body).next().unwrap();
    let kids: Vec<_> = doc.children(div).collect();
    assert_eq!(kids.len(), 3);

    doc.detach(kids[1]);
    let after: Vec<_> = doc.children(div).collect();
    assert_eq!(after, vec![kids[0], kids[2]]);
    assert_eq!(doc.node(kids[0]).next_sibling, Some(kids[2]));
    assert_eq!(doc.node(kids[2]).prev_sibling, Some(kids[0]));
    assert_eq!(doc.node(div).last_child, Some(kids[2]));

    doc.insert_before(kids[0], kids[1]);
    let after: Vec<_> = doc.children(div).collect();
    assert_eq!(after, vec![kids[1], kids[0], kids[2]]);
    assert_eq!(doc.node(div).first_child, Some(kids[1]));
}

#[test]
fn descendants_visits_preorder_without_escaping_the_root() {
    let out = parse_html("<div id=r><a><b></b></a><c></c></div><aside></aside>");
    let doc = out.document;
    let body = doc.body().unwrap();
    let root = doc.children(body).next().unwrap();
    let names: Vec<_> = doc
        .descendants(root)
        .filter_map(|n| doc.element(n).map(|e| e.local_name().to_string()))
        .collect();
    assert_eq!(names, vec!["a", "b", "c"], "must not walk into <aside>");
}

#[test]
fn serialization_round_trips_and_escapes() {
    let src = r#"<!DOCTYPE html><html><head></head><body><div class="a&b"><p>x &lt; y</p><br><img src="/i.png"><script>if (a<b) {}</script></div></body></html>"#;
    let out = mar_dom::parse_html(src);
    let doc = &out.document;
    let html = mar_dom::document_html(doc);

    assert!(html.contains(r#"class="a&amp;b""#), "attrs are escaped: {html}");
    assert!(html.contains("x &lt; y"), "text is escaped: {html}");
    assert!(html.contains("<br>") && !html.contains("</br>"), "void tags: {html}");
    assert!(html.contains("if (a<b) {}"), "script content stays raw: {html}");

    // Re-parsing the output must produce the same tree shape.
    let again = mar_dom::parse_html(&html);
    assert_eq!(mar_dom::document_html(&again.document), html);

    let div = mar_dom::query_selector(doc, doc.root(), "div").unwrap().unwrap();
    assert!(mar_dom::outer_html(doc, div).starts_with("<div "));
    assert!(mar_dom::inner_html(doc, div).starts_with("<p>"));
}

#[test]
fn fragments_import_into_a_live_document() {
    use mar_dom::{QualName, ns};
    let out = mar_dom::parse_html("<div id=host><span>old</span></div>");
    let mut doc = out.document;
    let host = mar_dom::query_selector(&doc, doc.root(), "#host").unwrap().unwrap();

    let ctx = QualName::new(None, ns!(html), "div".into());
    let (frag, holder) = mar_dom::parse_fragment_document("<b>x</b><i>y</i>", &ctx);
    let kids: Vec<_> = frag.children(holder).collect();
    assert_eq!(kids.len(), 2);

    // Replace the host's children with the imported fragment.
    while let Some(c) = doc.node(host).first_child {
        doc.detach(c);
    }
    for k in kids {
        let copy = doc.import_subtree(&frag, k);
        doc.append(host, copy);
    }
    assert_eq!(mar_dom::inner_html(&doc, host), "<b>x</b><i>y</i>");

    // cloneNode(true) must copy the subtree, not alias it.
    let clone = doc.clone_node(host, true);
    assert_eq!(mar_dom::inner_html(&doc, clone), "<b>x</b><i>y</i>");
    assert!(!doc.contains(host, clone));
    doc.set_text_content(host, "gone");
    assert_eq!(mar_dom::inner_html(&doc, clone), "<b>x</b><i>y</i>");
}
