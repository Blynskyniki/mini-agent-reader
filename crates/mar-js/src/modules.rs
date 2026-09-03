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

        Module::declare(ctx.clone(), name.to_owned(), response.body)
    }
}
