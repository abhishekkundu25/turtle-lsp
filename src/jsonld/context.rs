//! JSON-LD 1.1 context processing: the active context and its term
//! definitions (`@vocab`, `@base`, `@language`, `@direction`, `@protected`,
//! `@propagate`, `@import`, scoped contexts, keyword aliases), read from
//! [`super::json::Json`] with remote contexts from a
//! [`super::ContextLoader`]. It follows the JSON-LD 1.1 Processing
//! Algorithms: §4.1 Context Processing, §4.2 Create Term Definition and
//! §4.3 IRI Expansion. The processing mode is always `json-ld-1.1`.
//!
//! # API (what expansion builds on)
//!
//! - [`ActiveContext::new`]`(base)`: the initial context of a document whose
//!   URL is `base`. Cloning one is cheap (the term definitions are shared
//!   behind `Arc`s), so expansion clones it freely.
//! - [`Processor::process`]`(&active, &local_context, base_url, options)
//!   -> Result<ActiveContext, ContextError>`: the Context Processing
//!   algorithm for an `@context` value written in the document. `options`
//!   ([`Options`]) carry `override_protected`, `propagate` and
//!   `validate_scoped_context` as the spec names them.
//! - [`Processor::process_scoped`]`(&active, &scoped, at, propagate,
//!   override_protected)`: applies a term's scoped context
//!   ([`TermDefinition::context`]) during expansion, with the term
//!   definition's base URL. A property-scoped context passes `true, true`, a
//!   type-scoped one `false, false`. `at` is
//!   the span to report errors on when the scoped context came from a remote
//!   document (the key or type that applied it). Results are memoized per
//!   processor by (active context, scoped context), so applying the same
//!   type-scoped context to many nodes processes it once.
//! - [`ActiveContext::term`]`(term) -> Option<&TermDefinition>` and
//!   [`ActiveContext::terms`]: term definitions; [`ActiveContext::vocab`],
//!   [`ActiveContext::base`], [`ActiveContext::default_language`],
//!   [`ActiveContext::default_direction`], and
//!   [`ActiveContext::previous`] (set by a non-propagated context: expansion
//!   reverts to it for new node objects, step 7 of the Expansion algorithm).
//! - [`expand_iri`]`(&active, value, document_relative, vocab) ->
//!   Option<String>`: IRI Expansion outside context processing. `None` is the
//!   spec's `null`: a term mapped to `null`, or a value that only looks like
//!   a keyword (`@foo`), which expansion drops with a warning
//!   ([`has_keyword_form`]). Keywords and keyword aliases expand to the
//!   keyword itself.
//! - Errors are [`ContextError`]s: a W3C error code ([`ErrorCode`]), the
//!   span of the offending JSON in the document, and a message.
//!   [`ContextError::to_syntax_error`] makes the `jsonld-context`
//!   diagnostic, whose message starts with the W3C code. Warnings (a term
//!   or `@id` that looks like a keyword, ignored per the spec) accumulate on
//!   the processor: [`Processor::take_warnings`].
//!
//! # Spans and remote documents
//!
//! Every error points into the document being checked:
//! - an error in a term definition (including in its scoped context, and in
//!   the term definitions it depends on) sits on the term's key, without
//!   the quotes;
//! - an error in another entry of a context (`@vocab`, `@base`, ...) sits on
//!   its value: a string's content without the quotes, other JSON whole;
//! - an error inside a remote context (or an `@import`ed one) is reported on
//!   the string that names it (or the term whose definition names it), with
//!   the remote URL in the message; warnings inside remote contexts are not
//!   reported.
//!
//! Term definitions written in the document keep the span of their key,
//! without the quotes ([`TermDefinition::span`]).
//!
//! # Caching
//!
//! A [`ContextCache`] (by default [`ContextCache::global`], shared by every
//! thread) keeps each remote document's parsed JSON and its processed
//! results, keyed by URL and content. A result is reused when the context it
//! was applied to is the same (an empty initial context, or the very result
//! of an earlier cached step), and when every document it loaded on the way
//! (nested contexts, imports) still has the same content. So re-checking a
//! document re-processes only its inline contexts.

use super::json::{Json, Object, Value};
use super::loader::{ContextLoader, LoadError};
use super::unquoted;
use crate::syntax::{Severity, Span, SyntaxError};
use rustc_hash::FxHashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The diagnostic code of every context error.
pub const DIAGNOSTIC_CODE: &str = "jsonld-context";

/// How many remote contexts may be nested (the spec's "processor defined
/// limit"); one more is a `context overflow`.
pub const MAX_REMOTE_CONTEXTS: usize = 32;

// ---- errors ----

/// The JSON-LD 1.1 error codes context processing raises.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    ContextOverflow,
    CyclicIriMapping,
    InvalidBaseDirection,
    InvalidBaseIri,
    InvalidContainerMapping,
    InvalidContextEntry,
    InvalidContextNullification,
    InvalidDefaultLanguage,
    InvalidImportValue,
    InvalidIriMapping,
    InvalidKeywordAlias,
    InvalidLanguageMapping,
    InvalidLocalContext,
    InvalidNestValue,
    InvalidPrefixValue,
    InvalidPropagateValue,
    InvalidProtectedValue,
    InvalidRemoteContext,
    InvalidReverseProperty,
    InvalidScopedContext,
    InvalidTermDefinition,
    InvalidTypeMapping,
    InvalidVersionValue,
    InvalidVocabMapping,
    KeywordRedefinition,
    LoadingRemoteContextFailed,
    ProtectedTermRedefinition,
}

impl ErrorCode {
    /// The code as the spec writes it (`invalid term definition`).
    pub fn as_str(self) -> &'static str {
        use ErrorCode::*;
        match self {
            ContextOverflow => "context overflow",
            CyclicIriMapping => "cyclic IRI mapping",
            InvalidBaseDirection => "invalid base direction",
            InvalidBaseIri => "invalid base IRI",
            InvalidContainerMapping => "invalid container mapping",
            InvalidContextEntry => "invalid context entry",
            InvalidContextNullification => "invalid context nullification",
            InvalidDefaultLanguage => "invalid default language",
            InvalidImportValue => "invalid @import value",
            InvalidIriMapping => "invalid IRI mapping",
            InvalidKeywordAlias => "invalid keyword alias",
            InvalidLanguageMapping => "invalid language mapping",
            InvalidLocalContext => "invalid local context",
            InvalidNestValue => "invalid @nest value",
            InvalidPrefixValue => "invalid @prefix value",
            InvalidPropagateValue => "invalid @propagate value",
            InvalidProtectedValue => "invalid @protected value",
            InvalidRemoteContext => "invalid remote context",
            InvalidReverseProperty => "invalid reverse property",
            InvalidScopedContext => "invalid scoped context",
            InvalidTermDefinition => "invalid term definition",
            InvalidTypeMapping => "invalid type mapping",
            InvalidVersionValue => "invalid @version value",
            InvalidVocabMapping => "invalid vocab mapping",
            KeywordRedefinition => "keyword redefinition",
            LoadingRemoteContextFailed => "loading remote context failed",
            ProtectedTermRedefinition => "protected term redefinition",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A context processing error: the algorithm stops at the first one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextError {
    pub code: ErrorCode,
    /// The offending key or value in the document (for a remote context,
    /// the string naming it).
    pub span: Span,
    /// What is wrong, without the code.
    pub message: String,
}

impl ContextError {
    /// The `jsonld-context` diagnostic: "<W3C code>: <message>".
    pub fn to_syntax_error(&self) -> SyntaxError {
        SyntaxError {
            span: self.span,
            message: self.to_string(),
            severity: Severity::Error,
            code: DIAGNOSTIC_CODE,
        }
    }
}

impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

// ---- keywords and IRIs ----

/// The JSON-LD 1.1 keywords.
pub const KEYWORDS: &[&str] = &[
    "@base",
    "@container",
    "@context",
    "@direction",
    "@graph",
    "@id",
    "@import",
    "@included",
    "@index",
    "@json",
    "@language",
    "@list",
    "@nest",
    "@none",
    "@prefix",
    "@propagate",
    "@protected",
    "@reverse",
    "@set",
    "@type",
    "@value",
    "@version",
    "@vocab",
];

/// The entries of a context definition that are not term definitions.
const CONTEXT_KEYWORDS: &[&str] = &[
    "@base",
    "@direction",
    "@import",
    "@language",
    "@propagate",
    "@protected",
    "@version",
    "@vocab",
];

/// The entries an expanded term definition may have.
const TERM_KEYWORDS: &[&str] = &[
    "@id",
    "@reverse",
    "@container",
    "@context",
    "@direction",
    "@index",
    "@language",
    "@nest",
    "@prefix",
    "@protected",
    "@type",
];

pub fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

/// `@` followed by one or more ASCII letters: reserved for future keywords.
/// Such terms and IRIs are ignored (with a warning) unless they are
/// keywords.
pub fn has_keyword_form(s: &str) -> bool {
    s.strip_prefix('@')
        .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphabetic()))
}

/// Whether `s` has the form of an absolute IRI: a scheme and a colon.
pub fn is_absolute_iri(s: &str) -> bool {
    let Some(colon) = s.find(':') else {
        return false;
    };
    let scheme = &s.as_bytes()[..colon];
    scheme.first().is_some_and(u8::is_ascii_alphabetic)
        && scheme
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

pub fn is_blank_node(s: &str) -> bool {
    s.starts_with("_:")
}

/// Resolve `reference` against `base` (RFC 3986 §5.2, without
/// normalization). Neither is checked for validity.
pub fn resolve_iri(base: &str, reference: &str) -> String {
    let r = UriParts::parse(reference);
    if r.scheme.is_some() {
        return UriParts {
            path: &remove_dot_segments(r.path),
            ..r
        }
        .to_string();
    }
    let b = UriParts::parse(base);
    let (authority, path, query);
    let merged;
    if r.authority.is_some() {
        authority = r.authority;
        merged = remove_dot_segments(r.path);
        path = merged.as_str();
        query = r.query;
    } else {
        authority = b.authority;
        if r.path.is_empty() {
            path = b.path;
            query = r.query.or(b.query);
        } else {
            merged = if r.path.starts_with('/') {
                remove_dot_segments(r.path)
            } else if b.authority.is_some() && b.path.is_empty() {
                remove_dot_segments(&format!("/{}", r.path))
            } else {
                let dir = b.path.rfind('/').map_or("", |i| &b.path[..=i]);
                remove_dot_segments(&format!("{dir}{}", r.path))
            };
            path = merged.as_str();
            query = r.query;
        }
    }
    UriParts {
        scheme: b.scheme,
        authority,
        path,
        query,
        fragment: r.fragment,
    }
    .to_string()
}

/// The five components of RFC 3986 Appendix B.
struct UriParts<'a> {
    scheme: Option<&'a str>,
    authority: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

impl<'a> UriParts<'a> {
    fn parse(s: &'a str) -> UriParts<'a> {
        let (rest, fragment) = match s.split_once('#') {
            Some((a, f)) => (a, Some(f)),
            None => (s, None),
        };
        let (rest, query) = match rest.split_once('?') {
            Some((a, q)) => (a, Some(q)),
            None => (rest, None),
        };
        let (scheme, rest) = match rest.find([':', '/']) {
            Some(i) if i > 0 && rest.as_bytes()[i] == b':' => (Some(&rest[..i]), &rest[i + 1..]),
            _ => (None, rest),
        };
        let (authority, path) = match rest.strip_prefix("//") {
            Some(r) => {
                let end = r.find('/').unwrap_or(r.len());
                (Some(&r[..end]), &r[end..])
            }
            None => (None, rest),
        };
        UriParts {
            scheme,
            authority,
            path,
            query,
            fragment,
        }
    }
}

impl fmt::Display for UriParts<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(s) = self.scheme {
            write!(f, "{s}:")?;
        }
        if let Some(a) = self.authority {
            write!(f, "//{a}")?;
        }
        f.write_str(self.path)?;
        if let Some(q) = self.query {
            write!(f, "?{q}")?;
        }
        if let Some(fr) = self.fragment {
            write!(f, "#{fr}")?;
        }
        Ok(())
    }
}

/// RFC 3986 §5.2.4.
fn remove_dot_segments(path: &str) -> String {
    fn pop(out: &mut String) {
        let at = out.rfind('/').unwrap_or(0);
        out.truncate(at);
    }
    let mut input = path;
    let mut out = String::with_capacity(path.len());
    while !input.is_empty() {
        if let Some(r) = input.strip_prefix("../") {
            input = r;
        } else if let Some(r) = input.strip_prefix("./") {
            input = r;
        } else if input.starts_with("/./") {
            input = &input[2..];
        } else if input == "/." {
            input = "/";
        } else if input.starts_with("/../") {
            input = &input[3..];
            pop(&mut out);
        } else if input == "/.." {
            input = "/";
            pop(&mut out);
        } else if input == "." || input == ".." {
            input = "";
        } else {
            let skip = usize::from(input.starts_with('/'));
            let end = input[skip..].find('/').map_or(input.len(), |i| i + skip);
            out.push_str(&input[..end]);
            input = &input[end..];
        }
    }
    out
}

// ---- term definitions ----

/// A base direction (`@direction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Ltr,
    Rtl,
}

/// A container mapping, as a set of container keywords.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Container(u8);

impl Container {
    pub const LIST: Container = Container(1);
    pub const SET: Container = Container(2);
    pub const INDEX: Container = Container(4);
    pub const LANGUAGE: Container = Container(8);
    pub const GRAPH: Container = Container(16);
    pub const ID: Container = Container(32);
    pub const TYPE: Container = Container(64);

    pub fn contains(self, other: Container) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn with(self, other: Container) -> Container {
        Container(self.0 | other.0)
    }

    fn keyword(s: &str) -> Option<Container> {
        Some(match s {
            "@list" => Container::LIST,
            "@set" => Container::SET,
            "@index" => Container::INDEX,
            "@language" => Container::LANGUAGE,
            "@graph" => Container::GRAPH,
            "@id" => Container::ID,
            "@type" => Container::TYPE,
            _ => return None,
        })
    }

    fn count(self) -> u32 {
        self.0.count_ones()
    }
}

/// A term's scoped context, kept unprocessed as the spec does: expansion
/// applies it with [`Processor::process_scoped`].
#[derive(Clone, Debug)]
pub struct ScopedContext {
    /// The `@context` value of the term definition. Its spans are in the
    /// document named by `source` (`None`: the document being checked).
    pub json: Arc<Json>,
    /// The base URL relative context references in it resolve against.
    pub base_url: Option<Arc<str>>,
    /// The remote document it was read from, if any.
    pub source: Option<Arc<str>>,
}

/// A term definition (§4.2).
#[derive(Clone, Debug, Default)]
pub struct TermDefinition {
    /// The IRI mapping: an IRI, a blank node identifier or a keyword (for an
    /// alias). `None` for a term mapped to `null`, kept so a later
    /// definition can be checked against a protected one.
    pub iri: Option<Arc<str>>,
    pub reverse: bool,
    /// `@id`, `@vocab`, `@json`, `@none` or an IRI.
    pub type_mapping: Option<Arc<str>>,
    /// `Some(None)`: `"@language": null`.
    pub language: Option<Option<Arc<str>>>,
    /// `Some(None)`: `"@direction": null`.
    pub direction: Option<Option<Direction>>,
    pub container: Container,
    pub context: Option<ScopedContext>,
    /// Usable as the prefix of a compact IRI.
    pub prefix: bool,
    pub protected: bool,
    pub nest: Option<Arc<str>>,
    pub index: Option<Arc<str>>,
    /// The key that defined the term, when it is written in the document
    /// being checked (not in a remote context).
    pub span: Option<Span>,
}

impl TermDefinition {
    /// The same definition, ignoring `protected` and where it was written
    /// (§4.2.2 step 27.1).
    fn same_as(&self, o: &TermDefinition) -> bool {
        self.iri == o.iri
            && self.reverse == o.reverse
            && self.type_mapping == o.type_mapping
            && self.language == o.language
            && self.direction == o.direction
            && self.container == o.container
            && self.prefix == o.prefix
            && self.nest == o.nest
            && self.index == o.index
            && match (&self.context, &o.context) {
                (None, None) => true,
                (Some(a), Some(b)) => json_eq(&a.json, &b.json),
                _ => false,
            }
    }
}

/// Equal JSON values, ignoring spans and the order of object members.
fn json_eq(a: &Json, b: &Json) -> bool {
    match (&a.value, &b.value) {
        (Value::Null, Value::Null) | (Value::Invalid, Value::Invalid) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => {
            x == y || matches!((x.parse::<f64>(), y.parse::<f64>()), (Ok(p), Ok(q)) if p == q)
        }
        (Value::Array(x), Value::Array(y)) => {
            x.items.len() == y.items.len()
                && x.items.iter().zip(&y.items).all(|(p, q)| json_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            fn keys(o: &Object) -> Vec<&str> {
                let mut k: Vec<&str> = o.members.iter().map(|m| &*m.key).collect();
                k.sort_unstable();
                k.dedup();
                k
            }
            keys(x) == keys(y)
                && x.members.iter().all(|m| {
                    x.get(&m.key)
                        .zip(y.get(&m.key))
                        .is_some_and(|(p, q)| json_eq(&p.value, &q.value))
                })
        }
        _ => false,
    }
}

// ---- the active context ----

type DefMap = FxHashMap<Arc<str>, Arc<TermDefinition>>;

/// Term definitions: a shared base map and a small overlay of changes
/// (`None` removes a term), so a scoped context over a large context (such
/// as schema.org's) does not copy every definition.
#[derive(Clone, Debug, Default)]
struct Terms {
    base: Arc<DefMap>,
    overlay: Arc<FxHashMap<Arc<str>, Option<Arc<TermDefinition>>>>,
}

impl Terms {
    fn get(&self, term: &str) -> Option<&Arc<TermDefinition>> {
        match self.overlay.get(term) {
            Some(d) => d.as_ref(),
            None => self.base.get(term),
        }
    }

    fn set(&mut self, term: &str, def: Option<Arc<TermDefinition>>) {
        let overlay = Arc::make_mut(&mut self.overlay);
        if def.is_none() && !self.base.contains_key(term) {
            overlay.remove(term);
        } else {
            overlay.insert(Arc::from(term), def);
        }
    }

    fn remove(&mut self, term: &str) -> Option<Arc<TermDefinition>> {
        let previous = self.get(term).cloned();
        if previous.is_some() {
            self.set(term, None);
        }
        previous
    }

    fn is_known_empty(&self) -> bool {
        self.base.is_empty() && self.overlay.is_empty()
    }

    fn iter(&self) -> impl Iterator<Item = (&str, &TermDefinition)> {
        let overlay = self
            .overlay
            .iter()
            .filter_map(|(k, d)| d.as_ref().map(|d| (&**k, &**d)));
        let base = self
            .base
            .iter()
            .filter(|(k, _)| !self.overlay.contains_key(&***k))
            .map(|(k, d)| (&**k, &**d));
        overlay.chain(base)
    }

    /// Fold the overlay into the base once it is a sizeable part of it.
    fn compact(&mut self) {
        if self.overlay.is_empty() || self.overlay.len() * 4 <= self.base.len() {
            return;
        }
        let overlay = std::mem::take(&mut self.overlay);
        let base = Arc::make_mut(&mut self.base);
        for (k, d) in overlay.iter() {
            match d {
                Some(d) => base.insert(k.clone(), d.clone()),
                None => base.remove(k),
            };
        }
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// The active context (§4.1). Cheap to clone.
#[derive(Clone, Debug)]
pub struct ActiveContext {
    terms: Terms,
    base: Option<Arc<str>>,
    original_base: Option<Arc<str>>,
    vocab: Option<Arc<str>>,
    language: Option<Arc<str>>,
    direction: Option<Direction>,
    previous: Option<Arc<ActiveContext>>,
    /// Identifies the content (everything but the base IRIs), for caching:
    /// a fresh one for every processing result.
    id: u64,
}

impl ActiveContext {
    /// The initial context of a document at `base` (its base IRI and
    /// original base URL).
    pub fn new(base: Option<&str>) -> ActiveContext {
        ActiveContext::reset(base.map(Arc::from))
    }

    fn reset(original_base: Option<Arc<str>>) -> ActiveContext {
        ActiveContext {
            terms: Terms::default(),
            base: original_base.clone(),
            original_base,
            vocab: None,
            language: None,
            direction: None,
            previous: None,
            id: next_id(),
        }
    }

    pub fn term(&self, term: &str) -> Option<&TermDefinition> {
        self.terms.get(term).map(|d| &**d)
    }

    /// Every term definition, in no particular order.
    pub fn terms(&self) -> impl Iterator<Item = (&str, &TermDefinition)> {
        self.terms.iter()
    }

    /// The base IRI relative IRIs resolve against.
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    pub fn original_base(&self) -> Option<&str> {
        self.original_base.as_deref()
    }

    pub fn vocab(&self) -> Option<&str> {
        self.vocab.as_deref()
    }

    pub fn default_language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    pub fn default_direction(&self) -> Option<Direction> {
        self.direction
    }

    /// Identifies what the context holds apart from its base IRI (terms,
    /// `@vocab`, default language and direction, previous context): clones
    /// share it, and each processing result gets a new one. Expansion caches
    /// IRI expansions by it.
    pub fn content_id(&self) -> u64 {
        self.id
    }

    /// The context before a non-propagated context was applied.
    pub fn previous(&self) -> Option<&ActiveContext> {
        self.previous.as_deref()
    }

    pub fn has_protected(&self) -> bool {
        self.terms.iter().any(|(_, d)| d.protected)
    }

    /// Newly initialized: no definitions, mappings or previous context, and
    /// the base IRI is the original one.
    fn is_pristine(&self) -> bool {
        self.terms.is_known_empty()
            && self.vocab.is_none()
            && self.language.is_none()
            && self.direction.is_none()
            && self.previous.is_none()
            && self.base == self.original_base
    }
}

/// IRI Expansion (§4.3) outside context processing. `None` is `null`.
pub fn expand_iri(
    active: &ActiveContext,
    value: &str,
    document_relative: bool,
    vocab: bool,
) -> Option<String> {
    if is_keyword(value) {
        return Some(value.to_string());
    }
    if has_keyword_form(value) {
        return None;
    }
    expand_from_step_4(active, value, document_relative, vocab, &mut false)
}

/// Steps 4 to 9 of IRI Expansion; `used_base` is set when the base IRI was
/// used.
fn expand_from_step_4(
    active: &ActiveContext,
    value: &str,
    document_relative: bool,
    vocab: bool,
    used_base: &mut bool,
) -> Option<String> {
    if let Some(d) = active.terms.get(value)
        && (vocab || d.iri.as_deref().is_some_and(is_keyword))
    {
        return d.iri.as_deref().map(str::to_string);
    }
    if let Some(colon) = value.find(':').filter(|&i| i > 0) {
        let (prefix, suffix) = (&value[..colon], &value[colon + 1..]);
        if prefix == "_" || suffix.starts_with("//") {
            return Some(value.to_string());
        }
        if let Some(d) = active.terms.get(prefix)
            && d.prefix
            && let Some(iri) = &d.iri
        {
            return Some(format!("{iri}{suffix}"));
        }
        if is_absolute_iri(value) {
            return Some(value.to_string());
        }
    }
    if vocab && let Some(v) = &active.vocab {
        return Some(format!("{v}{value}"));
    }
    if document_relative && let Some(base) = &active.base {
        *used_base = true;
        return Some(resolve_iri(base, value));
    }
    Some(value.to_string())
}

// ---- the cache ----

/// Processed remote contexts by URL and content, shared across threads.
pub struct ContextCache {
    docs: Mutex<Vec<CachedDoc>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

/// How often a remote context was taken from the cache, or processed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
}

const MAX_CACHED_DOCS: usize = 64;
const MAX_RESULTS_PER_DOC: usize = 16;

struct CachedDoc {
    url: Box<str>,
    text: Arc<str>,
    json: Result<Arc<Json>, String>,
    results: Vec<Processed>,
}

/// What the context a remote context was applied to was.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Input {
    /// A newly initialized context.
    Pristine,
    /// The context with this id.
    Id(u64),
}

impl Input {
    fn of(active: &ActiveContext) -> Input {
        if active.is_pristine() {
            Input::Pristine
        } else {
            Input::Id(active.id)
        }
    }
}

#[derive(Clone)]
struct Processed {
    input: Input,
    input_base: Option<Arc<str>>,
    remote: Vec<Arc<str>>,
    validate: bool,
    /// The base IRI was used, so the result holds only for `input_base`.
    used_base: bool,
    /// Every document loaded while processing, with what it held.
    deps: Vec<Dep>,
    result: Result<ActiveContext, (ErrorCode, String)>,
}

#[derive(Clone, PartialEq, Eq)]
struct Dep {
    url: Arc<str>,
    text: Result<Arc<str>, LoadError>,
}

fn same_text(a: &Arc<str>, b: &Arc<str>) -> bool {
    Arc::ptr_eq(a, b) || a == b
}

impl Default for ContextCache {
    fn default() -> Self {
        ContextCache::new()
    }
}

impl ContextCache {
    pub fn new() -> ContextCache {
        ContextCache {
            docs: Mutex::new(Vec::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// The process-wide cache.
    pub fn global() -> &'static ContextCache {
        static CACHE: OnceLock<ContextCache> = OnceLock::new();
        CACHE.get_or_init(ContextCache::new)
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }

    /// The parsed JSON of the document at `url` with `text`, or why it is
    /// not JSON.
    fn parsed(&self, url: &str, text: &Arc<str>) -> Result<Arc<Json>, String> {
        if let Ok(mut docs) = self.docs.lock()
            && let Some(i) = docs
                .iter()
                .position(|d| &*d.url == url && same_text(&d.text, text))
        {
            let doc = docs.remove(i);
            let json = doc.json.clone();
            docs.insert(0, doc);
            return json;
        }
        let doc = super::json::parse(text);
        let json = match (doc.root, doc.errors.first()) {
            (_, Some(e)) => Err(e.message.clone()),
            (None, None) => Err("the document is empty".to_string()),
            (Some(root), None) => Ok(Arc::new(root)),
        };
        if let Ok(mut docs) = self.docs.lock() {
            docs.retain(|d| &*d.url != url);
            docs.insert(
                0,
                CachedDoc {
                    url: url.into(),
                    text: text.clone(),
                    json: json.clone(),
                    results: Vec::new(),
                },
            );
            docs.truncate(MAX_CACHED_DOCS);
        }
        json
    }

    /// Results of processing `url` (with `text`) that may apply to
    /// `active`, most recent first.
    fn results(
        &self,
        url: &str,
        text: &Arc<str>,
        active: &ActiveContext,
        remote: &[Arc<str>],
        validate: bool,
    ) -> Vec<Processed> {
        let input = Input::of(active);
        let Ok(docs) = self.docs.lock() else {
            return Vec::new();
        };
        let Some(doc) = docs
            .iter()
            .find(|d| &*d.url == url && same_text(&d.text, text))
        else {
            return Vec::new();
        };
        doc.results
            .iter()
            .filter(|p| {
                p.input == input
                    && p.validate == validate
                    && p.remote == remote
                    && (p.input_base == active.base
                        || (!p.used_base
                            && active.base == active.original_base
                            && p.result.as_ref().map_or(true, |r| r.previous.is_none())))
            })
            .cloned()
            .collect()
    }

    fn store(&self, url: &str, text: &Arc<str>, p: Processed) {
        if let Ok(mut docs) = self.docs.lock()
            && let Some(doc) = docs
                .iter_mut()
                .find(|d| &*d.url == url && same_text(&d.text, text))
        {
            doc.results.insert(0, p);
            doc.results.truncate(MAX_RESULTS_PER_DOC);
        }
    }
}

// ---- the processor ----

/// The flags of the Context Processing algorithm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub override_protected: bool,
    pub propagate: bool,
    pub validate_scoped_context: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            override_protected: false,
            propagate: true,
            validate_scoped_context: true,
        }
    }
}

/// Where errors go while reading JSON from another document: the span in
/// the checked document that led there, and that document's URL.
#[derive(Clone, Debug)]
struct Site {
    span: Span,
    url: Arc<str>,
}

#[derive(Clone)]
struct Loaded {
    url: Arc<str>,
    text: Arc<str>,
    json: Result<Arc<Json>, String>,
}

/// Active context id, scoped context, propagate, override protected, base
/// IRI.
type ScopedKey = (u64, usize, bool, bool, Option<Arc<str>>);

/// Runs context processing for one document: remote contexts come from its
/// loader (each URL loaded once per processor) and the shared cache.
pub struct Processor<'l> {
    loader: &'l dyn ContextLoader,
    cache: &'l ContextCache,
    warnings: Vec<SyntaxError>,
    /// Set while reading JSON from another document.
    site: Option<Site>,
    /// No warnings (while applying scoped contexts already validated).
    quiet: bool,
    used_base: bool,
    loads: FxHashMap<Arc<str>, Result<Loaded, LoadError>>,
    /// Every load, in order, for the dependencies of cached results.
    log: Vec<Dep>,
    scoped: FxHashMap<ScopedKey, (Arc<Json>, ActiveContext)>,
    /// The key of the term being defined, without its quotes: where errors
    /// inside its definition go.
    term_at: Option<Span>,
}

/// A context definition's entries, `@import`ed ones merged in.
struct Local<'a> {
    entries: Vec<Entry<'a>>,
    by_key: FxHashMap<&'a str, usize>,
    /// The `@import` value and the imported document.
    import: Option<Site>,
}

struct Entry<'a> {
    key: &'a str,
    key_span: Span,
    value: &'a Json,
    /// From the imported context.
    foreign: bool,
}

impl<'a> Local<'a> {
    fn new(own: &'a Object, import: Option<(&'a Object, Site)>) -> Local<'a> {
        let mut entries = Vec::new();
        let (imported, site) = match import {
            Some((o, s)) => (Some(o), Some(s)),
            None => (None, None),
        };
        let sources = imported
            .into_iter()
            .map(|o| (o, true))
            .chain([(own, false)]);
        for (o, foreign) in sources {
            for m in &o.members {
                if matches!(m.value.value, Value::Invalid) || (foreign && own.get(&m.key).is_some())
                {
                    continue;
                }
                entries.push(Entry {
                    key: &m.key,
                    key_span: m.key_span,
                    value: &m.value,
                    foreign,
                });
            }
        }
        let by_key = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.key, i))
            .collect();
        Local {
            entries,
            by_key,
            import: site,
        }
    }

    fn get(&self, key: &str) -> Option<&Entry<'a>> {
        self.by_key.get(key).map(|&i| &self.entries[i])
    }
}

/// The inputs of Create Term Definition that stay the same for a context
/// definition.
struct Frame<'f> {
    local: &'f Local<'f>,
    base_url: Option<&'f str>,
    protected: bool,
    override_protected: bool,
    remote: &'f [Arc<str>],
}

type Defined<'f> = FxHashMap<&'f str, bool>;

/// A member of an expanded term definition, ignoring unreadable values.
fn member<'a>(o: Option<&'a Object>, key: &str) -> Option<&'a super::json::Member> {
    o?.get(key)
        .filter(|m| !matches!(m.value.value, Value::Invalid))
}

impl<'l> Processor<'l> {
    /// A processor using the process-wide [`ContextCache::global`].
    pub fn new(loader: &'l dyn ContextLoader) -> Processor<'l> {
        Processor::with_cache(loader, ContextCache::global())
    }

    pub fn with_cache(loader: &'l dyn ContextLoader, cache: &'l ContextCache) -> Processor<'l> {
        Processor {
            loader,
            cache,
            warnings: Vec::new(),
            site: None,
            quiet: false,
            used_base: false,
            loads: FxHashMap::default(),
            log: Vec::new(),
            scoped: FxHashMap::default(),
            term_at: None,
        }
    }

    /// The warnings so far (`jsonld-context`, severity warning).
    pub fn take_warnings(&mut self) -> Vec<SyntaxError> {
        std::mem::take(&mut self.warnings)
    }

    /// Context Processing (§4.1.2) of `local`, an `@context` value in the
    /// document, over `active`. `base_url` resolves relative context
    /// references (the document's URL).
    pub fn process(
        &mut self,
        active: &ActiveContext,
        local: &Json,
        base_url: Option<&str>,
        options: Options,
    ) -> Result<ActiveContext, ContextError> {
        let site = self.site.take();
        self.log.clear();
        let r = self.process_in(active, local, base_url, &[], options);
        self.site = site;
        r
    }

    /// Apply a term's scoped context during expansion. A property-scoped
    /// context overrides protected terms and propagates; a type-scoped one
    /// does neither (`propagate` and `override_protected` false). Errors in
    /// a scoped context from a remote document are reported at `at`.
    pub fn process_scoped(
        &mut self,
        active: &ActiveContext,
        scoped: &ScopedContext,
        at: Span,
        propagate: bool,
        override_protected: bool,
    ) -> Result<ActiveContext, ContextError> {
        let key: ScopedKey = (
            active.id,
            Arc::as_ptr(&scoped.json) as usize,
            propagate,
            override_protected,
            active.base.clone(),
        );
        if let Some((_, r)) = self.scoped.get(&key) {
            return Ok(r.clone());
        }
        let site = std::mem::replace(
            &mut self.site,
            scoped.source.as_ref().map(|url| Site {
                span: at,
                url: url.clone(),
            }),
        );
        let quiet = std::mem::replace(&mut self.quiet, true);
        self.log.clear();
        let r = self.process_in(
            active,
            &scoped.json,
            scoped.base_url.as_deref(),
            &[],
            Options {
                override_protected,
                propagate,
                validate_scoped_context: true,
            },
        );
        self.site = site;
        self.quiet = quiet;
        if let Ok(ctx) = &r {
            self.scoped.insert(key, (scoped.json.clone(), ctx.clone()));
        }
        r
    }

    /// An error at `span`, or rather at the term being defined (its key,
    /// without the quotes), or at the reference to the remote document it is
    /// in.
    fn err(&self, code: ErrorCode, span: Span, message: impl Into<String>) -> ContextError {
        let message = message.into();
        let span = self.term_at.unwrap_or(span);
        match &self.site {
            Some(s) => ContextError {
                code,
                span: s.span,
                message: format!("{message} (in {})", s.url),
            },
            None => ContextError {
                code,
                span,
                message,
            },
        }
    }

    fn warn(&mut self, span: Span, message: String) {
        if self.site.is_none() && !self.quiet {
            self.warnings.push(SyntaxError {
                span: self.term_at.unwrap_or(span),
                message,
                severity: Severity::Warning,
                code: DIAGNOSTIC_CODE,
            });
        }
    }

    /// Enter an entry's document: an `@import`ed entry reports at the
    /// `@import` value. Returns the site to restore.
    fn enter(&mut self, local: &Local, e: &Entry) -> Option<Site> {
        let saved = self.site.clone();
        if e.foreign && self.site.is_none() {
            self.site = local.import.clone();
        }
        saved
    }

    fn load(&mut self, url: &str) -> Result<Loaded, LoadError> {
        let r = match self.loads.get(url) {
            Some(r) => r.clone(),
            None => {
                let r = self.loader.load(url).map(|d| Loaded {
                    json: self.cache.parsed(url, &d.text),
                    url: Arc::from(d.url),
                    text: d.text,
                });
                self.loads.insert(Arc::from(url), r.clone());
                r
            }
        };
        self.log.push(Dep {
            url: Arc::from(url),
            text: r.as_ref().map(|l| l.text.clone()).map_err(Clone::clone),
        });
        r
    }

    fn load_failed(&self, span: Span, url: &str, e: &LoadError) -> ContextError {
        let why = match e {
            LoadError::NetworkNotAllowed => format!(
                "{url} is not available offline; enable imports.network (or --network) to fetch it"
            ),
            LoadError::NotFound => format!("{url} was not found"),
            LoadError::Failed(why) => format!("{url}: {why}"),
        };
        self.err(ErrorCode::LoadingRemoteContextFailed, span, why)
    }

    /// A remote document's JSON, or the error for it.
    fn load_json(&mut self, span: Span, url: &str) -> Result<(Loaded, Arc<Json>), ContextError> {
        let loaded = self
            .load(url)
            .map_err(|e| self.load_failed(span, url, &e))?;
        match &loaded.json {
            Ok(json) => {
                let json = json.clone();
                Ok((loaded, json))
            }
            Err(why) => Err(self.err(
                ErrorCode::LoadingRemoteContextFailed,
                span,
                format!("{url} is not JSON: {why}"),
            )),
        }
    }

    /// A context reference resolved against `base_url`.
    fn context_url(
        &self,
        span: Span,
        base_url: Option<&str>,
        s: &str,
    ) -> Result<String, ContextError> {
        match base_url {
            _ if is_absolute_iri(s) => Ok(s.to_string()),
            Some(b) => Ok(resolve_iri(b, s)),
            None => Err(self.err(
                ErrorCode::LoadingRemoteContextFailed,
                span,
                format!("{s} is relative and the document has no base IRI"),
            )),
        }
    }

    /// The Context Processing algorithm.
    fn process_in(
        &mut self,
        active: &ActiveContext,
        local: &Json,
        base_url: Option<&str>,
        remote: &[Arc<str>],
        options: Options,
    ) -> Result<ActiveContext, ContextError> {
        // 1-3
        let mut result = active.clone();
        let mut propagate = options.propagate;
        if let Some(m) = local.as_object().and_then(|o| o.get("@propagate"))
            && let Value::Bool(b) = m.value.value
        {
            propagate = b;
        }
        if !propagate && result.previous.is_none() {
            result.previous = Some(Arc::new(active.clone()));
        }
        // 4-5
        let items = match &local.value {
            Value::Array(a) => a.items.as_slice(),
            _ => std::slice::from_ref(local),
        };
        for item in items {
            match &item.value {
                // Already a `json-syntax` error.
                Value::Invalid => {}
                Value::Null => {
                    if !options.override_protected && result.has_protected() {
                        return Err(self.err(
                            ErrorCode::InvalidContextNullification,
                            item.span,
                            "a context with protected terms cannot be set to null",
                        ));
                    }
                    let previous = result.previous.clone();
                    result = ActiveContext::reset(result.original_base.clone());
                    if !propagate {
                        result.previous = previous;
                    }
                }
                Value::String(s) => {
                    let at = unquoted(item.span);
                    let url: Arc<str> = Arc::from(self.context_url(at, base_url, s)?);
                    if !options.validate_scoped_context && remote.contains(&url) {
                        continue;
                    }
                    if remote.len() >= MAX_REMOTE_CONTEXTS {
                        return Err(self.err(
                            ErrorCode::ContextOverflow,
                            at,
                            format!(
                                "more than {MAX_REMOTE_CONTEXTS} nested remote contexts (does a context include itself?)"
                            ),
                        ));
                    }
                    let mut remote = remote.to_vec();
                    remote.push(url.clone());
                    result =
                        self.remote(&result, &url, at, &remote, options.validate_scoped_context)?;
                }
                Value::Object(o) => {
                    self.definition(&mut result, o, base_url, remote, options)?;
                }
                _ => {
                    return Err(self.err(
                        ErrorCode::InvalidLocalContext,
                        item.span,
                        "a context must be an object, a string, null or an array of them",
                    ));
                }
            }
        }
        result.terms.compact();
        result.id = next_id();
        Ok(result)
    }

    /// Step 5.2: a remote context, from the cache when it holds.
    fn remote(
        &mut self,
        active: &ActiveContext,
        url: &Arc<str>,
        span: Span,
        remote: &[Arc<str>],
        validate: bool,
    ) -> Result<ActiveContext, ContextError> {
        let (loaded, json) = self.load_json(span, url)?;
        let site_span = self
            .site
            .as_ref()
            .map_or(self.term_at.unwrap_or(span), |s| s.span);
        'cached: for p in self
            .cache
            .results(url, &loaded.text, active, remote, validate)
        {
            for d in &p.deps {
                let now = self.load(&d.url);
                let same = match (&now, &d.text) {
                    (Ok(l), Ok(t)) => same_text(&l.text, t),
                    (Err(a), Err(b)) => a == b,
                    _ => false,
                };
                if !same {
                    continue 'cached;
                }
            }
            self.cache.hits.fetch_add(1, Ordering::Relaxed);
            self.used_base |= p.used_base;
            return match p.result {
                Ok(mut ctx) => {
                    if ctx.base != active.base {
                        ctx.base = active.base.clone();
                        ctx.original_base = active.original_base.clone();
                    }
                    Ok(ctx)
                }
                Err((code, message)) => Err(ContextError {
                    code,
                    span: site_span,
                    message,
                }),
            };
        }
        self.cache.misses.fetch_add(1, Ordering::Relaxed);
        let Some(context) = json.as_object().and_then(|o| o.get("@context")) else {
            return Err(self.err(
                ErrorCode::InvalidRemoteContext,
                span,
                format!("{url} has no top-level @context"),
            ));
        };
        let site = self.site.replace(Site {
            span: site_span,
            url: loaded.url.clone(),
        });
        let used_base = std::mem::replace(&mut self.used_base, false);
        let start = self.log.len();
        let r = self.process_in(
            active,
            &context.value,
            Some(&loaded.url),
            remote,
            Options {
                validate_scoped_context: validate,
                ..Options::default()
            },
        );
        self.site = site;
        let used = self.used_base;
        self.used_base |= used_base;
        let mut deps: Vec<Dep> = Vec::new();
        for d in &self.log[start..] {
            if !deps.contains(d) {
                deps.push(d.clone());
            }
        }
        self.cache.store(
            url,
            &loaded.text,
            Processed {
                input: Input::of(active),
                input_base: active.base.clone(),
                remote: remote.to_vec(),
                validate,
                used_base: used,
                deps,
                result: match &r {
                    Ok(ctx) => Ok(ctx.clone()),
                    Err(e) => Err((e.code, e.message.clone())),
                },
            },
        );
        r
    }

    /// Step 5.6: the context an `@import` names, and where its errors go.
    fn import(
        &mut self,
        m: &super::json::Member,
        base_url: Option<&str>,
    ) -> Result<(Arc<Json>, Site), ContextError> {
        let Some(s) = m.value.as_str() else {
            return Err(self.err(
                ErrorCode::InvalidImportValue,
                m.value.span,
                "@import must be a string",
            ));
        };
        let at = unquoted(m.value.span);
        let url = self.context_url(at, base_url, s)?;
        let (loaded, json) = self.load_json(at, &url)?;
        let context = json
            .as_object()
            .and_then(|d| d.get("@context"))
            .and_then(|c| c.value.as_object());
        let Some(context) = context else {
            return Err(self.err(
                ErrorCode::InvalidRemoteContext,
                at,
                format!("{url} has no top-level @context object to import"),
            ));
        };
        if context.get("@import").is_some() {
            return Err(self.err(
                ErrorCode::InvalidContextEntry,
                at,
                format!("{url} has an @import itself, which an imported context may not have"),
            ));
        }
        let site = Site {
            span: self
                .site
                .as_ref()
                .map_or(self.term_at.unwrap_or(at), |s| s.span),
            url: loaded.url.clone(),
        };
        Ok((json, site))
    }

    /// Steps 5.5 to 5.13: a context definition.
    fn definition(
        &mut self,
        result: &mut ActiveContext,
        o: &Object,
        base_url: Option<&str>,
        remote: &[Arc<str>],
        options: Options,
    ) -> Result<(), ContextError> {
        // 5.5
        if let Some(m) = member(Some(o), "@version")
            && !matches!(&m.value.value, Value::Number(n) if n.parse::<f64>() == Ok(1.1))
        {
            return Err(self.err(
                ErrorCode::InvalidVersionValue,
                m.value.span,
                "@version must be the number 1.1",
            ));
        }
        // 5.6
        let imported = match member(Some(o), "@import") {
            Some(m) => Some(self.import(m, base_url)?),
            None => None,
        };
        let import = imported.as_ref().and_then(|(json, site)| {
            let ctx = json.as_object()?.get("@context")?.value.as_object()?;
            Some((ctx, site.clone()))
        });
        let local = Local::new(o, import);

        // 5.7
        if let Some(e) = local.get("@base")
            && remote.is_empty()
        {
            let saved = self.enter(&local, e);
            let r = match &e.value.value {
                Value::Null => Ok(None),
                Value::String(s) if is_absolute_iri(s) => Ok(Some(Arc::from(&**s))),
                Value::String(s) => match &result.base {
                    Some(b) => Ok(Some(Arc::from(resolve_iri(b, s)))),
                    None => Err(self.err(
                        ErrorCode::InvalidBaseIri,
                        unquoted(e.value.span),
                        "a relative @base needs a base IRI to resolve against",
                    )),
                },
                _ => Err(self.err(
                    ErrorCode::InvalidBaseIri,
                    e.value.span,
                    "@base must be an IRI or null",
                )),
            };
            self.site = saved;
            result.base = r?;
        }
        let mut defined = Defined::default();
        let frame = Frame {
            local: &local,
            base_url,
            protected: matches!(
                local.get("@protected").map(|e| &e.value.value),
                Some(Value::Bool(true))
            ),
            override_protected: options.override_protected,
            remote,
        };
        // 5.8
        if let Some(e) = local.get("@vocab") {
            let saved = self.enter(&local, e);
            let r = match &e.value.value {
                Value::Null => Ok(None),
                Value::String(s) => {
                    match self.expand(result, &frame, &mut defined, s, true, true) {
                        Ok(Some(v)) if is_absolute_iri(&v) || is_blank_node(&v) => {
                            Ok(Some(Arc::from(v)))
                        }
                        Ok(_) => Err(self.err(
                            ErrorCode::InvalidVocabMapping,
                            unquoted(e.value.span),
                            format!("@vocab \"{s}\" is not an IRI or a blank node identifier"),
                        )),
                        Err(e) => Err(e),
                    }
                }
                _ => Err(self.err(
                    ErrorCode::InvalidVocabMapping,
                    e.value.span,
                    "@vocab must be an IRI, a blank node identifier or null",
                )),
            };
            self.site = saved;
            result.vocab = r?;
        }
        // 5.9
        if let Some(e) = local.get("@language") {
            let saved = self.enter(&local, e);
            let r = match &e.value.value {
                Value::Null => Ok(None),
                Value::String(s) => Ok(Some(Arc::from(&**s))),
                _ => Err(self.err(
                    ErrorCode::InvalidDefaultLanguage,
                    e.value.span,
                    "@language must be a language tag or null",
                )),
            };
            self.site = saved;
            result.language = r?;
        }
        // 5.10
        if let Some(e) = local.get("@direction") {
            let saved = self.enter(&local, e);
            let r = direction(e.value).ok_or_else(|| {
                self.err(
                    ErrorCode::InvalidBaseDirection,
                    e.value.span,
                    "@direction must be \"ltr\", \"rtl\" or null",
                )
            });
            self.site = saved;
            result.direction = r?;
        }
        // 5.11, and the same check for the context's @protected.
        for (key, code) in [
            ("@propagate", ErrorCode::InvalidPropagateValue),
            ("@protected", ErrorCode::InvalidProtectedValue),
        ] {
            if let Some(e) = local.get(key)
                && !matches!(e.value.value, Value::Bool(_))
            {
                let saved = self.enter(&local, e);
                let err = self.err(code, e.value.span, format!("{key} must be true or false"));
                self.site = saved;
                return Err(err);
            }
        }
        // 5.13
        for (i, e) in local.entries.iter().enumerate() {
            if local.by_key[e.key] != i || CONTEXT_KEYWORDS.contains(&e.key) {
                continue;
            }
            self.define(result, &frame, e.key, &mut defined)?;
        }
        Ok(())
    }

    /// IRI Expansion (§4.3) during context processing, creating the term
    /// definitions it depends on.
    fn expand<'f>(
        &mut self,
        result: &mut ActiveContext,
        f: &Frame<'f>,
        defined: &mut Defined<'f>,
        value: &str,
        document_relative: bool,
        vocab: bool,
    ) -> Result<Option<String>, ContextError> {
        // 1-2
        if is_keyword(value) {
            return Ok(Some(value.to_string()));
        }
        if has_keyword_form(value) {
            return Ok(None);
        }
        // 3
        if let Some(e) = f.local.get(value)
            && defined.get(e.key) != Some(&true)
        {
            self.define(result, f, e.key, defined)?;
        }
        // 6.3, ahead of 4-5: every local entry is defined by the end anyway.
        // Not for what 6.2 returns as is: a blank node identifier, or an IRI
        // whose suffix starts with `//`.
        if let Some(colon) = value.find(':').filter(|&i| i > 0)
            && &value[..colon] != "_"
            && !value[colon + 1..].starts_with("//")
            && let Some(e) = f.local.get(&value[..colon])
            && defined.get(e.key) != Some(&true)
        {
            self.define(result, f, e.key, defined)?;
        }
        Ok(expand_from_step_4(
            result,
            value,
            document_relative,
            vocab,
            &mut self.used_base,
        ))
    }

    /// Create Term Definition (§4.2.2) for `term`, an entry of the local
    /// context.
    fn define<'f>(
        &mut self,
        result: &mut ActiveContext,
        f: &Frame<'f>,
        term: &'f str,
        defined: &mut Defined<'f>,
    ) -> Result<(), ContextError> {
        let Some(e) = f.local.get(term) else {
            return Ok(());
        };
        let saved = self.enter(f.local, e);
        let term_at = self.term_at.replace(unquoted(e.key_span));
        let r = self.define_entry(result, f, e, defined);
        self.term_at = term_at;
        self.site = saved;
        r
    }

    fn define_entry<'f>(
        &mut self,
        result: &mut ActiveContext,
        f: &Frame<'f>,
        e: &Entry<'f>,
        defined: &mut Defined<'f>,
    ) -> Result<(), ContextError> {
        use ErrorCode::*;
        let term = e.key;
        // 1
        match defined.get(term) {
            Some(true) => return Ok(()),
            Some(false) => {
                return Err(self.err(
                    CyclicIriMapping,
                    e.key_span,
                    format!("\"{term}\" is defined in terms of itself"),
                ));
            }
            None => {}
        }
        // 2
        if term.is_empty() {
            return Err(self.err(
                InvalidTermDefinition,
                e.key_span,
                "a term cannot be the empty string",
            ));
        }
        defined.insert(term, false);
        let value = e.value;
        let obj = value.as_object();
        // 4-5
        if term == "@type" {
            let ok = obj.is_some_and(|o| {
                !o.members.is_empty()
                    && o.members.iter().all(|m| match &*m.key {
                        "@container" => m.value.as_str() == Some("@set"),
                        "@protected" => true,
                        _ => false,
                    })
            });
            if !ok {
                return Err(self.err(
                    KeywordRedefinition,
                    e.key_span,
                    "@type may only be given {\"@container\": \"@set\"} and @protected",
                ));
            }
        } else if is_keyword(term) {
            return Err(self.err(
                KeywordRedefinition,
                e.key_span,
                format!("{term} is a keyword and cannot be redefined"),
            ));
        } else if has_keyword_form(term) {
            self.warn(
                e.key_span,
                format!("\"{term}\" looks like a keyword, so it is ignored as a term"),
            );
            return Ok(());
        }
        // 6
        let previous = result.terms.remove(term);
        // 7-9
        let simple = match &value.value {
            Value::Null | Value::String(_) => true,
            Value::Object(_) => false,
            _ => {
                return Err(self.err(
                    InvalidTermDefinition,
                    value.span,
                    "a term definition must be a string, an object or null",
                ));
            }
        };
        let id_value: Option<&Json> = match &value.value {
            Value::Object(_) => member(obj, "@id").map(|m| &m.value),
            _ => Some(value),
        };
        // 10
        let mut def = TermDefinition {
            protected: f.protected,
            span: self.site.is_none().then_some(unquoted(e.key_span)),
            ..TermDefinition::default()
        };
        // 11
        if let Some(m) = member(obj, "@protected") {
            match m.value.value {
                Value::Bool(b) => def.protected = b,
                _ => {
                    return Err(self.err(
                        InvalidProtectedValue,
                        m.value.span,
                        "@protected must be true or false",
                    ));
                }
            }
        }
        // 12
        let type_member = member(obj, "@type");
        if let Some(m) = type_member {
            let Some(t) = m.value.as_str() else {
                return Err(self.err(InvalidTypeMapping, m.value.span, "@type must be a string"));
            };
            match self.expand(result, f, defined, t, false, true)? {
                Some(t)
                    if matches!(t.as_str(), "@id" | "@json" | "@none" | "@vocab")
                        || is_absolute_iri(&t) =>
                {
                    def.type_mapping = Some(Arc::from(t));
                }
                _ => {
                    return Err(self.err(
                        InvalidTypeMapping,
                        m.value.span,
                        "@type must be @id, @json, @none, @vocab or an IRI",
                    ));
                }
            }
        }
        // 13
        if let Some(m) = member(obj, "@reverse") {
            if let Some(other) = member(obj, "@id").or_else(|| member(obj, "@nest")) {
                return Err(self.err(
                    InvalidReverseProperty,
                    other.key_span,
                    format!("a reverse property cannot have {}", other.key),
                ));
            }
            let Some(r) = m.value.as_str() else {
                return Err(self.err(InvalidIriMapping, m.value.span, "@reverse must be a string"));
            };
            if has_keyword_form(r) {
                self.warn(
                    m.value.span,
                    format!("\"{r}\" looks like a keyword, so the term is ignored"),
                );
                return Ok(());
            }
            match self.expand(result, f, defined, r, false, true)? {
                Some(iri) if is_absolute_iri(&iri) || is_blank_node(&iri) => {
                    def.iri = Some(Arc::from(iri));
                }
                _ => {
                    return Err(self.err(
                        InvalidIriMapping,
                        m.value.span,
                        format!("@reverse \"{r}\" is not an IRI or a blank node identifier"),
                    ));
                }
            }
            if let Some(c) = member(obj, "@container") {
                def.container = match &c.value.value {
                    Value::Null => Container::default(),
                    Value::String(s) if &**s == "@set" => Container::SET,
                    Value::String(s) if &**s == "@index" => Container::INDEX,
                    _ => {
                        return Err(self.err(
                            InvalidReverseProperty,
                            c.value.span,
                            "a reverse property's @container must be @set, @index or null",
                        ));
                    }
                };
            }
            def.reverse = true;
            // Not in the spec's step 13, but implementations (and the
            // expand test 0131) keep a reverse term's property index.
            self.index(result, f, defined, obj, &mut def)?;
            return self.finish(result, f, e, defined, previous, def);
        }
        // 14
        if let Some(id) = id_value
            && id.as_str() != Some(term)
        {
            match &id.value {
                Value::Null => {}
                Value::String(s) => {
                    if !is_keyword(s) && has_keyword_form(s) {
                        self.warn(
                            id.span,
                            format!("\"{s}\" looks like a keyword, so the term is ignored"),
                        );
                        return Ok(());
                    }
                    let iri = self.expand(result, f, defined, s, false, true)?;
                    let Some(iri) =
                        iri.filter(|i| is_keyword(i) || is_absolute_iri(i) || is_blank_node(i))
                    else {
                        return Err(self.err(
                            InvalidIriMapping,
                            id.span,
                            format!(
                                "\"{s}\" does not expand to an IRI, a blank node identifier or a keyword"
                            ),
                        ));
                    };
                    if iri == "@context" {
                        return Err(self.err(
                            InvalidKeywordAlias,
                            id.span,
                            "@context cannot be aliased",
                        ));
                    }
                    let inner_colon = term.find(':').is_some_and(|i| i > 0 && i + 1 < term.len());
                    if inner_colon || term.contains('/') {
                        defined.insert(term, true);
                        let own = self.expand(result, f, defined, term, false, true);
                        defined.insert(term, false);
                        if own?.as_deref() != Some(iri.as_str()) {
                            return Err(self.err(
                                InvalidIriMapping,
                                id.span,
                                format!(
                                    "\"{term}\" is itself an IRI or compact IRI, and @id cannot map it to another IRI"
                                ),
                            ));
                        }
                    }
                    if !term.contains([':', '/'])
                        && simple
                        && (is_blank_node(&iri)
                            || iri.ends_with([':', '/', '?', '#', '[', ']', '@']))
                    {
                        def.prefix = true;
                    }
                    def.iri = Some(Arc::from(iri));
                }
                _ => {
                    return Err(self.err(
                        InvalidIriMapping,
                        id.span,
                        "@id must be a string or null",
                    ));
                }
            }
        } else if let Some(colon) = term.find(':').filter(|&i| i > 0) {
            // 15
            let (prefix, suffix) = (&term[..colon], &term[colon + 1..]);
            if f.local.get(prefix).is_some() {
                self.define(result, f, prefix, defined)?;
            }
            def.iri = Some(
                match result.terms.get(prefix).and_then(|d| d.iri.as_deref()) {
                    Some(p) => Arc::from(format!("{p}{suffix}")),
                    None => Arc::from(term),
                },
            );
        } else if term.contains('/') {
            // 16
            match expand_from_step_4(result, term, true, true, &mut self.used_base) {
                Some(i) if is_absolute_iri(&i) => def.iri = Some(Arc::from(i)),
                _ => {
                    return Err(self.err(
                        InvalidIriMapping,
                        e.key_span,
                        format!("\"{term}\" does not expand to an IRI"),
                    ));
                }
            }
        } else if term == "@type" {
            // 17
            def.iri = Some(Arc::from("@type"));
        } else if let Some(v) = &result.vocab {
            // 18
            def.iri = Some(Arc::from(format!("{v}{term}")));
        } else {
            return Err(self.err(
                InvalidIriMapping,
                e.key_span,
                format!("\"{term}\" has no @id and there is no @vocab to map it with"),
            ));
        }
        // 19
        if let Some(m) = member(obj, "@container") {
            let Some(c) = container(&m.value) else {
                return Err(self.err(
                    InvalidContainerMapping,
                    m.value.span,
                    "not a valid @container: use @list, @set, @index, @language, @graph, @id, @type or an allowed combination",
                ));
            };
            def.container = c;
            if c.contains(Container::TYPE) {
                let t = def.type_mapping.get_or_insert_with(|| Arc::from("@id"));
                if !matches!(&**t, "@id" | "@vocab") {
                    let span = type_member.map_or(m.value.span, |t| t.value.span);
                    return Err(self.err(
                        InvalidTypeMapping,
                        span,
                        "a @type container needs @type @id or @vocab",
                    ));
                }
            }
        }
        // 20
        self.index(result, f, defined, obj, &mut def)?;
        // 21
        if let Some(m) = member(obj, "@context") {
            let checked = self.process_in(
                result,
                &m.value,
                f.base_url,
                f.remote,
                Options {
                    override_protected: true,
                    propagate: true,
                    validate_scoped_context: false,
                },
            );
            if let Err(inner) = checked {
                return Err(match inner.code {
                    InvalidScopedContext => inner,
                    code => ContextError {
                        code: InvalidScopedContext,
                        span: inner.span,
                        message: format!("{code}: {}", inner.message),
                    },
                });
            }
            def.context = Some(ScopedContext {
                json: Arc::new(m.value.clone()),
                base_url: f.base_url.map(Arc::from),
                source: self.site.as_ref().map(|s| s.url.clone()),
            });
        }
        // 22
        if let Some(m) = member(obj, "@language")
            && type_member.is_none()
        {
            def.language = Some(match &m.value.value {
                Value::Null => None,
                Value::String(s) => Some(Arc::from(&**s)),
                _ => {
                    return Err(self.err(
                        InvalidLanguageMapping,
                        m.value.span,
                        "@language must be a language tag or null",
                    ));
                }
            });
        }
        // 23
        if let Some(m) = member(obj, "@direction")
            && type_member.is_none()
        {
            let Some(d) = direction(&m.value) else {
                return Err(self.err(
                    InvalidBaseDirection,
                    m.value.span,
                    "@direction must be \"ltr\", \"rtl\" or null",
                ));
            };
            def.direction = Some(d);
        }
        // 24
        if let Some(m) = member(obj, "@nest") {
            match m.value.as_str() {
                Some(n) if n == "@nest" || !is_keyword(n) => def.nest = Some(Arc::from(n)),
                _ => {
                    return Err(self.err(
                        InvalidNestValue,
                        m.value.span,
                        "@nest must be a term or @nest",
                    ));
                }
            }
        }
        // 25
        if let Some(m) = member(obj, "@prefix") {
            if term.contains([':', '/']) {
                return Err(self.err(
                    InvalidTermDefinition,
                    m.key_span,
                    "a compact IRI or IRI term cannot have @prefix",
                ));
            }
            let Value::Bool(p) = m.value.value else {
                return Err(self.err(
                    InvalidPrefixValue,
                    m.value.span,
                    "@prefix must be true or false",
                ));
            };
            def.prefix = p;
            if p && def.iri.as_deref().is_some_and(is_keyword) {
                return Err(self.err(
                    InvalidTermDefinition,
                    m.value.span,
                    "a keyword alias cannot be a prefix",
                ));
            }
        }
        // 26
        if let Some(m) =
            obj.and_then(|o| o.members.iter().find(|m| !TERM_KEYWORDS.contains(&&*m.key)))
        {
            return Err(self.err(
                InvalidTermDefinition,
                m.key_span,
                format!("{} is not allowed in a term definition", m.key),
            ));
        }
        self.finish(result, f, e, defined, previous, def)
    }

    /// Step 20: a property index (`@index`, with an `@index` container).
    fn index<'f>(
        &mut self,
        result: &mut ActiveContext,
        f: &Frame<'f>,
        defined: &mut Defined<'f>,
        obj: Option<&Object>,
        def: &mut TermDefinition,
    ) -> Result<(), ContextError> {
        use ErrorCode::*;
        if let Some(m) = member(obj, "@index") {
            if !def.container.contains(Container::INDEX) {
                return Err(self.err(
                    InvalidTermDefinition,
                    m.key_span,
                    "@index needs an @index container",
                ));
            }
            let index = match m.value.as_str() {
                Some(s) => self.expand(result, f, defined, s, false, true)?,
                None => None,
            };
            if !index.as_deref().is_some_and(is_absolute_iri) {
                return Err(self.err(
                    InvalidTermDefinition,
                    m.value.span,
                    "@index must be a property that expands to an IRI",
                ));
            }
            def.index = m.value.as_str().map(Arc::from);
        }
        Ok(())
    }

    /// Steps 27-28: check a protected definition, then store. (The spec
    /// returns before step 27 for a reverse property; a protected one is
    /// checked here too.)
    fn finish<'f>(
        &mut self,
        result: &mut ActiveContext,
        f: &Frame<'f>,
        e: &Entry<'f>,
        defined: &mut Defined<'f>,
        previous: Option<Arc<TermDefinition>>,
        def: TermDefinition,
    ) -> Result<(), ContextError> {
        let def = match previous {
            Some(p) if p.protected && !f.override_protected => {
                if !def.same_as(&p) {
                    return Err(self.err(
                        ErrorCode::ProtectedTermRedefinition,
                        e.key_span,
                        format!("\"{}\" is protected and cannot be redefined", e.key),
                    ));
                }
                p
            }
            _ => Arc::new(def),
        };
        result.terms.set(e.key, Some(def));
        defined.insert(e.key, true);
        Ok(())
    }
}

/// A `@direction` value: `Some(None)` for null.
fn direction(v: &Json) -> Option<Option<Direction>> {
    match &v.value {
        Value::Null => Some(None),
        Value::String(s) if &**s == "ltr" => Some(Some(Direction::Ltr)),
        Value::String(s) if &**s == "rtl" => Some(Some(Direction::Rtl)),
        _ => None,
    }
}

/// A valid `@container` value (§4.2.2 step 19.1).
fn container(v: &Json) -> Option<Container> {
    let items = match &v.value {
        Value::Null => return Some(Container::default()),
        Value::String(_) => std::slice::from_ref(v),
        Value::Array(a) => a.items.as_slice(),
        _ => return None,
    };
    let mut c = Container::default();
    for item in items {
        c = c.with(Container::keyword(item.as_str()?)?);
    }
    let set = c.contains(Container::SET);
    let ok = if c.contains(Container::LIST) {
        c == Container::LIST
    } else if c.contains(Container::GRAPH) {
        let rest = Container(c.0 & !(Container::GRAPH.0 | Container::SET.0));
        rest.is_empty() || rest == Container::ID || rest == Container::INDEX
    } else {
        c.count() <= if set { 2 } else { 1 }
    };
    ok.then_some(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonld::NoNetwork;
    use crate::jsonld::json;

    const BASE: &str = "http://example.com/doc.jsonld";

    fn root(text: &str) -> Json {
        let doc = json::parse(text);
        assert!(doc.errors.is_empty(), "{:?}", doc.errors);
        doc.root.unwrap()
    }

    fn context_of(doc: &Json) -> &Json {
        &doc.as_object().unwrap().get("@context").unwrap().value
    }

    /// Process the `@context` of `text` with a private cache.
    fn process_with(text: &str, loader: &dyn ContextLoader) -> Result<ActiveContext, ContextError> {
        let cache = ContextCache::new();
        let doc = root(text);
        Processor::with_cache(loader, &cache).process(
            &ActiveContext::new(Some(BASE)),
            context_of(&doc),
            Some(BASE),
            Options::default(),
        )
    }

    fn process(text: &str) -> Result<ActiveContext, ContextError> {
        process_with(text, &NoNetwork::default())
    }

    fn ok(text: &str) -> ActiveContext {
        process(text).unwrap_or_else(|e| panic!("{e}"))
    }

    fn iri<'a>(c: &'a ActiveContext, term: &str) -> Option<&'a str> {
        c.term(term).and_then(|d| d.iri.as_deref())
    }

    fn spanned(text: &str, span: Span) -> String {
        text[span.start as usize..span.end as usize].to_string()
    }

    /// The error's code and the text its span covers.
    fn fails(text: &str) -> (ErrorCode, String) {
        fails_with(text, &NoNetwork::default())
    }

    fn fails_with(text: &str, loader: &dyn ContextLoader) -> (ErrorCode, String) {
        let e = process_with(text, loader).expect_err("an error");
        (e.code, spanned(text, e.span))
    }

    #[test]
    fn simple_and_expanded_term_definitions() {
        let c = ok(r#"{"@context": {
            "ex": "http://ex.org/",
            "name": {"@id": "http://ex.org/name", "@type": "@id", "@container": "@set"},
            "label": {"@id": "ex:label", "@language": "en", "@direction": "rtl"},
            "nothing": null
        }}"#);
        assert_eq!(iri(&c, "ex"), Some("http://ex.org/"));
        assert!(c.term("ex").unwrap().prefix);
        let name = c.term("name").unwrap();
        assert_eq!(name.type_mapping.as_deref(), Some("@id"));
        assert_eq!(name.container, Container::SET);
        assert!(!name.prefix);
        let label = c.term("label").unwrap();
        assert_eq!(label.iri.as_deref(), Some("http://ex.org/label"));
        assert_eq!(label.language, Some(Some(Arc::from("en"))));
        assert_eq!(label.direction, Some(Some(Direction::Rtl)));
        assert_eq!(c.term("nothing").unwrap().iri, None);
        assert_eq!(expand_iri(&c, "nothing", false, true), None);
    }

    #[test]
    fn term_spans_point_at_their_keys() {
        let text = r#"{"@context": {"ex": "http://ex.org/"}}"#;
        let c = ok(text);
        assert_eq!(spanned(text, c.term("ex").unwrap().span.unwrap()), "ex");
    }

    #[test]
    fn compact_iris_and_prefix_flags() {
        let c = ok(r#"{"@context": {
            "ex": "http://ex.org/",
            "noprefix": {"@id": "http://np.org/"},
            "forced": {"@id": "http://f.org/", "@prefix": true},
            "nodelim": "http://nd.org/x",
            "ex:thing": {"@type": "@id"},
            "bnode": "_:b"
        }}"#);
        let x = |v: &str| expand_iri(&c, v, false, true);
        assert_eq!(x("ex:a").as_deref(), Some("http://ex.org/a"));
        // Only prefix terms make compact IRIs.
        assert_eq!(x("noprefix:a").as_deref(), Some("noprefix:a"));
        assert_eq!(x("forced:a").as_deref(), Some("http://f.org/a"));
        assert_eq!(x("nodelim:a").as_deref(), Some("nodelim:a"));
        assert!(c.term("bnode").unwrap().prefix);
        // A compact-IRI term takes its IRI from its prefix.
        assert_eq!(iri(&c, "ex:thing"), Some("http://ex.org/thing"));
        // Blank nodes and IRIs with an authority are left alone.
        assert_eq!(x("_:x").as_deref(), Some("_:x"));
        assert_eq!(x("ex://a").as_deref(), Some("ex://a"));
    }

    #[test]
    fn definitions_may_depend_on_later_ones() {
        let c = ok(r#"{"@context": {"name": "foaf:name", "foaf": "http://xmlns.com/foaf/0.1/"}}"#);
        assert_eq!(iri(&c, "name"), Some("http://xmlns.com/foaf/0.1/name"));
    }

    #[test]
    fn vocab_and_base() {
        let c = ok(
            r#"{"@context": {"@vocab": "http://v.org/", "@base": "http://b.org/dir/", "term": {}}}"#,
        );
        assert_eq!(c.vocab(), Some("http://v.org/"));
        assert_eq!(c.base(), Some("http://b.org/dir/"));
        assert_eq!(c.original_base(), Some(BASE));
        assert_eq!(iri(&c, "term"), Some("http://v.org/term"));
        assert_eq!(
            expand_iri(&c, "x", false, true).as_deref(),
            Some("http://v.org/x")
        );
        assert_eq!(
            expand_iri(&c, "../x", true, false).as_deref(),
            Some("http://b.org/x")
        );
        assert_eq!(expand_iri(&c, "x", false, false).as_deref(), Some("x"));
        // A relative @vocab resolves against the base IRI, or concatenates
        // to the current @vocab.
        assert_eq!(
            ok(r#"{"@context": {"@vocab": "/v#"}}"#).vocab(),
            Some("http://example.com/v#")
        );
        let c = ok(r#"{"@context": [{"@vocab": "http://v.org/"}, {"@vocab": "sub/"}]}"#);
        assert_eq!(c.vocab(), Some("http://v.org/sub/"));
        assert_eq!(ok(r#"{"@context": {"@vocab": "_:"}}"#).vocab(), Some("_:"));
        // A relative @base resolves; null removes it.
        let c = ok(r#"{"@context": {"@base": "other/"}}"#);
        assert_eq!(c.base(), Some("http://example.com/other/"));
        assert_eq!(ok(r#"{"@context": {"@base": null}}"#).base(), None);
    }

    #[test]
    fn language_and_direction_defaults() {
        let c = ok(r#"{"@context": {"@language": "de", "@direction": "ltr"}}"#);
        assert_eq!(c.default_language(), Some("de"));
        assert_eq!(c.default_direction(), Some(Direction::Ltr));
        let c = ok(r#"{"@context": [{"@language": "de"}, {"@language": null}]}"#);
        assert_eq!(c.default_language(), None);
    }

    #[test]
    fn keyword_aliases() {
        let c =
            ok(r#"{"@context": {"id": "@id", "type": "@type", "@type": {"@container": "@set"}}}"#);
        assert_eq!(expand_iri(&c, "id", false, true).as_deref(), Some("@id"));
        assert_eq!(
            expand_iri(&c, "type", false, false).as_deref(),
            Some("@type")
        );
        assert_eq!(
            expand_iri(&c, "@type", false, true).as_deref(),
            Some("@type")
        );
        assert_eq!(c.term("@type").unwrap().container, Container::SET);
    }

    #[test]
    fn keyword_like_terms_and_ids_are_ignored_with_a_warning() {
        let cache = ContextCache::new();
        let text = r#"{"@context": {"@ignoreMe": "http://x.org/", "t": {"@id": "@ignored"}, "r": {"@reverse": "@nope"}}}"#;
        let doc = root(text);
        let loader = NoNetwork::default();
        let mut p = Processor::with_cache(&loader, &cache);
        let c = p
            .process(
                &ActiveContext::new(None),
                context_of(&doc),
                None,
                Options::default(),
            )
            .unwrap();
        assert!(c.term("@ignoreMe").is_none() && c.term("t").is_none() && c.term("r").is_none());
        let w: Vec<String> = p
            .take_warnings()
            .iter()
            .inspect(|w| assert_eq!((w.code, w.severity), (DIAGNOSTIC_CODE, Severity::Warning)))
            .map(|w| spanned(text, w.span))
            .collect();
        assert_eq!(w, ["@ignoreMe", "t", "r"]);
        assert_eq!(expand_iri(&c, "@ignoreMe", false, true), None);
    }

    #[test]
    fn null_resets_and_keeps_the_original_base() {
        let c = ok(
            r#"{"@context": [{"@base": "http://b.org/", "@vocab": "http://v.org/", "a": "http://a.org/"}, null]}"#,
        );
        assert!(c.term("a").is_none() && c.vocab().is_none());
        assert_eq!(c.base(), Some(BASE));
    }

    #[test]
    fn protected_terms() {
        // The same definition again is fine; a different one is an error at
        // the key.
        ok(r#"{"@context": [{"@protected": true, "a": "http://a.org/"}, {"a": "http://a.org/"}]}"#);
        let text =
            r#"{"@context": [{"@protected": true, "a": "http://a.org/"}, {"a": "http://b.org/"}]}"#;
        assert_eq!(
            fails(text),
            (ErrorCode::ProtectedTermRedefinition, "a".into())
        );
        // A term's @protected overrides the context's.
        ok(
            r#"{"@context": [{"@protected": true, "a": {"@id": "http://a.org/", "@protected": false}}, {"a": "http://b.org/"}]}"#,
        );
        let c = ok(r#"{"@context": {"a": {"@id": "http://a.org/", "@protected": true}}}"#);
        assert!(c.term("a").unwrap().protected && c.has_protected());
        // Null cannot clear protected terms, unless overriding.
        let text = r#"{"@context": [{"@protected": true, "a": "http://a.org/"}, null]}"#;
        assert_eq!(
            fails(text),
            (ErrorCode::InvalidContextNullification, "null".into())
        );
        let doc = root(r#"{"@context": [{"@protected": true, "a": "http://a.org/"}]}"#);
        let cache = ContextCache::new();
        let loader = NoNetwork::default();
        let mut p = Processor::with_cache(&loader, &cache);
        let c = p
            .process(
                &ActiveContext::new(None),
                context_of(&doc),
                None,
                Options::default(),
            )
            .unwrap();
        let over = Options {
            override_protected: true,
            ..Options::default()
        };
        assert!(
            p.process(&c, &root("null"), None, over)
                .unwrap()
                .term("a")
                .is_none()
        );
        let redefine = root(r#"{"a": "http://b.org/"}"#);
        let c2 = p.process(&c, &redefine, None, over).unwrap();
        assert_eq!(iri(&c2, "a"), Some("http://b.org/"));
    }

    #[test]
    fn propagate_false_keeps_the_previous_context() {
        let cache = ContextCache::new();
        let loader = NoNetwork::default();
        let mut p = Processor::with_cache(&loader, &cache);
        let start = ActiveContext::new(None);
        let doc = root(r#"{"@propagate": false, "a": "http://a.org/"}"#);
        let c = p.process(&start, &doc, None, Options::default()).unwrap();
        assert_eq!(iri(&c, "a"), Some("http://a.org/"));
        assert!(c.previous().unwrap().term("a").is_none());
        // `propagate = false` (a type-scoped context) does the same, and an
        // existing previous context stays.
        let doc = root(r#"{"b": "http://b.org/"}"#);
        let opts = Options {
            propagate: false,
            ..Options::default()
        };
        let c2 = p.process(&c, &doc, None, opts).unwrap();
        assert!(c2.previous().unwrap().term("a").is_none());
        let c3 = p.process(&start, &doc, None, opts).unwrap();
        assert!(c3.term("b").is_some() && c3.previous().unwrap().term("b").is_none());
    }

    #[test]
    fn scoped_contexts_are_validated_and_kept_unprocessed() {
        let text = r#"{"@context": {
            "@vocab": "http://v.org/",
            "Person": {"@context": {"name": "http://schema.org/name"}},
            "knows": {"@id": "http://v.org/knows", "@context": [null, {"x": "http://x.org/"}]}
        }}"#;
        let c = ok(text);
        let scoped = c.term("Person").unwrap().context.clone().unwrap();
        assert!(scoped.source.is_none());
        assert_eq!(scoped.base_url.as_deref(), Some(BASE));
        assert!(c.term("name").is_none());
        let cache = ContextCache::new();
        let loader = NoNetwork::default();
        let mut p = Processor::with_cache(&loader, &cache);
        let applied = p
            .process_scoped(&c, &scoped, Span::default(), false, true)
            .unwrap();
        assert_eq!(iri(&applied, "name"), Some("http://schema.org/name"));
        assert!(applied.previous().unwrap().term("name").is_none());
        // Applying it again is memoized.
        let again = p
            .process_scoped(&c, &scoped, Span::default(), false, true)
            .unwrap();
        assert_eq!(again.id, applied.id);
        // An invalid scoped context is an error at the definition inside it.
        let text = r#"{"@context": {"Person": {"@id": "http://v.org/P", "@context": {"bad": 5}}}}"#;
        let e = process(text).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidScopedContext);
        assert!(
            e.message.starts_with("invalid term definition: "),
            "{}",
            e.message
        );
        assert_eq!(spanned(text, e.span), "bad");
        assert_eq!(
            e.to_syntax_error().message,
            format!("invalid scoped context: {}", e.message)
        );
        // Scoped contexts may redefine protected terms and reset the context.
        ok(
            r#"{"@context": {"@protected": true, "a": "http://a.org/", "T": {"@id": "http://t.org/", "@context": [null, {"a": "http://b.org/"}]}}}"#,
        );
    }

    #[test]
    fn a_remote_scoped_context_reports_at_the_reference() {
        let loader = NoNetwork::default().with(
            "http://example.com/remote",
            r#"{"@context": {"T": {"@id": "http://t.org/", "@context": {"x": "http://x.org/"}}}}"#,
        );
        let c = process_with(r#"{"@context": "remote"}"#, &loader).unwrap();
        let scoped = c.term("T").unwrap().context.clone().unwrap();
        assert_eq!(scoped.source.as_deref(), Some("http://example.com/remote"));
        assert_eq!(
            scoped.base_url.as_deref(),
            Some("http://example.com/remote")
        );
        let cache = ContextCache::new();
        let mut p = Processor::with_cache(&loader, &cache);
        let applied = p
            .process_scoped(&c, &scoped, Span::new(3, 4), true, true)
            .unwrap();
        assert_eq!(iri(&applied, "x"), Some("http://x.org/"));
    }

    #[test]
    fn import_merges_a_remote_context_under_the_local_one() {
        let loader = NoNetwork::default().with(
            "http://example.com/imported.jsonld",
            r#"{"@context": {"a": "http://imported.org/a", "b": "http://imported.org/b", "@vocab": "http://iv.org/"}}"#,
        );
        let c = process_with(
            r#"{"@context": {"@import": "imported.jsonld", "b": "http://local.org/b"}}"#,
            &loader,
        )
        .unwrap();
        assert_eq!(iri(&c, "a"), Some("http://imported.org/a"));
        assert_eq!(iri(&c, "b"), Some("http://local.org/b"));
        assert_eq!(c.vocab(), Some("http://iv.org/"));
        // Imported definitions have no span in the document.
        assert!(c.term("a").unwrap().span.is_none() && c.term("b").unwrap().span.is_some());
        // An error inside the imported context is reported at the @import.
        let loader =
            NoNetwork::default().with("http://example.com/bad.jsonld", r#"{"@context": {"a": 5}}"#);
        let text = r#"{"@context": {"@import": "bad.jsonld"}}"#;
        assert_eq!(
            fails_with(text, &loader),
            (ErrorCode::InvalidTermDefinition, "bad.jsonld".into())
        );
        let loader = NoNetwork::default().with(
            "http://example.com/nested.jsonld",
            r#"{"@context": {"@import": "x.jsonld"}}"#,
        );
        let text = r#"{"@context": {"@import": "nested.jsonld"}}"#;
        assert_eq!(
            fails_with(text, &loader),
            (ErrorCode::InvalidContextEntry, "nested.jsonld".into())
        );
        let loader =
            NoNetwork::default().with("http://example.com/arr.jsonld", r#"{"@context": []}"#);
        let text = r#"{"@context": {"@import": "arr.jsonld"}}"#;
        assert_eq!(
            fails_with(text, &loader),
            (ErrorCode::InvalidRemoteContext, "arr.jsonld".into())
        );
        assert_eq!(
            fails(r#"{"@context": {"@import": 5}}"#),
            (ErrorCode::InvalidImportValue, "5".into())
        );
    }

    #[test]
    fn remote_contexts_resolve_against_their_own_url() {
        let loader = NoNetwork::default()
            .with(
                "http://example.com/ctx/a.jsonld",
                r#"{"@context": ["b.jsonld", {"a": "http://a.org/", "@base": "http://ignored.org/"}]}"#,
            )
            .with("http://example.com/ctx/b.jsonld", r#"{"@context": {"b": "http://b.org/"}}"#);
        let c = process_with(r#"{"@context": "ctx/a.jsonld"}"#, &loader).unwrap();
        assert_eq!(iri(&c, "a"), Some("http://a.org/"));
        assert_eq!(iri(&c, "b"), Some("http://b.org/"));
        // @base in a remote context is ignored.
        assert_eq!(c.base(), Some(BASE));
        assert!(c.term("a").unwrap().span.is_none());
    }

    #[test]
    fn remote_context_failures_are_errors_at_the_reference() {
        let text = r#"{"@context": "https://example.org/missing.jsonld"}"#;
        assert_eq!(
            fails(text),
            (
                ErrorCode::LoadingRemoteContextFailed,
                "https://example.org/missing.jsonld".into()
            )
        );
        let e = process(text).unwrap_err();
        assert!(
            e.message.contains("imports.network") && e.message.contains("--network"),
            "{e}"
        );
        assert_eq!(
            fails(r#"{"@context": "urn:x:nothing"}"#),
            (
                ErrorCode::LoadingRemoteContextFailed,
                "urn:x:nothing".into()
            )
        );
        let loader = NoNetwork::default()
            .with("http://example.com/broken", "{")
            .with("http://example.com/plain", r#"{"a": 1}"#)
            .with(
                "http://example.com/bad",
                r#"{"@context": {"a": {"@id": 5}}}"#,
            );
        assert_eq!(
            fails_with(r#"{"@context": "broken"}"#, &loader),
            (ErrorCode::LoadingRemoteContextFailed, "broken".into())
        );
        assert_eq!(
            fails_with(r#"{"@context": "plain"}"#, &loader),
            (ErrorCode::InvalidRemoteContext, "plain".into())
        );
        let text = r#"{"@context": [{}, "bad"]}"#;
        let e = process_with(text, &loader).unwrap_err();
        assert_eq!(
            (e.code, spanned(text, e.span).as_str()),
            (ErrorCode::InvalidIriMapping, "bad")
        );
        assert!(e.message.ends_with("(in http://example.com/bad)"), "{e}");
    }

    #[test]
    fn a_context_including_itself_overflows() {
        let loader =
            NoNetwork::default().with("http://example.com/self", r#"{"@context": "self"}"#);
        assert_eq!(
            fails_with(r#"{"@context": "self"}"#, &loader),
            (ErrorCode::ContextOverflow, "self".into())
        );
        // A scoped context naming its own document is not followed again.
        let loader = NoNetwork::default().with(
            "http://example.com/scoped",
            r#"{"@context": {"t": {"@id": "http://t.org/", "@context": "scoped"}}}"#,
        );
        process_with(r#"{"@context": "scoped"}"#, &loader).unwrap();
    }

    #[test]
    fn cyclic_definitions() {
        assert_eq!(
            fails(r#"{"@context": {"a": "b:x", "b": "a:y"}}"#),
            (ErrorCode::CyclicIriMapping, "a".into())
        );
        assert_eq!(
            fails(r#"{"@context": {"a": {"@id": "a:x"}}}"#),
            (ErrorCode::CyclicIriMapping, "a".into())
        );
    }

    /// Each error code: in a term definition on the term's key, elsewhere on
    /// the offending value (a string's content without the quotes).
    #[test]
    fn every_error_code_at_its_span() {
        use ErrorCode::*;
        let cases: &[(&str, ErrorCode, &str)] = &[
            (r#"{"@context": 5}"#, InvalidLocalContext, "5"),
            (r#"{"@context": [true]}"#, InvalidLocalContext, "true"),
            (
                r#"{"@context": {"@version": 1.0}}"#,
                InvalidVersionValue,
                "1.0",
            ),
            (
                r#"{"@context": {"@version": "1.1"}}"#,
                InvalidVersionValue,
                "\"1.1\"",
            ),
            (r#"{"@context": {"@base": 5}}"#, InvalidBaseIri, "5"),
            (
                r#"{"@context": [{"@base": null}, {"@base": "rel"}]}"#,
                InvalidBaseIri,
                "rel",
            ),
            (r#"{"@context": {"@vocab": 5}}"#, InvalidVocabMapping, "5"),
            (
                r#"{"@context": [{"@base": null}, {"@vocab": "rel"}]}"#,
                InvalidVocabMapping,
                "rel",
            ),
            (
                r#"{"@context": {"@language": 5}}"#,
                InvalidDefaultLanguage,
                "5",
            ),
            (
                r#"{"@context": {"@direction": "up"}}"#,
                InvalidBaseDirection,
                "\"up\"",
            ),
            (
                r#"{"@context": {"@propagate": "no"}}"#,
                InvalidPropagateValue,
                "\"no\"",
            ),
            (
                r#"{"@context": {"@protected": 1}}"#,
                InvalidProtectedValue,
                "1",
            ),
            (
                r#"{"@context": {"": "http://x.org/"}}"#,
                InvalidTermDefinition,
                "",
            ),
            (r#"{"@context": {"a": 5}}"#, InvalidTermDefinition, "a"),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@foo": 1}}}"#,
                InvalidTermDefinition,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@index": "http://p/"}}}"#,
                InvalidTermDefinition,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@container": "@index", "@index": "@type"}}}"#,
                InvalidTermDefinition,
                "a",
            ),
            (
                r#"{"@context": {"@vocab": "http://x/", "a/b": {"@prefix": true}}}"#,
                InvalidTermDefinition,
                "a/b",
            ),
            (
                r#"{"@context": {"a": {"@id": "@type", "@prefix": true}}}"#,
                InvalidTermDefinition,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@prefix": "yes"}}}"#,
                InvalidPrefixValue,
                "a",
            ),
            (
                r#"{"@context": {"@id": "http://x/"}}"#,
                KeywordRedefinition,
                "@id",
            ),
            (
                r#"{"@context": {"@type": {"@id": "http://x/"}}}"#,
                KeywordRedefinition,
                "@type",
            ),
            (r#"{"@context": {"a": {"@id": 5}}}"#, InvalidIriMapping, "a"),
            (
                r#"{"@context": [{"@base": null}, {"a": "relative"}]}"#,
                InvalidIriMapping,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@type": "@id"}}}"#,
                InvalidIriMapping,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@reverse": 5}}}"#,
                InvalidIriMapping,
                "a",
            ),
            (
                r#"{"@context": {"a:b": {"@id": "http://x/other"}}}"#,
                InvalidIriMapping,
                "a:b",
            ),
            (
                r#"{"@context": {"a": "@context"}}"#,
                InvalidKeywordAlias,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@type": 5}}}"#,
                InvalidTypeMapping,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@type": "_:b"}}}"#,
                InvalidTypeMapping,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@type": "http://t/", "@container": "@type"}}}"#,
                InvalidTypeMapping,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@container": "@foo"}}}"#,
                InvalidContainerMapping,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@container": ["@list", "@set"]}}}"#,
                InvalidContainerMapping,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@container": ["@graph", "@id", "@index"]}}}"#,
                InvalidContainerMapping,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@container": ["@index", "@language"]}}}"#,
                InvalidContainerMapping,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@reverse": "http://x/", "@id": "http://y/"}}}"#,
                InvalidReverseProperty,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@reverse": "http://x/", "@container": "@list"}}}"#,
                InvalidReverseProperty,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@language": 5}}}"#,
                InvalidLanguageMapping,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@direction": "up"}}}"#,
                InvalidBaseDirection,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@nest": "@id"}}}"#,
                InvalidNestValue,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@nest": 5}}}"#,
                InvalidNestValue,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@protected": "yes"}}}"#,
                InvalidProtectedValue,
                "a",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@context": {"b": 5}}}}"#,
                InvalidScopedContext,
                "b",
            ),
            (
                r#"{"@context": {"a": {"@id": "http://x/", "@context": 5}}}"#,
                InvalidScopedContext,
                "a",
            ),
            (
                r#"{"@context": {"@import": "https://example.org/x"}}"#,
                LoadingRemoteContextFailed,
                "https://example.org/x",
            ),
        ];
        for (text, code, span) in cases {
            let got = fails(text);
            assert_eq!((got.0, got.1.as_str()), (*code, *span), "{text}");
        }
        let e = process(r#"{"@context": {"a": 5}}"#)
            .unwrap_err()
            .to_syntax_error();
        assert!(e.message.starts_with("invalid term definition: "));
        assert_eq!((e.code, e.severity), (DIAGNOSTIC_CODE, Severity::Error));
    }

    #[test]
    fn valid_containers() {
        let c = ok(r#"{"@context": {
            "a": {"@id": "http://x/a", "@container": ["@graph", "@id", "@set"]},
            "b": {"@id": "http://x/b", "@container": ["@set", "@type"]},
            "c": {"@id": "http://x/c", "@container": "@list"},
            "d": {"@id": "http://x/d", "@container": "@index", "@index": "http://x/p"},
            "e": {"@id": "http://x/e", "@container": null}
        }}"#);
        assert!(
            c.term("a")
                .unwrap()
                .container
                .contains(Container::GRAPH.with(Container::ID))
        );
        assert_eq!(c.term("b").unwrap().type_mapping.as_deref(), Some("@id"));
        assert_eq!(c.term("c").unwrap().container, Container::LIST);
        assert_eq!(c.term("d").unwrap().index.as_deref(), Some("http://x/p"));
        assert!(c.term("e").unwrap().container.is_empty());
    }

    #[test]
    fn iri_and_relative_terms() {
        let c = ok(
            r#"{"@context": {"@vocab": "http://v.org/", "http://x.org/p": {"@type": "@id"}, "rel/p": {}}}"#,
        );
        assert_eq!(iri(&c, "http://x.org/p"), Some("http://x.org/p"));
        assert_eq!(iri(&c, "rel/p"), Some("http://v.org/rel/p"));
        // An IRI-shaped term may map only to itself.
        ok(r#"{"@context": {"http://x.org/p": {"@id": "http://x.org/p"}}}"#);
        assert_eq!(
            fails(r#"{"@context": {"http://x.org/p": {"@id": "http://y.org/p"}}}"#),
            (ErrorCode::InvalidIriMapping, "http://x.org/p".into())
        );
    }

    #[test]
    fn rfc3986_resolution() {
        let base = "http://a/b/c/d;p?q";
        for (r, want) in [
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("", "http://a/b/c/d;p?q"),
            (".", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../g", "http://a/b/g"),
            ("../../../g", "http://a/g"),
            ("/./g", "http://a/g"),
            ("g;x=1/../y", "http://a/b/c/y"),
            ("g:h", "g:h"),
        ] {
            assert_eq!(resolve_iri(base, r), want, "{r}");
        }
        assert_eq!(resolve_iri("http://a", "b"), "http://a/b");
    }

    #[test]
    fn processed_remote_contexts_come_from_the_cache() {
        let loader = NoNetwork::default().with(
            "http://example.com/ctx",
            r#"{"@context": {"@vocab": "http://v.org/", "n": "http://n.org/"}}"#,
        );
        let cache = ContextCache::new();
        let doc = root(r#"{"@context": ["ctx", {"x": "http://x.org/"}]}"#);
        let run = |loader: &NoNetwork, base: &str| {
            Processor::with_cache(loader, &cache)
                .process(
                    &ActiveContext::new(Some(base)),
                    context_of(&doc),
                    Some(BASE),
                    Options::default(),
                )
                .unwrap()
        };
        let a = run(&loader, BASE);
        assert_eq!(cache.stats(), CacheStats { hits: 0, misses: 1 });
        let b = run(&loader, BASE);
        assert_eq!(cache.stats(), CacheStats { hits: 1, misses: 1 });
        assert_eq!(iri(&b, "n"), iri(&a, "n"));
        assert_eq!(iri(&b, "x"), Some("http://x.org/"));
        // Another document reuses it too: nothing in it used the base.
        let c = run(&loader, "http://other.org/doc");
        assert_eq!(cache.stats().hits, 2);
        assert_eq!(c.base(), Some("http://other.org/doc"));
        // New content is processed again.
        let changed = NoNetwork::default().with(
            "http://example.com/ctx",
            r#"{"@context": {"n": "http://n2.org/"}}"#,
        );
        assert_eq!(iri(&run(&changed, BASE), "n"), Some("http://n2.org/"));
        assert_eq!(cache.stats().misses, 2);
    }

    #[test]
    fn a_cached_context_is_reprocessed_when_a_nested_one_changed() {
        let cache = ContextCache::new();
        let text = r#"{"@context": "outer"}"#;
        let doc = root(text);
        let run = |inner: &str| {
            let loader = NoNetwork::default()
                .with("http://example.com/outer", r#"{"@context": "inner"}"#)
                .with("http://example.com/inner", inner.to_string());
            Processor::with_cache(&loader, &cache)
                .process(
                    &ActiveContext::new(Some(BASE)),
                    context_of(&doc),
                    Some(BASE),
                    Options::default(),
                )
                .map(|c| iri(&c, "a").map(str::to_string))
        };
        let one = r#"{"@context": {"a": "http://1/"}}"#;
        let two = r#"{"@context": {"a": "http://2/"}}"#;
        assert_eq!(run(one).unwrap().as_deref(), Some("http://1/"));
        assert_eq!(run(two).unwrap().as_deref(), Some("http://2/"));
        assert_eq!(run(two).unwrap().as_deref(), Some("http://2/"));
        assert_eq!(cache.stats().hits, 1);
        // A cached error is reported again, at the reference.
        let bad = r#"{"@context": {"a": 1}}"#;
        assert!(run(bad).is_err());
        let hits = cache.stats().hits;
        let e = run(bad).unwrap_err();
        assert_eq!(cache.stats().hits, hits + 1);
        assert_eq!(
            (e.code, spanned(text, e.span).as_str()),
            (ErrorCode::InvalidTermDefinition, "outer")
        );
        assert!(e.message.ends_with("(in http://example.com/inner)"), "{e}");
    }

    /// Processing schema.org's context a second time costs nearly nothing.
    #[test]
    fn the_bundled_schema_org_context_is_processed_once() {
        let cache = ContextCache::new();
        let doc = root(r#"{"@context": "https://schema.org/"}"#);
        let run = || {
            let t = std::time::Instant::now();
            let c = Processor::with_cache(&crate::jsonld::loader::Bundled, &cache)
                .process(
                    &ActiveContext::new(Some(BASE)),
                    context_of(&doc),
                    Some(BASE),
                    Options::default(),
                )
                .unwrap();
            (c, t.elapsed())
        };
        let (first, cold) = run();
        let (second, warm) = run();
        assert_eq!(cache.stats(), CacheStats { hits: 1, misses: 1 });
        assert_eq!(iri(&second, "Person"), Some("http://schema.org/Person"));
        assert!(first.terms().count() > 1000);
        eprintln!("schema.org context: cold {cold:?}, warm {warm:?}");
        if cfg!(not(debug_assertions)) {
            assert!(warm * 20 < cold, "cold {cold:?}, warm {warm:?}");
        }
    }
}
