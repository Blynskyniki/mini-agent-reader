//! CSS selector matching over the arena DOM.
//!
//! Matching is delegated to Servo's `selectors` crate, so `querySelector`
//! supports the real grammar: combinators, attribute operators, `:nth-child`,
//! `:not`, `:is`, `:where` and `:has`.

use crate::arena::{Document, NodeId};
use cssparser::{CowRcStr, ParseError, Parser, ParserInput, SourceLocation, ToCss};
use html5ever::{LocalName, Namespace, ns};
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode,
    SelectorCaches,
};
use selectors::matching::{ElementSelectorFlags, matches_selector};
use selectors::parser::{
    NonTSPseudoClass as NonTSPseudoClassTrait, Parser as SelectorParser,
    PseudoElement as PseudoElementTrait, Selector, SelectorList, SelectorParseErrorKind,
};
use selectors::{Element, OpaqueElement};
use std::fmt;

// ---------------------------------------------------------------------------
// Selector implementation types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct CssLocalName(pub LocalName);

impl<'a> From<&'a str> for CssLocalName {
    fn from(s: &'a str) -> Self {
        CssLocalName(LocalName::from(s))
    }
}

impl ToCss for CssLocalName {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(&self.0)
    }
}

impl precomputed_hash::PrecomputedHash for CssLocalName {
    fn precomputed_hash(&self) -> u32 {
        self.0.precomputed_hash()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct CssString(pub String);

impl AsRef<str> for CssString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<'a> From<&'a str> for CssString {
    fn from(s: &'a str) -> Self {
        CssString(s.to_owned())
    }
}

impl ToCss for CssString {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        cssparser::serialize_string(&self.0, dest)
    }
}

/// Pseudo-classes that depend on state a rendering browser would track.
///
/// We have no user interacting with the page, so these are matched against a
/// fixed, documented model rather than live state: link-ish ones look at the
/// DOM, interaction ones (`:hover`, `:active`, `:focus`) never match, and form
/// ones read the corresponding attribute.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NonTSPseudoClass {
    AnyLink,
    Link,
    Visited,
    Hover,
    Active,
    Focus,
    FocusVisible,
    FocusWithin,
    Target,
    Enabled,
    Disabled,
    Checked,
    Indeterminate,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    Valid,
    Invalid,
    Default,
    PlaceholderShown,
    Defined,
    /// Anything we do not model. Never matches, but parses, so one unknown
    /// pseudo-class in a selector list does not fail the whole query.
    Unsupported(String),
}

impl ToCss for NonTSPseudoClass {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        use NonTSPseudoClass::*;
        let name = match self {
            AnyLink => "any-link",
            Link => "link",
            Visited => "visited",
            Hover => "hover",
            Active => "active",
            Focus => "focus",
            FocusVisible => "focus-visible",
            FocusWithin => "focus-within",
            Target => "target",
            Enabled => "enabled",
            Disabled => "disabled",
            Checked => "checked",
            Indeterminate => "indeterminate",
            Required => "required",
            Optional => "optional",
            ReadOnly => "read-only",
            ReadWrite => "read-write",
            Valid => "valid",
            Invalid => "invalid",
            Default => "default",
            PlaceholderShown => "placeholder-shown",
            Defined => "defined",
            Unsupported(s) => s.as_str(),
        };
        write!(dest, ":{name}")
    }
}

impl NonTSPseudoClassTrait for NonTSPseudoClass {
    type Impl = MarSelectorImpl;

    fn is_active_or_hover(&self) -> bool {
        matches!(self, NonTSPseudoClass::Active | NonTSPseudoClass::Hover)
    }

    fn is_user_action_state(&self) -> bool {
        matches!(
            self,
            NonTSPseudoClass::Active | NonTSPseudoClass::Hover | NonTSPseudoClass::Focus
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PseudoElement(String);

impl ToCss for PseudoElement {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        write!(dest, "::{}", self.0)
    }
}

impl PseudoElementTrait for PseudoElement {
    type Impl = MarSelectorImpl;
}

#[derive(Clone, Debug)]
pub struct MarSelectorImpl;

impl selectors::SelectorImpl for MarSelectorImpl {
    type ExtraMatchingData<'a> = ();
    type AttrValue = CssString;
    type Identifier = CssLocalName;
    type LocalName = CssLocalName;
    type NamespaceUrl = Namespace;
    type NamespacePrefix = CssLocalName;
    type BorrowedNamespaceUrl = Namespace;
    type BorrowedLocalName = CssLocalName;
    type NonTSPseudoClass = NonTSPseudoClass;
    type PseudoElement = PseudoElement;
}

struct SelectorParserImpl;

impl<'i> SelectorParser<'i> for SelectorParserImpl {
    type Impl = MarSelectorImpl;
    type Error = SelectorParseErrorKind<'i>;

    // Level 4 selectors are off by default in the crate; a page's own CSS and
    // an agent's queries both use them, so turn them on.
    fn parse_is_and_where(&self) -> bool {
        true
    }

    fn parse_has(&self) -> bool {
        true
    }

    fn parse_nth_child_of(&self) -> bool {
        true
    }

    fn parse_part(&self) -> bool {
        true
    }

    fn parse_slotted(&self) -> bool {
        true
    }

    /// A selector list containing something we do not model should still parse.
    /// `allow_forgiving_selectors` only covers `:is`/`:where`, so unknown
    /// functional pseudo-classes are caught below instead of failing the query.
    fn allow_forgiving_selectors(&self) -> bool {
        true
    }

    fn parse_non_ts_functional_pseudo_class<'t>(
        &self,
        name: CowRcStr<'i>,
        parser: &mut Parser<'i, 't>,
        _after_part: bool,
    ) -> Result<NonTSPseudoClass, ParseError<'i, Self::Error>> {
        // Consume the argument list so the rest of the selector keeps parsing.
        while parser.next().is_ok() {}
        Ok(NonTSPseudoClass::Unsupported(
            name.as_ref().to_ascii_lowercase(),
        ))
    }

    fn parse_non_ts_pseudo_class(
        &self,
        _location: SourceLocation,
        name: CowRcStr<'i>,
    ) -> Result<NonTSPseudoClass, ParseError<'i, Self::Error>> {
        Ok(parse_pseudo_class_name(&name))
    }

    fn parse_pseudo_element(
        &self,
        _location: SourceLocation,
        name: CowRcStr<'i>,
    ) -> Result<PseudoElement, ParseError<'i, Self::Error>> {
        Ok(PseudoElement(name.as_ref().to_ascii_lowercase()))
    }
}

fn parse_pseudo_class_name(name: &str) -> NonTSPseudoClass {
    use NonTSPseudoClass::*;
    match name.to_ascii_lowercase().as_str() {
        "any-link" => AnyLink,
        "link" => Link,
        "visited" => Visited,
        "hover" => Hover,
        "active" => Active,
        "focus" => Focus,
        "focus-visible" => FocusVisible,
        "focus-within" => FocusWithin,
        "target" => Target,
        "enabled" => Enabled,
        "disabled" => Disabled,
        "checked" => Checked,
        "indeterminate" => Indeterminate,
        "required" => Required,
        "optional" => Optional,
        "read-only" => ReadOnly,
        "read-write" => ReadWrite,
        "valid" => Valid,
        "invalid" => Invalid,
        "default" => Default,
        "placeholder-shown" => PlaceholderShown,
        "defined" => Defined,
        other => Unsupported(other.to_owned()),
    }
}

// ---------------------------------------------------------------------------
// Element handle
// ---------------------------------------------------------------------------

/// A borrowed element: an arena index plus the document it belongs to.
///
/// Copy and 16 bytes wide, so the matcher can pass it around freely.
#[derive(Clone, Copy)]
pub struct ElementRef<'a> {
    pub doc: &'a Document,
    pub id: NodeId,
}

impl<'a> ElementRef<'a> {
    /// Wrap `id` if it is an element node.
    #[inline]
    pub fn new(doc: &'a Document, id: NodeId) -> Option<Self> {
        doc.data(id).is_element().then_some(ElementRef { doc, id })
    }

    #[inline]
    fn element(&self) -> &'a crate::arena::ElementData {
        self.doc
            .element(self.id)
            .expect("ElementRef always wraps an element")
    }

    #[inline]
    fn attr(&self, name: &str) -> Option<&'a str> {
        self.element().attr(&LocalName::from(name))
    }
}

impl fmt::Debug for ElementRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} {:?}>", self.element().local_name(), self.id)
    }
}

impl<'a> Element for ElementRef<'a> {
    type Impl = MarSelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // Identity is the arena slot; the document backs a stable allocation
        // for as long as this borrow lives.
        OpaqueElement::new(&self.doc.node(self.id).data)
    }

    fn parent_element(&self) -> Option<Self> {
        let parent = self.doc.node(self.id).parent?;
        ElementRef::new(self.doc, parent)
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let prev = self.doc.prev_element_sibling(self.id)?;
        ElementRef::new(self.doc, prev)
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let next = self.doc.next_element_sibling(self.id)?;
        ElementRef::new(self.doc, next)
    }

    fn first_element_child(&self) -> Option<Self> {
        self.doc
            .children(self.id)
            .find(|&c| self.doc.data(c).is_element())
            .and_then(|c| ElementRef::new(self.doc, c))
    }

    fn is_html_element_in_html_document(&self) -> bool {
        self.element().name.ns == ns!(html)
    }

    fn has_local_name(&self, name: &CssLocalName) -> bool {
        *self.element().local_name() == name.0
    }

    fn has_namespace(&self, namespace: &Namespace) -> bool {
        self.element().name.ns == *namespace
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.element().name == other.element().name
    }

    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&Namespace>,
        local_name: &CssLocalName,
        operation: &AttrSelectorOperation<&CssString>,
    ) -> bool {
        self.element().attrs.iter().any(|attr| {
            let ns_ok = match ns {
                NamespaceConstraint::Any => true,
                NamespaceConstraint::Specific(url) => attr.name.ns == **url,
            };
            ns_ok && attr.name.local == local_name.0 && operation.eval_str(&attr.value)
        })
    }

    fn has_attr_in_no_namespace(&self, local_name: &CssLocalName) -> bool {
        self.element()
            .attrs
            .iter()
            .any(|a| a.name.ns == ns!() && a.name.local == local_name.0)
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &NonTSPseudoClass,
        _context: &mut MatchingContext<'_, MarSelectorImpl>,
    ) -> bool {
        use NonTSPseudoClass::*;
        let el = self.element();
        let tag = el.local_name().as_ref();
        match pc {
            // Link-ish: decided by the DOM, not by history we do not have.
            AnyLink | Link => {
                matches!(tag, "a" | "area" | "link") && el.has_attr(&LocalName::from("href"))
            }
            Visited => false,
            // No pointer, no keyboard, no fragment navigation.
            Hover | Active | Focus | FocusVisible | FocusWithin | Target => false,
            Disabled => {
                matches!(
                    tag,
                    "button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset"
                ) && el.has_attr(&LocalName::from("disabled"))
            }
            Enabled => {
                matches!(
                    tag,
                    "button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset"
                ) && !el.has_attr(&LocalName::from("disabled"))
            }
            Checked => {
                (matches!(tag, "input") && el.has_attr(&LocalName::from("checked")))
                    || (tag == "option" && el.has_attr(&LocalName::from("selected")))
            }
            Required => el.has_attr(&LocalName::from("required")),
            Optional => {
                matches!(tag, "input" | "select" | "textarea")
                    && !el.has_attr(&LocalName::from("required"))
            }
            ReadOnly => {
                !matches!(tag, "input" | "textarea") || el.has_attr(&LocalName::from("readonly"))
            }
            ReadWrite => {
                matches!(tag, "input" | "textarea") && !el.has_attr(&LocalName::from("readonly"))
            }
            PlaceholderShown => {
                matches!(tag, "input" | "textarea")
                    && el.has_attr(&LocalName::from("placeholder"))
                    && el.attr(&LocalName::from("value")).unwrap_or("").is_empty()
            }
            Defined => true,
            // Constraint validation and :indeterminate need state we do not track.
            Indeterminate | Valid | Invalid | Default => false,
            Unsupported(_) => false,
        }
    }

    fn match_pseudo_element(
        &self,
        _pe: &PseudoElement,
        _context: &mut MatchingContext<'_, MarSelectorImpl>,
    ) -> bool {
        // ::before / ::after generate boxes, and we have no box tree.
        false
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {
        // Flags exist to invalidate style on DOM mutation. We restyle nothing.
    }

    fn is_link(&self) -> bool {
        matches!(self.element().local_name().as_ref(), "a" | "area" | "link")
            && self.element().has_attr(&LocalName::from("href"))
    }

    fn is_html_slot_element(&self) -> bool {
        self.element().local_name().as_ref() == "slot"
    }

    fn has_id(&self, id: &CssLocalName, case_sensitivity: CaseSensitivity) -> bool {
        self.attr("id")
            .is_some_and(|v| case_sensitivity.eq(v.as_bytes(), id.0.as_bytes()))
    }

    fn has_class(&self, name: &CssLocalName, case_sensitivity: CaseSensitivity) -> bool {
        self.attr("class").is_some_and(|class_attr| {
            class_attr
                .split_ascii_whitespace()
                .any(|c| case_sensitivity.eq(c.as_bytes(), name.0.as_bytes()))
        })
    }

    fn has_custom_state(&self, _name: &CssLocalName) -> bool {
        false
    }

    fn imported_part(&self, _name: &CssLocalName) -> Option<CssLocalName> {
        None
    }

    fn is_part(&self, _name: &CssLocalName) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        // :empty ignores comments but not text, including whitespace.
        !self.doc.children(self.id).any(|c| match self.doc.data(c) {
            crate::arena::NodeData::Text(t) => !t.is_empty(),
            crate::arena::NodeData::Element(_) => true,
            _ => false,
        })
    }

    fn is_root(&self) -> bool {
        self.doc.node(self.id).parent == Some(self.doc.root())
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        // Returning false tells the matcher to skip bloom-filter fast paths.
        // Documents here are small and queried a handful of times, so building
        // the filter costs more than the rejections it would save.
        false
    }
}

// ---------------------------------------------------------------------------
// Public query API
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
#[error("invalid CSS selector {selector:?}: {message}")]
pub struct SelectorError {
    pub selector: String,
    pub message: String,
}

/// A parsed selector list, reusable across many queries.
pub struct Matcher {
    list: SelectorList<MarSelectorImpl>,
    source: String,
}

impl Matcher {
    pub fn new(selector: &str) -> Result<Self, SelectorError> {
        let mut input = ParserInput::new(selector);
        let mut parser = Parser::new(&mut input);
        let list = SelectorList::parse(
            &SelectorParserImpl,
            &mut parser,
            selectors::parser::ParseRelative::No,
        )
        .map_err(|e| SelectorError {
            selector: selector.to_owned(),
            message: format!("{e:?}"),
        })?;
        Ok(Matcher {
            list,
            source: selector.to_owned(),
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    fn selectors(&self) -> &[Selector<MarSelectorImpl>] {
        self.list.slice()
    }

    /// Does `element` match this selector list?
    ///
    /// Allocates a fresh cache per call. Use [`Matcher::query_all`] or
    /// [`Matcher::query_first`] when testing many elements: they build one
    /// cache and reuse it, which is what makes `:nth-child` and `:has` cheap.
    pub fn matches(&self, element: ElementRef<'_>) -> bool {
        let mut caches = SelectorCaches::default();
        self.matches_cached(element, &mut caches)
    }

    fn matches_cached(&self, element: ElementRef<'_>, caches: &mut SelectorCaches) -> bool {
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            caches,
            quirks_mode_of(element.doc),
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        self.selectors()
            .iter()
            .any(|s| matches_selector(s, 0, None, &element, &mut context))
    }

    /// First descendant of `root` (excluding `root`) that matches.
    pub fn query_first(&self, doc: &Document, root: NodeId) -> Option<NodeId> {
        let mut caches = SelectorCaches::default();
        doc.descendants(root)
            .filter_map(|id| ElementRef::new(doc, id))
            .find(|e| self.matches_cached(*e, &mut caches))
            .map(|e| e.id)
    }

    /// Every descendant of `root` (excluding `root`) that matches, in document order.
    pub fn query_all(&self, doc: &Document, root: NodeId) -> Vec<NodeId> {
        let mut caches = SelectorCaches::default();
        doc.descendants(root)
            .filter_map(|id| ElementRef::new(doc, id))
            .filter(|e| self.matches_cached(*e, &mut caches))
            .map(|e| e.id)
            .collect()
    }

    /// Nearest inclusive ancestor of `start` that matches (`Element.closest`).
    pub fn closest(&self, doc: &Document, start: NodeId) -> Option<NodeId> {
        let mut caches = SelectorCaches::default();
        std::iter::once(start)
            .chain(doc.ancestors(start))
            .filter_map(|id| ElementRef::new(doc, id))
            .find(|e| self.matches_cached(*e, &mut caches))
            .map(|e| e.id)
    }
}

fn quirks_mode_of(doc: &Document) -> QuirksMode {
    match doc.quirks_mode {
        markup5ever::interface::QuirksMode::Quirks => QuirksMode::Quirks,
        markup5ever::interface::QuirksMode::LimitedQuirks => QuirksMode::LimitedQuirks,
        markup5ever::interface::QuirksMode::NoQuirks => QuirksMode::NoQuirks,
    }
}

/// One-shot `querySelector`. Prefer [`Matcher`] when querying repeatedly.
pub fn query_selector(
    doc: &Document,
    root: NodeId,
    selector: &str,
) -> Result<Option<NodeId>, SelectorError> {
    Ok(Matcher::new(selector)?.query_first(doc, root))
}

/// One-shot `querySelectorAll`. Prefer [`Matcher`] when querying repeatedly.
pub fn query_selector_all(
    doc: &Document,
    root: NodeId,
    selector: &str,
) -> Result<Vec<NodeId>, SelectorError> {
    Ok(Matcher::new(selector)?.query_all(doc, root))
}
