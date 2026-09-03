//! ES modules, fetched over the same seam as everything else.
//!
//! A `type="module"` script compiles and runs without this, but an `import` of
//! another URL has nowhere to go. That is not a niche gap: an application built
//! by Vite, Angular or anything else shipping native modules is a shell plus a
//! graph of imports, and without the graph the shell renders nothing.
//!
//! QuickJS asks for a module synchronously, in the middle of evaluating the one
//! that imported it. That suits this engine exactly — the settle loop is
//! single-threaded and the network provider blocks, so a fetch here is simply
//! "the virtual clock does not advance while the network works".

use crate::net::{HttpRequest, NetworkProvider};
use crate::state::Shared;
use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::{Ctx, Error, Module, Result, module::Declared};
use std::rc::Rc;
use url::Url;

/// Resolves a specifier against the URL of the module doing the importing.
pub struct UrlResolver {
    page_url: Url,
}

impl UrlResolver {
    pub fn new(page_url: Url) -> Self {
        UrlResolver { page_url }
    }
}

impl Resolver for UrlResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
        // A module's name is its URL. The first one is named for the document
        // when it was written inline, and everything resolves from there.
        let base = Url::parse(base).unwrap_or_else(|_| self.page_url.clone());

        // A bare specifier needs an import map or a bundler, and neither is
        // here. Say so, rather than resolving it into a URL that will 404.
        let looks_bare =
            !name.starts_with('.') && !name.starts_with('/') && Url::parse(name).is_err();
        if looks_bare {
            return Err(Error::new_resolving_message(
                base.to_string(),
                name.to_string(),
                "bare specifiers need an import map, which this engine does not have",
            ));
        }

        base.join(name).map(|u| u.to_string()).map_err(|e| {
            Error::new_resolving_message(base.to_string(), name.to_string(), e.to_string())
        })
    }
}

/// Fetches a resolved module URL and hands QuickJS its source.
pub struct UrlLoader<N: NetworkProvider + 'static> {
    net: Rc<N>,
    state: Shared,
    page_url: Url,
    /// Modules fetched so far, against the page's request budget.
    fetched: usize,
}

impl<N: NetworkProvider + 'static> UrlLoader<N> {
    pub fn new(net: Rc<N>, state: Shared, page_url: Url) -> Self {
        UrlLoader {
            net,
            state,
            page_url,
            fetched: 0,
        }
    }
}

impl<N: NetworkProvider + 'static> Loader for UrlLoader<N> {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        let failed = |reason: String| Error::new_loading_message(name.to_string(), reason);

        // An import graph is a way to make a page fetch forever. It draws on
        // the same budget as every other subresource, so a page cannot spend
        // more by using imports than it could by using `fetch`.
        {
            let mut state = self.state.borrow_mut();
            let limit = state.limits.max_requests;
            if state.request_count >= limit || self.fetched >= limit {
                return Err(failed(format!("module budget of {limit} requests spent")));
            }
            // A graph fetched one blocking request at a time is the easiest way
            // for a page to run past its wall-clock budget, because the budget
            // is otherwise only checked between callbacks and this happens in
            // the middle of one.
            if state.out_of_time() {
                return Err(failed("the page ran out of time".to_owned()));
            }
            state.request_count += 1;
        }
        self.fetched += 1;

        let response = self
            .net
            .fetch(HttpRequest {
                method: "GET".to_owned(),
                url: name.to_owned(),
                headers: vec![("Referer".to_owned(), self.page_url.as_str().to_owned())],
                body: None,
            })
            .map_err(failed)?;

        if !(200..300).contains(&response.status) {
            return Err(failed(format!(
                "{} {}",
                response.status, response.status_text
            )));
        }

        // A module served as HTML is a soft 404 — a server answering a missing
        // path with its index page. Compiling it produces a wall of syntax
        // errors that says nothing about the real problem.
        if response
            .header("content-type")
            .is_some_and(|t| t.trim_start().starts_with("text/html"))
        {
            return Err(failed("served as HTML, not JavaScript".to_owned()));
        }

        // The graph one level down, started now rather than when QuickJS gets
        // round to asking. An import graph is discovered by parsing each module
        // in turn, so without this a page of a hundred chunks pays a hundred
        // round trips end to end — which on a news site is most of the render.
        let base = Url::parse(name).unwrap_or_else(|_| self.page_url.clone());
        let next: Vec<String> = import_specifiers(&response.body)
            .into_iter()
            .filter_map(|spec| base.join(&spec).ok())
            .map(|u| u.to_string())
            .collect();
        if !next.is_empty() {
            self.net.prefetch(next);
        }

        Module::declare(ctx.clone(), name.to_owned(), response.body)
    }
}

/// The URLs a module statically imports, found by reading its source.
///
/// Deliberately a scan and not a parse: this only decides what to fetch early.
/// A specifier missed costs nothing — the loader fetches it the slow way — and
/// one imagined inside a comment or a string costs a single wasted request. A
/// real parse would cost more than it saves.
fn import_specifiers(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;
    while let Some(found) = source[i..].find("import") {
        let at = i + found;
        i = at + 6;
        // A word boundary before, so `reimport` and `x.import` do not count.
        if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || matches!(bytes[at - 1], b'_' | b'$' | b'.')) {
            continue;
        }
        // Take the first quoted string on the rest of the line, or the next
        // one after a `from`. Both forms end at the closing quote.
        let rest = &source[i..source.len().min(i + 512)];
        let Some(quote_at) = rest.find(['"', '\'', '`']) else {
            continue;
        };
        // Anything but whitespace, an identifier, braces, a star, a comma, a
        // parenthesis or `from` between `import` and the quote means this is
        // not an import statement.
        if rest[..quote_at].contains(|c: char| {
            !(c.is_alphanumeric() || " \t\r\n{},*()_$".contains(c))
        }) {
            continue;
        }
        let quote = rest.as_bytes()[quote_at] as char;
        let after = &rest[quote_at + 1..];
        let Some(end) = after.find(quote) else { continue };
        let spec = &after[..end];
        // Only a URL the loader could resolve: relative or absolute paths.
        if spec.starts_with('.') || spec.starts_with('/') || spec.starts_with("http") {
            out.push(spec.to_owned());
        }
    }
    out.truncate(MAX_SPECULATIVE_IMPORTS);
    out
}

/// A ceiling on how far ahead one module may reach. A barrel file re-exporting
/// a thousand things should not fetch a thousand of them speculatively.
const MAX_SPECULATIVE_IMPORTS: usize = 24;

#[cfg(test)]
mod tests {
    use super::import_specifiers;

    #[test]
    fn specifiers_are_found_in_the_forms_bundlers_emit() {
        let found = import_specifiers(
            r#"
            import a from "./a.js";
            import {b, c} from '../b/c.js';
            import * as d from "/abs/d.js";
            import "./side-effect.js";
            export {e} from "./e.js";
            const f = await import("./lazy.js");
            import lodash from "lodash";
            const notAnImport = "./decoy.js";
            "#,
        );
        assert!(found.contains(&"./a.js".to_owned()));
        assert!(found.contains(&"../b/c.js".to_owned()));
        assert!(found.contains(&"/abs/d.js".to_owned()));
        assert!(found.contains(&"./side-effect.js".to_owned()));
        assert!(found.contains(&"./lazy.js".to_owned()));
        assert!(
            !found.contains(&"lodash".to_owned()),
            "a bare specifier has nothing to resolve against"
        );
        assert!(
            !found.contains(&"./decoy.js".to_owned()),
            "a string that is not an import is not one"
        );
    }
}
