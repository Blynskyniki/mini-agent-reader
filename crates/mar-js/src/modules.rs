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

/// An import map, as a page declares one in `<script type="importmap">`.
///
/// A bare specifier — `import x from '@wordpress/interactivity'` — has nothing
/// to resolve against on its own. The map is what gives it a URL, and a site
/// built on WordPress's Interactivity API, or on any framework that ships an
/// unbundled module graph, ships one. Without it every module that names a
/// bare specifier fails to load, and with it the specifier is just a URL.
#[derive(Debug, Clone, Default)]
pub struct ImportMap {
    /// Longest keys first, so a prefix match takes the most specific entry.
    imports: Vec<(String, String)>,
    /// Scope URL prefix and its own entries, longest prefix first.
    scopes: Vec<(String, Vec<(String, String)>)>,
}

impl ImportMap {
    /// Parse the JSON of one import map, resolving its entries against `base`.
    ///
    /// Lenient on purpose: an entry that does not parse is dropped rather than
    /// failing the map, which is what a browser does with a bad entry too.
    pub fn parse(json: &str, base: &Url) -> Option<ImportMap> {
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        let object = value.as_object()?;
        let mut map = ImportMap::default();
        if let Some(imports) = object.get("imports").and_then(|v| v.as_object()) {
            map.imports = Self::entries(imports, base);
        }
        if let Some(scopes) = object.get("scopes").and_then(|v| v.as_object()) {
            for (scope, entries) in scopes {
                let Some(entries) = entries.as_object() else {
                    continue;
                };
                let Ok(scope_url) = base.join(scope) else {
                    continue;
                };
                map.scopes
                    .push((scope_url.to_string(), Self::entries(entries, base)));
            }
            map.scopes
                .sort_by_key(|(scope, _)| std::cmp::Reverse(scope.len()));
        }
        Some(map)
    }

    /// Merge another map in, as the spec does when a page carries several.
    pub fn merge(&mut self, other: ImportMap) {
        for (key, value) in other.imports {
            if !self.imports.iter().any(|(k, _)| *k == key) {
                self.imports.push((key, value));
            }
        }
        self.imports
            .sort_by_key(|(key, _)| std::cmp::Reverse(key.len()));
        self.scopes.extend(other.scopes);
        self.scopes
            .sort_by_key(|(scope, _)| std::cmp::Reverse(scope.len()));
    }

    pub fn is_empty(&self) -> bool {
        self.imports.is_empty() && self.scopes.is_empty()
    }

    fn entries(
        object: &serde_json::Map<String, serde_json::Value>,
        base: &Url,
    ) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = object
            .iter()
            .filter_map(|(key, value)| {
                let value = value.as_str()?;
                let key = Self::normalise_key(key, base);
                let target = base.join(value).ok()?.to_string();
                // A prefix key maps to a prefix target; anything else is a
                // mismatch the spec rejects.
                if key.ends_with('/') != target.ends_with('/') && key.ends_with('/') {
                    return None;
                }
                Some((key, target))
            })
            .collect();
        out.sort_by_key(|(key, _)| std::cmp::Reverse(key.len()));
        out
    }

    /// A key that is itself a URL, or relative to one, is stored resolved so
    /// that `import '/js/app.js'` and `import './js/app.js'` meet the same
    /// entry. A bare key stays as written.
    fn normalise_key(key: &str, base: &Url) -> String {
        if key.starts_with('/')
            || key.starts_with("./")
            || key.starts_with("../")
            || Url::parse(key).is_ok()
        {
            base.join(key)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| key.to_owned())
        } else {
            key.to_owned()
        }
    }

    fn lookup(entries: &[(String, String)], specifier: &str) -> Option<String> {
        for (key, target) in entries {
            if key == specifier {
                return Some(target.clone());
            }
            if key.ends_with('/')
                && let Some(rest) = specifier.strip_prefix(key.as_str())
            {
                return Some(format!("{target}{rest}"));
            }
        }
        None
    }

    /// The URL `specifier` maps to when imported from `referrer`, if any.
    pub fn resolve(&self, specifier: &str, referrer: &Url) -> Option<String> {
        let normalised = Self::normalise_key(specifier, referrer);
        let referrer = referrer.as_str();
        for (scope, entries) in &self.scopes {
            if referrer.starts_with(scope.as_str())
                && let Some(found) = Self::lookup(entries, &normalised)
            {
                return Some(found);
            }
        }
        Self::lookup(&self.imports, &normalised)
    }
}

/// Resolves a specifier against the URL of the module doing the importing.
pub struct UrlResolver {
    page_url: Url,
    import_map: ImportMap,
}

impl UrlResolver {
    pub fn new(page_url: Url) -> Self {
        UrlResolver {
            page_url,
            import_map: ImportMap::default(),
        }
    }

    pub fn with_import_map(page_url: Url, import_map: ImportMap) -> Self {
        UrlResolver {
            page_url,
            import_map,
        }
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

        // The page's import map speaks first, for bare specifiers and for
        // URLs alike: a map may redirect `/js/app.js` as well as name `vue`.
        if let Some(mapped) = self.import_map.resolve(name, &base) {
            return Ok(mapped);
        }

        // A bare specifier needs an import map, and this page has none that
        // names it. Say so, rather than resolving it into a URL that will 404.
        let looks_bare =
            !name.starts_with('.') && !name.starts_with('/') && Url::parse(name).is_err();
        if looks_bare {
            return Err(Error::new_resolving_message(
                base.to_string(),
                name.to_string(),
                "a bare specifier, and the page's import map does not name it",
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

        let module = Module::declare(ctx.clone(), name.to_owned(), response.body)?;
        set_import_meta(&module, name);
        Ok(module)
    }
}

/// Give a module the `import.meta.url` it would have in a browser.
///
/// Vite, Stencil and anything else that locates its own assets writes
/// `new URL('.', import.meta.url)`, and with `url` undefined that throws
/// before the application has drawn anything.
pub fn set_import_meta(module: &Module<'_, Declared>, url: &str) {
    if let Ok(meta) = module.meta() {
        let _ = meta.set("url", url);
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
        if at > 0
            && (bytes[at - 1].is_ascii_alphanumeric()
                || matches!(bytes[at - 1], b'_' | b'$' | b'.'))
        {
            continue;
        }
        // Take the first quoted string on the rest of the line, or the next
        // one after a `from`. Both forms end at the closing quote.
        // The window has to end on a character boundary: a module with a CJK
        // string literal past an `import` is otherwise a panic, and with
        // panics fatal that is one page taking a whole batch with it.
        let mut end = source.len().min(i + 512);
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        let rest = &source[i..end];
        let Some(quote_at) = rest.find(['"', '\'', '`']) else {
            continue;
        };
        // Anything but whitespace, an identifier, braces, a star, a comma, a
        // parenthesis or `from` between `import` and the quote means this is
        // not an import statement.
        if rest[..quote_at]
            .contains(|c: char| !(c.is_alphanumeric() || " \t\r\n{},*()_$".contains(c)))
        {
            continue;
        }
        let quote = rest.as_bytes()[quote_at] as char;
        let after = &rest[quote_at + 1..];
        let Some(end) = after.find(quote) else {
            continue;
        };
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
    use super::{ImportMap, import_specifiers};
    use url::Url;

    #[test]
    fn an_import_map_names_bare_specifiers_and_prefixes() {
        let base = Url::parse("https://example.com/blog/post/").unwrap();
        let map = ImportMap::parse(
            r#"{"imports": {"vue": "/js/vue.js", "lodash/": "https://cdn.example/lodash/", "./local.js": "./elsewhere.js"},
                "scopes": {"/admin/": {"vue": "/js/vue-admin.js"}}}"#,
            &base,
        )
        .unwrap();
        assert_eq!(
            map.resolve("vue", &base).as_deref(),
            Some("https://example.com/js/vue.js")
        );
        assert_eq!(
            map.resolve("lodash/debounce.js", &base).as_deref(),
            Some("https://cdn.example/lodash/debounce.js")
        );
        assert_eq!(
            map.resolve("./local.js", &base).as_deref(),
            Some("https://example.com/blog/post/elsewhere.js"),
            "a relative key is matched after resolution"
        );
        let admin = Url::parse("https://example.com/admin/page.js").unwrap();
        assert_eq!(
            map.resolve("vue", &admin).as_deref(),
            Some("https://example.com/js/vue-admin.js"),
            "a scope wins inside its prefix"
        );
        assert_eq!(map.resolve("react", &base), None);
    }

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

    #[test]
    fn a_multibyte_string_near_an_import_does_not_panic() {
        // 512 bytes after `import` lands inside a three-byte character.
        let mut source = String::from("import x from './a.js'; const s = '");
        while source.len() < 560 {
            source.push('十');
        }
        source.push_str("';");
        let found = import_specifiers(&source);
        assert!(found.contains(&"./a.js".to_owned()));
    }
}
